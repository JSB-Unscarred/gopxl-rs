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
//!
//! 另含不做 I/O 的协议状态机：主连接的 FIFO 对应器 [`ControlCore`]、流连接的 [`StreamCore`]，以及按服务端规则
//! 判断通知投递的 [`SubscriptionExpr`]。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod state;
mod status;
mod sub;
mod url;

pub use state::{ControlCore, DisconnectReason, ReplyRx, StreamCore, StreamEvent};
pub use status::Status;
pub use sub::{SubExprError, SubscriptionExpr};
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
