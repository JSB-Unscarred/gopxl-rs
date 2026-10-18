//! 主连接的协议状态机 [`ControlCore`]（纯逻辑，不做 I/O、不起任务）：按发送顺序把响应交给请求。连接层在锁外用
//! [`Inbound::decode`] 解码每一帧（坏帧即 [`DisconnectReason::Decode`]），再交给它投递。

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::oneshot;

use super::{Inbound, Notification, Response};
use crate::error::DisconnectReason;

/// 一个请求的结果：响应，或断开连接的原因。
pub(super) type Reply = Result<Response, Arc<DisconnectReason>>;

/// 一个请求的应答接收端，由 [`ControlCore::register`] 返回。
///
/// 调用方超时即丢弃它：请求在等待队列里的位置保留，迟到的响应到达时被丢弃，后面的请求仍拿到自己的响应。
pub(super) type ReplyRx = oneshot::Receiver<Reply>;

/// 主连接的等待队列：按发送顺序保存挂起请求的应答通道，第 n 个响应交给第 n 个请求。
///
/// 线上不带请求 id：服务端每条连接一个 worker，按到达顺序处理请求，所以第 n 个响应属于第 n 个请求。调用方在写锁内、
/// 帧的第一个字节写出之前 [`register`](Self::register)，队列顺序因此等于线上顺序。流与文件传输各用自己的连接，
/// 主连接上只有一问一答的请求（流的原因见 [`StreamHandle`](super::StreamHandle)）；队列为空时到达的响应不属于任何
/// 请求，丢弃。
#[derive(Debug, Default)]
pub(super) struct ControlCore {
    pending: VecDeque<oneshot::Sender<Reply>>,
}

impl ControlCore {
    /// 登记即将写出的请求并返回它的应答接收端；必须在它的第一个字节写出之前调用。
    ///
    /// 通道在这里建立：每个请求只收到一次结果（应答或 `fail_all`），`oneshot` 的 `send` 从不等待。
    pub(super) fn register(&mut self) -> ReplyRx {
        let (tx, rx) = oneshot::channel();
        self.pending.push_back(tx);
        rx
    }

    /// 处理一条消息：响应交给队首的请求，通知返回给连接层按订阅投递；流消息（主连接不开流）与未知 `type` 忽略。
    pub(super) fn on_inbound(&mut self, inbound: Inbound) -> Option<Notification> {
        match inbound {
            Inbound::Response(response) => {
                if let Some(tx) = self.pending.pop_front() {
                    // 发送失败说明调用方已超时，响应丢弃。
                    let _ = tx.send(Ok(response));
                }
                None
            }
            Inbound::Notification(notification) => Some(notification),
            Inbound::Stream(_) | Inbound::Other => None,
        }
    }

    /// 断开连接时让所有挂起的请求以同一个原因失败，清空队列。
    pub(super) fn fail_all(&mut self, reason: &Arc<DisconnectReason>) {
        for tx in self.pending.drain(..) {
            let _ = tx.send(Err(Arc::clone(reason)));
        }
    }
}

// 请求与响应的配对出错时，响应会静默交给错误的调用方。
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use tokio::sync::oneshot::error::TryRecvError;

    use super::{ControlCore, DisconnectReason};
    use crate::control::Inbound;
    use crate::value::{Value, msgpack};

    /// 消息体为 `text`（JSON）的服务端消息：carrier 头（`Length`、0xB000、`CarrierStatus` 1、`DataLength`）加消息体。
    fn inbound(text: &str) -> Inbound {
        let mut body = Vec::new();
        msgpack::encode(&text.parse::<Value>().unwrap(), &mut body).unwrap();
        let n = u32::try_from(body.len()).unwrap();
        let mut frame = (n + 14).to_le_bytes().to_vec();
        frame.extend(0xB000_u16.to_le_bytes());
        frame.extend(1_i32.to_le_bytes());
        frame.extend(n.to_le_bytes());
        frame.extend(body);
        Inbound::decode(Bytes::from(frame)).unwrap()
    }

    // 按顺序配对：超时的请求丢弃接收端后，它的位置仍接住迟到的响应，下一个请求拿到自己的响应；通知、流消息与未知
    // type 不占位置；队列为空时的响应丢弃。
    #[test]
    fn tombstone_absorbs_late_response() {
        let mut core = ControlCore::default();
        drop(core.register());
        let mut b = core.register();

        let notify = inbound(r#"{"type":"notification","eventType":"updated","path":"/a"}"#);
        assert!(core.on_inbound(notify).is_some());
        let ignored = [
            r#"{"type":"stream","streamId":1,"streamStatus":"streaming","status":1,"path":"/m"}"#,
            r#"{"type":"debug"}"#,
        ];
        for text in ignored {
            assert!(core.on_inbound(inbound(text)).is_none());
        }
        let late = inbound(r#"{"type":"response","status":1,"payload":"A"}"#);
        assert!(core.on_inbound(late).is_none());
        assert!(matches!(b.try_recv(), Err(TryRecvError::Empty)));
        let own = inbound(r#"{"type":"response","status":-999,"payload":"B"}"#);
        assert!(core.on_inbound(own).is_none());
        assert_eq!(b.try_recv().unwrap().unwrap().payload, Value::from("B"));
        let stray = inbound(r#"{"type":"response","status":1}"#);
        assert!(core.on_inbound(stray).is_none());
    }

    // 断开连接：fail_all 让每个挂起的请求恰好以同一个原因失败一次，并清空队列。
    #[test]
    fn fail_all_completes_every_slot_once() {
        let mut core = ControlCore::default();
        let receivers: Vec<_> = (0..3).map(|_| core.register()).collect();
        let reason = Arc::new(DisconnectReason::Closed);
        core.fail_all(&reason);
        for mut rx in receivers {
            assert!(Arc::ptr_eq(&rx.try_recv().unwrap().unwrap_err(), &reason));
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Closed)));
        }
    }
}
