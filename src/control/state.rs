//! 控制通道的协议状态机（纯逻辑，不做 I/O、不起线程）：主连接的 [`ControlCore`] 按 FIFO 把响应交给请求，
//! 流连接的 [`StreamCore`] 区分应答、数据与结束消息。连接层在锁外用 [`Inbound::decode`] 解码每一帧（坏帧即
//! [`DisconnectReason::Decode`]），再交给它们，按返回值投递或拆连接。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use super::{
    Inbound, InboundError, Method, Notification, Request, Response, Status, StreamMessage,
    StreamState,
};

/// 拆掉连接的原因。所有挂起的请求以同一个原因（`Arc`）失败：调用方能做的只有重连，所以各种原因不分别报错。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// 坏帧：服务端从不产生，字节流已不可信。
    #[error(transparent)]
    Decode(#[from] InboundError),
    /// 挂起队列为空时收到响应：之前某个响应已被错配，或这是一条孤立响应。
    #[error("response without a pending request")]
    Unsolicited,
    /// 响应的 `path` 与队首请求的路径不同：FIFO 已错位。
    #[error("response path {got:?} does not match request path {expected:?}")]
    Desync {
        /// 队首请求的路径。
        expected: String,
        /// 响应的路径。
        got: String,
    },
}

/// 一个请求的结果：响应，或拆连接的原因。
type Reply = Result<Response, Arc<DisconnectReason>>;

/// 一个请求的应答接收端，由 [`ControlCore::register`] 返回。
///
/// 调用方超时即丢弃它：槽位成为墓碑，照样占住 FIFO 中的位置吸收迟到的响应，后面的请求仍拿到自己的响应。
pub type ReplyRx = Receiver<Reply>;

/// 主连接的 FIFO 对应器，取代 C++ `GoRestClient` 的事务队列。
///
/// 线上不带请求 id：服务端每条连接一个 worker，按到达顺序处理请求，所以第 n 个响应属于第 n 个请求。
/// - 调用方在写锁内、帧的第一个字节写出之前 [`register`](Self::register)，槽位顺序因此等于线上顺序。
///   C++ SDK 先入队后发送，发送失败留下孤儿事务（重写方案 §5.4 第 1 条）。
/// - 服务端内部请求失败时会多发一条孤立响应，两个 SDK 都把它当作队首的应答，此后全部错位（§5.4 第 42 条）。
///   流与文件传输不走主连接，已知的触发条件不会出现在这里；其余来源用响应的 `path` 检测（见
///   [`DisconnectReason`]），拆连接好过把响应交给错误的调用方。
#[derive(Debug, Default)]
pub struct ControlCore {
    pending: VecDeque<Slot>,
}

/// 一个已写出、未应答的请求。
#[derive(Debug)]
struct Slot {
    /// 响应带 `path` 时应指向的路径；`create` 的响应可能回显新建子资源的路径（未确认，OQ-17），为 `None`，不比较。
    path: Option<String>,
    tx: SyncSender<Reply>,
}

impl ControlCore {
    /// 登记即将写出的请求并返回它的应答接收端；必须在它的第一个字节写出之前调用。
    ///
    /// 通道在这里建立：容量 1，且每个槽位只发送一次（应答或 `fail_all`），所以读线程持锁 `send` 时从不阻塞。
    pub fn register(&mut self, request: &Request) -> ReplyRx {
        let (tx, rx) = sync_channel(1);
        let path = (request.method != Method::Create).then(|| request.path.clone());
        self.pending.push_back(Slot { path, tx });
        rx
    }

    /// 处理一条消息：响应交给队首的请求，通知返回给连接层按订阅投递；流消息（主连接不开流）与未知 `type`
    /// 忽略。返回 `Err` 时连接层拆掉连接，再调用 [`fail_all`](Self::fail_all)。
    pub fn on_inbound(
        &mut self,
        inbound: Inbound,
    ) -> Result<Option<Notification>, DisconnectReason> {
        match inbound {
            Inbound::Response(response) => {
                let slot = self.pending.front().ok_or(DisconnectReason::Unsolicited)?;
                if let Some(expected) = &slot.path {
                    if !response.path.is_empty() && !same_path(expected, &response.path) {
                        return Err(DisconnectReason::Desync {
                            expected: expected.clone(),
                            got: response.path,
                        });
                    }
                }
                if let Some(slot) = self.pending.pop_front() {
                    // 发送失败即墓碑：调用方已超时，响应被丢弃。
                    let _ = slot.tx.send(Ok(response));
                }
                Ok(None)
            }
            Inbound::Notification(notification) => Ok(Some(notification)),
            Inbound::Stream(_) | Inbound::Other(_) => Ok(None),
        }
    }

    /// 拆连接时让所有挂起的请求以同一个原因失败，清空队列。
    pub fn fail_all(&mut self, reason: &Arc<DisconnectReason>) {
        for slot in self.pending.drain(..) {
            let _ = slot.tx.send(Err(Arc::clone(reason)));
        }
    }
}

/// 两个路径是否指向同一资源：服务端路由时按 `/` 切分并丢弃空段，所以 `/a//b/` 与 `/a/b` 相同。
fn same_path(a: &str, b: &str) -> bool {
    fn segments(path: &str) -> impl Iterator<Item = &str> {
        path.split('/').filter(|segment| !segment.is_empty())
    }
    segments(a).eq(segments(b))
}

/// 流连接的状态机。每个流一条自有连接，只发一个 `stream` 请求，所以只有“等应答”和“已应答”两个状态。
///
/// - `/dataSources` 的数据走服务端的另一个队列，可能先于应答到达，照常作为数据；
/// - 应答之后的响应都是服务端内部请求（`removeStream`、`streamStart`）失败产生的孤立响应，忽略。两个 SDK
///   在共享连接上把它当作下一个请求的应答（§5.4 第 42 条）。
#[derive(Debug, Default)]
pub struct StreamCore {
    replied: bool,
}

/// [`StreamCore::on_inbound`] 的结果。
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// `stream` 请求的应答；`status` 不是 OK 时流未开始。
    Reply(Response),
    /// 数据消息（`streaming`，以及未知的 `streamStatus`）。
    Data(StreamMessage),
    /// 结束消息：`OK` 表示 complete，否则是 abort 的原因（例如资源被删除时为 `CLOSED`）。服务端随后自行删除流。
    /// `/dataSources` 的数据可能紧跟在它之后：服务端每轮输出先写响应队列（含结束消息）、再写流队列。
    End(Status),
    /// 孤立响应、通知与未知 `type`。
    Ignored,
}

impl StreamCore {
    /// 处理流连接上的一条消息。
    pub fn on_inbound(&mut self, inbound: Inbound) -> StreamEvent {
        match inbound {
            Inbound::Response(response) if !self.replied => {
                self.replied = true;
                StreamEvent::Reply(response)
            }
            Inbound::Stream(message) => match message.state {
                StreamState::Complete | StreamState::Abort => StreamEvent::End(message.status),
                _ => StreamEvent::Data(message),
            },
            _ => StreamEvent::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::TryRecvError;

    use super::{ControlCore, DisconnectReason, ReplyRx, StreamCore, StreamEvent};
    use crate::control::tests::message;
    use crate::control::{Inbound, Method, Request, Response, Status};
    use crate::value::Value;

    /// 消息体为 `json` 的服务端消息。
    fn inbound(json: &str) -> Inbound {
        Inbound::decode(message(json)).unwrap()
    }

    fn register(core: &mut ControlCore, method: Method, path: &str) -> ReplyRx {
        core.register(&Request::new(method, path))
    }

    fn reply(rx: &ReplyRx) -> Response {
        rx.try_recv().unwrap().unwrap()
    }

    // FIFO 与墓碑（§5.4 第 1 条）：超时的请求丢弃接收端后，槽位仍吸收它的迟到响应，下一个请求拿到自己的响应；
    // 不带 path 的响应不比较，通知不占槽位。
    #[test]
    fn tombstone_absorbs_late_response() {
        let mut core = ControlCore::default();
        drop(register(&mut core, Method::Read, "/a"));
        let b = register(&mut core, Method::Read, "/b");

        let notify = inbound(r#"{"type":"notification","eventType":"updated","path":"/a"}"#);
        assert!(core.on_inbound(notify).unwrap().is_some());
        let late = inbound(r#"{"type":"response","status":1,"path":"/a","payload":"A"}"#);
        assert_eq!(core.on_inbound(late).unwrap(), None);
        assert!(matches!(b.try_recv(), Err(TryRecvError::Empty)));
        let own = inbound(r#"{"type":"response","status":-999,"payload":"B"}"#);
        assert_eq!(core.on_inbound(own).unwrap(), None);
        assert_eq!(reply(&b).payload, Value::from("B"));
    }

    // 守卫（§5.4 第 42 条）：path 按服务端的路由规则比较（空段不计），不一致即 Desync，槽位留在队列里由
    // fail_all 以同一个原因完成；create 豁免；挂起队列为空时 Unsolicited。主连接上的流消息与未知 type 忽略。
    #[test]
    fn guard_detects_desync_and_unsolicited() {
        let mut core = ControlCore::default();
        let response = |path: &str| {
            inbound(&format!(
                r#"{{"type":"response","status":1,"path":"{path}"}}"#
            ))
        };

        let rx = register(&mut core, Method::Update, "/tools//T-0/");
        assert_eq!(core.on_inbound(response("/tools/T-0")).unwrap(), None);
        assert_eq!(reply(&rx).status, Status::OK);

        let rx = register(&mut core, Method::Create, "/tools");
        assert_eq!(core.on_inbound(response("/tools/T-1")).unwrap(), None);
        assert_eq!(reply(&rx).path, "/tools/T-1");

        let ignored = [
            r#"{"type":"stream","streamId":1,"streamStatus":"streaming","status":1,"path":"/m"}"#,
            r#"{"type":"debug"}"#,
        ];
        for text in ignored {
            assert_eq!(core.on_inbound(inbound(text)).unwrap(), None);
        }
        assert!(matches!(
            core.on_inbound(response("/a")),
            Err(DisconnectReason::Unsolicited)
        ));

        let rx = register(&mut core, Method::Read, "/system");
        let error = core.on_inbound(response("/replay")).unwrap_err();
        assert!(matches!(
            &error,
            DisconnectReason::Desync { expected, got } if expected == "/system" && got == "/replay"
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        let reason = Arc::new(error);
        core.fail_all(&reason);
        assert!(Arc::ptr_eq(&rx.try_recv().unwrap().unwrap_err(), &reason));
    }

    // 拆连接（§5.4 第 2 条）：fail_all 让每个挂起的请求恰好以同一个原因失败一次，并清空队列。
    #[test]
    fn fail_all_completes_every_slot_once() {
        let mut core = ControlCore::default();
        let receivers: Vec<_> = (0..3)
            .map(|i| register(&mut core, Method::Read, &format!("/{i}")))
            .collect();
        let reason = Arc::new(DisconnectReason::Unsolicited);
        core.fail_all(&reason);
        for rx in &receivers {
            assert!(Arc::ptr_eq(&rx.try_recv().unwrap().unwrap_err(), &reason));
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
        }
        assert!(matches!(
            core.on_inbound(inbound(r#"{"type":"response","status":1}"#)),
            Err(DisconnectReason::Unsolicited)
        ));
    }

    // 流连接（§5.4 第 42 条）：应答前的数据照常产出；只有第一条响应是应答，之后的 −997 孤立响应忽略；
    // complete / abort 产出结束码。
    #[test]
    fn stream_core_tolerates_early_data_and_spurious_responses() {
        let stream = |state: &str, status: i32| {
            inbound(&format!(
                r#"{{"type":"stream","streamId":7,"streamStatus":"{state}","status":{status},"path":"/m"}}"#
            ))
        };
        let reply = r#"{"type":"response","status":1,"path":"/m","payload":{"streamId":7}}"#;
        let mut core = StreamCore::default();
        assert!(matches!(
            core.on_inbound(stream("streaming", 1)),
            StreamEvent::Data(_)
        ));
        assert!(matches!(
            core.on_inbound(inbound(reply)),
            StreamEvent::Reply(Response {
                status: Status::OK,
                ..
            })
        ));
        let cases = [
            (stream("complete", 1), StreamEvent::End(Status::OK)),
            (stream("abort", -990), StreamEvent::End(Status::CLOSED)),
            (
                inbound(r#"{"type":"response","status":-997,"path":"/m"}"#),
                StreamEvent::Ignored,
            ),
            (
                inbound(r#"{"type":"notification","eventType":"updated","path":"/m"}"#),
                StreamEvent::Ignored,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(core.on_inbound(input), expected);
        }
        assert!(matches!(
            core.on_inbound(stream("streaming", 1)),
            StreamEvent::Data(_)
        ));
    }
}
