//! 主连接的协议状态机 [`ControlCore`]（纯逻辑，不做 I/O、不起线程）：按 FIFO 把响应交给请求。连接层在锁外用
//! [`Inbound::decode`] 解码每一帧（坏帧即 [`DisconnectReason::Decode`]），再交给它投递。

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use super::{Inbound, InboundError, Notification, Response};
use crate::frame::FrameError;

/// 拆掉连接的原因。所有挂起的请求以同一个原因（`Arc`）失败：调用方能做的只有重连，所以各种原因不分别报错。
/// 流连接与 GDP 连接的断开也用它表示。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// 本端关闭了连接（`Client` 被 drop）。
    #[error("closed by the client")]
    Closed,
    /// 服务端在帧边界上关闭了连接。
    #[error("closed by the server")]
    Eof,
    /// 读取失败、帧中途 EOF，或长度字段越界。
    #[error(transparent)]
    Read(#[from] FrameError),
    /// 写出失败或超时（10 s）：可能只写出了半帧，字节流已不可信。
    #[error("write failed: {0}")]
    Write(io::Error),
    /// 坏帧：服务端从不产生，字节流已不可信。
    #[error(transparent)]
    Decode(#[from] InboundError),
}

/// 一个请求的结果：响应，或拆连接的原因。
pub(super) type Reply = Result<Response, Arc<DisconnectReason>>;

/// 一个请求的应答接收端，由 [`ControlCore::register`] 返回。
///
/// 调用方超时即丢弃它：槽位成为墓碑，照样占住 FIFO 中的位置吸收迟到的响应，后面的请求仍拿到自己的响应。
pub(super) type ReplyRx = Receiver<Reply>;

/// 主连接的 FIFO 对应器，取代 C++ `GoRestClient` 的事务队列。
///
/// 线上不带请求 id：服务端每条连接一个 worker，按到达顺序处理请求，所以第 n 个响应属于第 n 个请求。调用方在写锁内、
/// 帧的第一个字节写出之前 [`register`](Self::register)，槽位顺序因此等于线上顺序（C++ SDK 先入队后发送，发送失败
/// 留下孤儿事务）。流结束与 `cancelStream` 竞争产生的孤立响应会让 FIFO 错位，所以流与文件传输各用自己的连接，
/// 主连接上没有它们；挂起队列为空时到达的响应不属于任何请求，与 C++ 一样丢弃。
#[derive(Debug, Default)]
pub(super) struct ControlCore {
    pending: VecDeque<SyncSender<Reply>>,
}

impl ControlCore {
    /// 登记即将写出的请求并返回它的应答接收端；必须在它的第一个字节写出之前调用。
    ///
    /// 通道在这里建立：容量 1，且每个槽位只发送一次（应答或 `fail_all`），所以读线程持锁 `send` 时从不阻塞。
    pub(super) fn register(&mut self) -> ReplyRx {
        let (tx, rx) = sync_channel(1);
        self.pending.push_back(tx);
        rx
    }

    /// 处理一条消息：响应交给队首的请求，通知返回给连接层按订阅投递；流消息（主连接不开流）与未知 `type` 忽略。
    pub(super) fn on_inbound(&mut self, inbound: Inbound) -> Option<Notification> {
        match inbound {
            Inbound::Response(response) => {
                if let Some(tx) = self.pending.pop_front() {
                    // 发送失败即墓碑：调用方已超时，响应被丢弃。
                    let _ = tx.send(Ok(response));
                }
                None
            }
            Inbound::Notification(notification) => Some(notification),
            Inbound::Stream(_) | Inbound::Other => None,
        }
    }

    /// 拆连接时让所有挂起的请求以同一个原因失败，清空队列。
    pub(super) fn fail_all(&mut self, reason: &Arc<DisconnectReason>) {
        for tx in self.pending.drain(..) {
            let _ = tx.send(Err(Arc::clone(reason)));
        }
    }
}

// ControlCore 是自有设计，无法与官方实现对照，出错时响应会静默交给错误的调用方。
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::TryRecvError;

    use bytes::Bytes;

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

    // FIFO 与墓碑：超时的请求丢弃接收端后，槽位仍吸收它的迟到响应，下一个请求拿到自己的响应；通知、流消息与未知
    // type 不占槽位；挂起队列为空时的响应丢弃。
    #[test]
    fn tombstone_absorbs_late_response() {
        let mut core = ControlCore::default();
        drop(core.register());
        let b = core.register();

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

    // 拆连接：fail_all 让每个挂起的请求恰好以同一个原因失败一次，并清空队列。
    #[test]
    fn fail_all_completes_every_slot_once() {
        let mut core = ControlCore::default();
        let receivers: Vec<_> = (0..3).map(|_| core.register()).collect();
        let reason = Arc::new(DisconnectReason::Closed);
        core.fail_all(&reason);
        for rx in &receivers {
            assert!(Arc::ptr_eq(&rx.try_recv().unwrap().unwrap_err(), &reason));
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
        }
    }
}
