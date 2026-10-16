//! 控制通道（TCP）的线上编解码：carrier 帧与 MessagePack 消息体，对应 C++ SDK 的 `GoRequest`、`GoResponse`
//! 与 `GoCtrlChannelV1` 的帧格式。
//!
//! carrier 帧的整数都是小端，`Length` 包含自身 4 字节：
//!
//! | 方向 | 帧头 | 消息体 |
//! |---|---|---|
//! | 客户端 → 服务端 | `u32 Length = 10 + N`、`u16 0xB000`、`u32 N` | N 字节 MessagePack |
//! | 服务端 → 客户端 | `u32 Length = 14 + N`、`u16 0xB000`、`i32 CarrierStatus = 1`、`u32 N` | N 字节 MessagePack |
//!
//! 只用 0xB000（MessagePack）：服务端按请求的格式回复，通知与流消息跟随最近一次请求的格式，所以只发 0xB000 的
//! 客户端只会收到 0xB000（C++ SDK 也只收 0xB000）。
//!
//! 服务端从不产生的输入一律是坏帧，[`Inbound::decode`] 返回 `Err`，由连接层拆掉连接：类型不是 0xB000、
//! `CarrierStatus` 不是 1、`DataLength` 超出帧、消息体不是 MessagePack 对象，或缺少 / 类型错误的必需字段（包括
//! response 的 `status`）。C++ SDK 忽略 `CarrierStatus`；`DataLength` 越界时读进下一帧而错位；其余情况报读错误并
//! 丢弃该消息，等待中的请求不会失败，FIFO 随之错位；缺少 `status` 的响应被当作成功（重写方案 §5.4 第 6、7 条）。
//! 未知的 `type`（如 `debug`）得到 [`Inbound::Other`] 由调用方忽略；C++ 把它当作响应，会错配等待中的请求。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod status;
mod url;

pub use status::Status;
pub use url::{url_decode, url_encode};

use bytes::{Buf, Bytes};

use crate::value::msgpack::{self, DecodeError, EncodeError, write_str};
use crate::value::{Map, Value};

/// 控制通道的默认端口（`GO_PXL_SDK_DEFAULT_CONTROL_PORT`）。
pub const DEFAULT_PORT: u16 = 3600;

/// 服务端 → 客户端 carrier 帧头的长度，也是最短的合法帧；连接层用 `FrameReader::new(HEADER_LEN, max)` 读帧。
pub const HEADER_LEN: u32 = 14;

/// 客户端 → 服务端 carrier 帧头的长度。
const REQUEST_HEADER_LEN: u32 = 10;

/// carrier 类型：消息体是 MessagePack。
const MSGPACK: u16 = 0xB000;

/// 请求方法，对应 `GoRequestMethod`。服务端按字符串精确匹配（区分大小写）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// `create`：创建资源。
    Create,
    /// `read`：读取资源。
    Read,
    /// `update`：修改资源。
    Update,
    /// `delete`：删除资源。
    Delete,
    /// `call`：调用命令或动作。
    Call,
    /// `sub`：订阅资源的变更通知。
    Sub,
    /// `unSub`：取消订阅。
    UnSub,
    /// `listSub`：列出当前的订阅（C++ SDK 未提供）。
    ListSub,
    /// `clearSub`：清除全部订阅（C++ SDK 未提供）。
    ClearSub,
    /// `stream`：开始流（C++ 的 `StartStream`）。
    Stream,
    /// `cancelStream`：结束流，路径与 `stream` 请求的完全相同（C++ 的 `StopStream`）。
    CancelStream,
    /// `listStream`：列出当前的流（C++ SDK 未提供）。
    ListStream,
}

impl Method {
    /// 线上的方法名。拼写与服务端一致（`unSub`、`cancelStream` …）；.NET SDK 发送全小写的名字，服务端回复 −997
    /// （重写方案 §5.4 第 9 条）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Read => "read",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Call => "call",
            Self::Sub => "sub",
            Self::UnSub => "unSub",
            Self::ListSub => "listSub",
            Self::ClearSub => "clearSub",
            Self::Stream => "stream",
            Self::CancelStream => "cancelStream",
            Self::ListStream => "listStream",
        }
    }
}

/// 一个请求，对应 `GoRequest`。线上不带请求 id，响应按发送顺序与请求配对。
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// 方法。
    pub method: Method,
    /// 资源路径，原样发送，不做规范化或编码。
    pub path: String,
    /// 载荷，例如 `update` 的补丁、`stream` 的 `period`。
    pub payload: Value,
    /// 参数，例如 `read` 的 `expandLevel`、`includeSchema`。
    pub args: Value,
}

impl Request {
    /// `payload` 与 `args` 为空对象（线上 `0x80`），与 C++ 的默认值 `GoJson()` 逐字节一致。
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            payload: Value::object(),
            args: Value::object(),
        }
    }

    /// 把整帧（carrier 头与消息体）追加到 `out`，对应 `GoRequest::ToByteArray` 加上 carrier 头。
    ///
    /// 消息体与 C++ SDK 逐字节一致：4 个键按字节序 `args`、`method`、`path`、`payload` 直接写出，不克隆载荷。
    /// `Length` 超过 `u32::MAX` 时返回 `EncodeError` 并把 `out` 恢复到原长度：C++ 先写出截断的长度再报错，
    /// 字节流随之错位（重写方案 §5.4 第 21 条）。
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        let start = out.len();
        let header = start..start + REQUEST_HEADER_LEN as usize;
        // 帧头占位，消息体写完后回填。
        out.extend_from_slice(&[0; REQUEST_HEADER_LEN as usize]);
        let len = self
            .write_body(out)
            .and_then(|()| u32::try_from(out.len() - start).map_err(|_| EncodeError));
        match len {
            Ok(len) => {
                let n = len - REQUEST_HEADER_LEN;
                let bytes = len.to_le_bytes().into_iter().chain(MSGPACK.to_le_bytes());
                out.splice(header, bytes.chain(n.to_le_bytes()));
                Ok(())
            }
            Err(e) => {
                out.truncate(start);
                Err(e)
            }
        }
    }

    /// 消息体：4 个键的对象，键按字节序写出（与 nlohmann 的 `std::map` 相同）。
    fn write_body(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        out.push(0x84);
        write_str(out, "args")?;
        msgpack::encode(&self.args, out)?;
        write_str(out, "method")?;
        write_str(out, self.method.as_str())?;
        write_str(out, "path")?;
        write_str(out, &self.path)?;
        write_str(out, "payload")?;
        msgpack::encode(&self.payload, out)
    }
}

/// 请求的响应（`type: "response"`），对应 `GoRequestResponse`。
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// 请求的结果；`status` 为 OK 时载荷仍可能带 `errors`（例如重复开始同一个流）。
    pub status: Status,
    /// 资源路径；服务端只在处理函数给出时写入，缺失时为空串（同 C++ 的 `Path()`），不能用来配对请求。
    pub path: String,
    /// 载荷，任意类型；缺失时为 `Nil`。
    pub payload: Value,
}

/// 订阅的通知（`type: "notification"`），对应 `GoNotificationResponse`。
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    /// 事件类型。
    pub event: EventType,
    /// 发生变化的资源路径。
    pub path: String,
    /// 载荷；服务端只为 `/system` 与 `/replay` 的通知写入非 `Nil` 的载荷。
    pub payload: Value,
}

/// 通知的事件类型（`eventType`），对应 `GoNotificationType`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventType {
    /// `created`。
    Created,
    /// `updated`。
    Updated,
    /// `deleted`。
    Deleted,
    /// `embeddedUpdated`：嵌入的子资源发生变化。
    EmbeddedUpdated,
    /// 其他字符串（服务端对非法枚举值写 `"Unknown"`）。C++ 把它们当作 `Updated`（重写方案 §5.4 第 11 条）。
    Other(String),
}

impl EventType {
    /// 解析 `eventType`，未知的字符串放进 `Other`。
    fn parse(text: String) -> Self {
        match text.as_str() {
            "created" => Self::Created,
            "updated" => Self::Updated,
            "deleted" => Self::Deleted,
            "embeddedUpdated" => Self::EmbeddedUpdated,
            _ => Self::Other(text),
        }
    }
}

/// 流消息（`type: "stream"`），对应 `GoStreamResponse`。
#[derive(Clone, Debug, PartialEq)]
pub struct StreamMessage {
    /// 流 id，即 `stream` 请求的响应载荷中的 `streamId`。
    pub stream_id: u32,
    /// 流的状态。
    pub state: StreamState,
    /// `complete` 时为 OK，`abort` 时为中止原因（例如资源被删除时为 `CLOSED`）。
    pub status: Status,
    /// 流的资源路径。
    pub path: String,
    /// 数据消息是一条 GDP 消息（`Bin`，帧的零拷贝切片），metrics 是对象，结束消息为 `Nil`。
    pub payload: Value,
}

/// 流的状态（`streamStatus`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// `streaming`：流进行中，消息带数据。
    Streaming,
    /// `complete`：流正常结束。
    Complete,
    /// `abort`：流被中止。
    Abort,
    /// 其他字符串。
    Other(String),
}

impl StreamState {
    /// 解析 `streamStatus`，未知的字符串放进 `Other`。
    fn parse(text: String) -> Self {
        match text.as_str() {
            "streaming" => Self::Streaming,
            "complete" => Self::Complete,
            "abort" => Self::Abort,
            _ => Self::Other(text),
        }
    }
}

/// 服务端发来的一条消息，按 `type` 分类。
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    /// `response`：按发送顺序对应最早一个未完成的请求。
    Response(Response),
    /// `notification`。
    Notification(Notification),
    /// `stream`。
    Stream(StreamMessage),
    /// 未知的 `type`（例如隐藏调试命令打开的 `debug`），携带该字符串，由调用方忽略。
    Other(String),
}

impl Inbound {
    /// 解码一帧。`frame` 是 [`FrameReader`](crate::frame::FrameReader) 返回的整帧，`Length` 必须等于帧长；
    /// `DataLength` 之后的尾随字节忽略（与 SDK 相同）。
    ///
    /// 必需字段：response 的 `status`（整数且在 `i32` 内）；notification 的 `eventType`、`path`；stream 的
    /// `streamId`（整数且在 `u32` 内）、`streamStatus`、`status`、`path`。`payload` 缺失为 `Nil`。
    pub fn decode(frame: Bytes) -> Result<Self, InboundError> {
        let body = carrier_body(frame).ok_or(InboundError::Header)?;
        let Value::Map(mut msg) = msgpack::decode(&body)? else {
            return Err(InboundError::Field("type"));
        };
        let ty = take_str(&mut msg, "type")?;
        let payload = msg.remove("payload").unwrap_or_default();
        Ok(match ty.as_str() {
            "response" => Self::Response(Response {
                status: Status(get_int(&msg, "status")?),
                path: match msg.remove("path") {
                    None => String::new(),
                    Some(Value::Str(path)) => path,
                    Some(_) => return Err(InboundError::Field("path")),
                },
                payload,
            }),
            "notification" => Self::Notification(Notification {
                event: EventType::parse(take_str(&mut msg, "eventType")?),
                path: take_str(&mut msg, "path")?,
                payload,
            }),
            "stream" => Self::Stream(StreamMessage {
                stream_id: get_int(&msg, "streamId")?,
                state: StreamState::parse(take_str(&mut msg, "streamStatus")?),
                status: Status(get_int(&msg, "status")?),
                path: take_str(&mut msg, "path")?,
                payload,
            }),
            _ => Self::Other(ty),
        })
    }
}

/// [`Inbound::decode`] 的错误：坏帧，字节流已不可信，连接应当关闭。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InboundError {
    /// 帧短于 [`HEADER_LEN`]、`Length` 不等于帧长、类型不是 0xB000、`CarrierStatus` 不是 1，或 `DataLength` 超出帧。
    #[error("malformed control carrier header")]
    Header,
    /// 消息体不是一条合法的 MessagePack 文档。
    #[error(transparent)]
    Body(#[from] DecodeError),
    /// 必需字段缺失或类型错误；消息体顶层不是对象时报 `"type"`。
    #[error("control message field {0:?} is missing or has the wrong type")]
    Field(&'static str),
}

/// 校验服务端 carrier 头并切出消息体（零拷贝）。`Length` 必须等于帧长，与 [`gdp::decode_message`](crate::gdp::decode_message)
/// 的规则相同。
fn carrier_body(mut frame: Bytes) -> Option<Bytes> {
    let len = frame.len();
    let length = frame.try_get_u32_le().ok()?;
    let ty = frame.try_get_u16_le().ok()?;
    let carrier_status = frame.try_get_i32_le().ok()?;
    let n = frame.try_get_u32_le().ok()? as usize;
    if length as usize == len && ty == MSGPACK && carrier_status == 1 && n <= frame.len() {
        Some(frame.split_to(n))
    } else {
        None
    }
}

/// 取出字符串字段；缺失或不是字符串时报 `Field(key)`。
fn take_str(msg: &mut Map, key: &'static str) -> Result<String, InboundError> {
    match msg.remove(key) {
        Some(Value::Str(text)) => Ok(text),
        _ => Err(InboundError::Field(key)),
    }
}

/// 读取整数字段（任意 marker 宽度）；缺失、不是整数或超出 `T` 时报 `Field(key)`。
/// C++ 用 `static_cast` 接受布尔与浮点数并回绕越界值，这里一律视为坏帧。
fn get_int<T: TryFrom<i64>>(msg: &Map, key: &'static str) -> Result<T, InboundError> {
    msg.get(key)
        .and_then(Value::as_i64)
        .and_then(|v| T::try_from(v).ok())
        .ok_or(InboundError::Field(key))
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{
        EventType, Inbound, InboundError, Method, Notification, Request, Response, Status,
        StreamMessage, StreamState,
    };
    use crate::hex;
    use crate::value::{Value, msgpack};

    fn json(text: &str) -> Value {
        text.parse().unwrap()
    }

    /// 服务端 carrier 帧：帧头、消息体、`DataLength` 之后的尾随字节。
    fn carrier(ty: u16, carrier_status: i32, body: &[u8], trailing: &[u8]) -> Bytes {
        let len = u32::try_from(14 + body.len() + trailing.len()).unwrap();
        let n = u32::try_from(body.len()).unwrap();
        let mut frame = len.to_le_bytes().to_vec();
        frame.extend_from_slice(&ty.to_le_bytes());
        frame.extend_from_slice(&carrier_status.to_le_bytes());
        frame.extend_from_slice(&n.to_le_bytes());
        frame.extend_from_slice(body);
        frame.extend_from_slice(trailing);
        Bytes::from(frame)
    }

    /// 消息体为 `json` 的正常 carrier 帧。
    fn message(text: &str) -> Bytes {
        let mut body = Vec::new();
        msgpack::encode(&json(text), &mut body).unwrap();
        carrier(0xB000, 1, &body, &[])
    }

    // G1–G6 与 unSub、cancelStream（derived，nlohmann 3.8.0 等价编码器生成）：请求帧与 C++ SDK 逐字节一致，
    // 方法名用服务端的精确拼写；连续追加到同一个缓冲区时帧头回填在各自的起点。
    #[test]
    fn request_frames_match_cpp_sdk() {
        let bin = Value::from(vec![0_u8, 1, 2, 3]);
        let mut g4 = Request::new(Method::Read, "/system");
        g4.args = json(r#"{"expandLevel":0,"includeSchema":false}"#);
        let mut g5 = Request::new(Method::Call, "/system/commands/restore");
        g5.payload = json(r#"{"contents":["liveJob","replay"]}"#);
        g5.payload.set("/data", bin.clone()).unwrap();
        let mut g6 = Request::new(Method::Call, "/tools/T-0/actions/importFile");
        g6.payload = json(r#"{"fileArg":{"filename":"a.csv"}}"#);
        g6.payload.set("/fileArg/data", bin).unwrap();

        let cases = [
            (
                Request::new(Method::Read, "/system/info"),
                "38 00 00 00 00 B0 2E 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 A4 72 65 61 64 A4 70 61 74 68 AC 2F 73 79 73 74 65 6D \
                 2F 69 6E 66 6F A7 70 61 79 6C 6F 61 64 80",
            ),
            (
                Request::new(Method::Read, "/system"),
                "33 00 00 00 00 B0 29 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 A4 72 65 61 64 A4 70 61 74 68 A7 2F 73 79 73 74 65 6D \
                 A7 70 61 79 6C 6F 61 64 80",
            ),
            (
                Request::new(Method::Sub, "/system"),
                "32 00 00 00 00 B0 28 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 A3 73 75 62 A4 70 61 74 68 A7 2F 73 79 73 74 65 6D \
                 A7 70 61 79 6C 6F 61 64 80",
            ),
            (
                g4,
                "4F 00 00 00 00 B0 45 00 00 00 \
                 84 A4 61 72 67 73 82 AB 65 78 70 61 6E 64 4C 65 76 65 6C 00 AD 69 6E 63 6C 75 64 65 53 63 68 65 \
                 6D 61 C2 A6 6D 65 74 68 6F 64 A4 72 65 61 64 A4 70 61 74 68 A7 2F 73 79 73 74 65 6D A7 70 61 79 \
                 6C 6F 61 64 80",
            ),
            (
                g5,
                "68 00 00 00 00 B0 5E 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 A4 63 61 6C 6C A4 70 61 74 68 B8 2F 73 79 73 74 65 6D \
                 2F 63 6F 6D 6D 61 6E 64 73 2F 72 65 73 74 6F 72 65 A7 70 61 79 6C 6F 61 64 82 A8 63 6F 6E 74 65 \
                 6E 74 73 92 A7 6C 69 76 65 4A 6F 62 A6 72 65 70 6C 61 79 A4 64 61 74 61 C4 04 00 01 02 03",
            ),
            (
                g6,
                "6C 00 00 00 00 B0 62 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 A4 63 61 6C 6C A4 70 61 74 68 BD 2F 74 6F 6F 6C 73 2F \
                 54 2D 30 2F 61 63 74 69 6F 6E 73 2F 69 6D 70 6F 72 74 46 69 6C 65 A7 70 61 79 6C 6F 61 64 81 A7 \
                 66 69 6C 65 41 72 67 82 A4 64 61 74 61 C4 04 00 01 02 03 A8 66 69 6C 65 6E 61 6D 65 A5 61 2E 63 \
                 73 76",
            ),
            (
                Request::new(Method::UnSub, "/system"),
                "34 00 00 00 00 B0 2A 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 A5 75 6E 53 75 62 A4 70 61 74 68 A7 2F 73 79 73 74 65 \
                 6D A7 70 61 79 6C 6F 61 64 80",
            ),
            (
                Request::new(Method::CancelStream, "/dataSources/x/data"),
                "47 00 00 00 00 B0 3D 00 00 00 \
                 84 A4 61 72 67 73 80 A6 6D 65 74 68 6F 64 AC 63 61 6E 63 65 6C 53 74 72 65 61 6D A4 70 61 74 68 \
                 B3 2F 64 61 74 61 53 6F 75 72 63 65 73 2F 78 2F 64 61 74 61 A7 70 61 79 6C 6F 61 64 80",
            ),
        ];
        let mut out = vec![0xEE];
        for (request, expected) in cases {
            let start = out.len();
            request.encode(&mut out).unwrap();
            assert_eq!(
                out[start..],
                hex(expected),
                "{} {}",
                request.method.as_str(),
                request.path
            );
        }
        assert_eq!(out[0], 0xEE);
    }

    // G9、G10、G10b、G11（derived，按服务端的插入键序与 C 类型宽度生成）：解码出类型化的字段，
    // 不依赖键序与整数宽度；G11 的 GDP 载荷原样成为 Bin。
    #[test]
    fn server_messages_decode_to_typed_fields() {
        let stream = |state, status, path: &str, payload| {
            Inbound::Stream(StreamMessage {
                stream_id: 1,
                state,
                status,
                path: path.into(),
                payload,
            })
        };
        let mut gdp = hex("23 01 00 00");
        gdp.resize(291, 0x5A);
        let g11 = [
            hex("8F 01 00 00 00 B0 01 00 00 00 81 01 00 00"),
            hex(
                "86 A8 73 74 72 65 61 6D 49 64 01 AC 73 74 72 65 61 6D 53 74 61 74 75 73 A9 73 74 72 65 61 \
                 6D 69 6E 67 A7 70 61 79 6C 6F 61 64 C5 01 23",
            ),
            gdp.clone(),
            hex(
                "A4 70 61 74 68 B3 2F 64 61 74 61 53 6F 75 72 63 65 73 2F 78 2F 64 61 74 61 A4 74 79 70 65 \
                 A6 73 74 72 65 61 6D A6 73 74 61 74 75 73 D2 00 00 00 01",
            ),
        ]
        .concat();

        let cases = [
            (
                hex("58 00 00 00 00 B0 01 00 00 00 4A 00 00 00 \
                     85 A9 65 76 65 6E 74 54 79 70 65 A7 63 72 65 61 74 65 64 A7 70 61 79 6C 6F 61 64 C0 A4 70 \
                     61 74 68 AA 2F 74 6F 6F 6C 73 2F 54 2D 30 A4 74 79 70 65 AC 6E 6F 74 69 66 69 63 61 74 69 \
                     6F 6E A6 73 74 61 74 75 73 D2 00 00 00 01"),
                Inbound::Notification(Notification {
                    event: EventType::Created,
                    path: "/tools/T-0".into(),
                    payload: Value::Nil,
                }),
            ),
            (
                hex("58 00 00 00 00 B0 01 00 00 00 4A 00 00 00 \
                     86 A8 73 74 72 65 61 6D 49 64 01 AC 73 74 72 65 61 6D 53 74 61 74 75 73 A8 63 6F 6D 70 6C \
                     65 74 65 A7 70 61 79 6C 6F 61 64 C0 A4 70 61 74 68 A2 2F 61 A4 74 79 70 65 A6 73 74 72 65 \
                     61 6D A6 73 74 61 74 75 73 D2 00 00 00 01"),
                stream(StreamState::Complete, Status::OK, "/a", Value::Nil),
            ),
            (
                hex("55 00 00 00 00 B0 01 00 00 00 47 00 00 00 \
                     86 A8 73 74 72 65 61 6D 49 64 01 AC 73 74 72 65 61 6D 53 74 61 74 75 73 A5 61 62 6F 72 74 \
                     A7 70 61 79 6C 6F 61 64 C0 A4 70 61 74 68 A2 2F 61 A4 74 79 70 65 A6 73 74 72 65 61 6D A6 \
                     73 74 61 74 75 73 D2 FF FF FC 22"),
                stream(StreamState::Abort, Status::CLOSED, "/a", Value::Nil),
            ),
            (
                g11,
                stream(
                    StreamState::Streaming,
                    Status::OK,
                    "/dataSources/x/data",
                    gdp.into(),
                ),
            ),
        ];
        for (frame, expected) in cases {
            assert_eq!(Inbound::decode(Bytes::from(frame)).unwrap(), expected);
        }
    }

    // 修 C++ 的第 7、11 条：未知 type 得到 Other 而不是响应，未知 eventType / streamStatus 保留原字符串；
    // response 缺 path 为空串、缺 payload 为 Nil；`DataLength` 之后的尾随字节忽略（与 SDK 的 `EndRead` 相同）。
    #[test]
    fn unknown_strings_are_kept_and_trailing_bytes_ignored() {
        let mut response = Vec::new();
        msgpack::encode(&json(r#"{"type":"response","status":-999}"#), &mut response).unwrap();
        let cases = [
            (
                message(r#"{"type":"debug","payload":1}"#),
                Inbound::Other("debug".into()),
            ),
            (
                message(r#"{"type":"notification","eventType":"Unknown","path":"/a"}"#),
                Inbound::Notification(Notification {
                    event: EventType::Other("Unknown".into()),
                    path: "/a".into(),
                    payload: Value::Nil,
                }),
            ),
            (
                message(
                    r#"{"type":"stream","streamId":4294967295,"streamStatus":"paused","status":-988,"path":"/a"}"#,
                ),
                Inbound::Stream(StreamMessage {
                    stream_id: u32::MAX,
                    state: StreamState::Other("paused".into()),
                    status: Status::ABORT,
                    path: "/a".into(),
                    payload: Value::Nil,
                }),
            ),
            (
                carrier(0xB000, 1, &response, b"\xC1\xFF\x00"),
                Inbound::Response(Response {
                    status: Status::NOT_FOUND,
                    path: String::new(),
                    payload: Value::Nil,
                }),
            ),
        ];
        for (frame, expected) in cases {
            assert_eq!(Inbound::decode(frame).unwrap(), expected);
        }
    }

    // 坏帧规则：服务端从不产生的输入一律是 Err。帧头错误（含整帧的每个真前缀）报 Header 而不 panic；
    // 必需字段缺失或类型错误报对应的字段名，缺 status 不会被当作成功（修 C++ 的第 7 条）。
    #[test]
    fn malformed_frames_are_errors() {
        let ok = message(r#"{"type":"response","status":1}"#);
        for end in 0..ok.len() {
            assert_eq!(
                Inbound::decode(ok.slice(..end)),
                Err(InboundError::Header),
                "prefix of {end} bytes"
            );
        }

        let body = &ok[14..];
        // Length 与帧长不符、DataLength 超出帧。
        let patched = |at: usize| {
            let mut frame = ok.to_vec();
            frame[at] += 1;
            Bytes::from(frame)
        };
        let header_cases = [
            carrier(0xB001, 1, body, &[]),
            carrier(0xB000, 0, body, &[]),
            carrier(0xB000, -998, body, &[]),
            patched(0),
            patched(10),
        ];
        for frame in header_cases {
            assert_eq!(Inbound::decode(frame), Err(InboundError::Header));
        }

        assert!(matches!(
            Inbound::decode(carrier(0xB000, 1, &[], &[])),
            Err(InboundError::Body(_))
        ));

        let field_cases = [
            ("[1]", "type"),
            ("{}", "type"),
            (r#"{"type":1}"#, "type"),
            (r#"{"type":"response"}"#, "status"),
            (r#"{"type":"response","status":true}"#, "status"),
            (r#"{"type":"response","status":2147483648}"#, "status"),
            (r#"{"type":"response","status":1,"path":null}"#, "path"),
            (r#"{"type":"notification","path":"/a"}"#, "eventType"),
            (r#"{"type":"notification","eventType":"created"}"#, "path"),
            (
                r#"{"type":"stream","streamId":-1,"streamStatus":"abort","status":1,"path":"/a"}"#,
                "streamId",
            ),
            (
                r#"{"type":"stream","streamId":4294967296,"streamStatus":"abort","status":1,"path":"/a"}"#,
                "streamId",
            ),
            (
                r#"{"type":"stream","streamId":1,"status":1,"path":"/a"}"#,
                "streamStatus",
            ),
            (
                r#"{"type":"stream","streamId":1,"streamStatus":"abort","path":"/a"}"#,
                "status",
            ),
            (
                r#"{"type":"stream","streamId":1,"streamStatus":"abort","status":1}"#,
                "path",
            ),
        ];
        for (text, field) in field_cases {
            assert_eq!(
                Inbound::decode(message(text)),
                Err(InboundError::Field(field)),
                "{text}"
            );
        }
    }
}
