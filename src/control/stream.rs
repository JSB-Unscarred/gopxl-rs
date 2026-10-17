//! 流：每个流一条自有连接，对应 C++ 的 `StartStream` / `StopStream` / `SetStreamHandler`。
//!
//! [`Handle::open_stream`] 对同一台设备再开一条控制连接，只写出一个 `stream` 请求；之后读线程 `gopxl-stream` 把
//! 流消息放进有界通道，[`StreamHandle`] Drop 即关闭连接（服务端在连接关闭时清理流），从不发 `cancelStream`。
//! 这样主连接上没有流：流结束与 `cancelStream` 竞争产生的孤立响应、`removeStream` / `streamStart` 失败的孤立
//! 响应都只会出现在这条连接上，应答之后的响应一律忽略；C++ 把它们当作下一个请求的应答，此后全部错位（重写方案
//! §5.4 第 18、19、42 条）。

use std::convert::Infallible;
use std::io::{BufReader, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::Duration;

use super::client::{CONNECT_TIMEOUT, Handle, encode, with_payload};
use super::{DisconnectReason, HEADER_LEN, Inbound, Method, Response, StreamMessage};
use crate::error::{self, Error, Result};
use crate::frame::FrameReader;
use crate::net::{self, Link, MAX_FRAME};
use crate::value::Value;

/// 等待 `stream` 应答的超时。
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// 流消息通道的容量。消费者跟不上时读线程阻塞，TCP 背压传到服务端（服务端写超时 10 s 后断开连接，
/// [`StreamHandle::recv_timeout`] 返回 `Disconnected`，协议假设 T3）；容量小，`/dataSources` 的大消息不会在本地积压。
const QUEUE: usize = 4;

/// 一个流，拥有自己的连接与读线程；owned、`Send`，可以移到任何线程。Drop 即关闭连接（C++ 的 `StopStream`），
/// 不 join 读线程（见 `Link`）。
#[derive(Debug)]
pub struct StreamHandle {
    link: Arc<Link>,
    messages: Receiver<StreamMessage>,
}

impl Handle {
    /// 在新连接上开始流 `path`（例如 `/system/metrics`、`/dataSources/<id>/data`），`payload` 原样放进请求
    /// （例如 `{"period": 微秒}`），等待应答（5 s）。应答的 `status` 不是 OK 时报 `Request`。每个流一条连接是协议
    /// 假设 T1。
    pub fn open_stream(&self, path: &str, payload: impl Into<Value>) -> Result<StreamHandle> {
        let request = with_payload(Method::Stream, path, payload);
        let frame = encode(&request)?;
        let socket = net::connect(self.addr(), CONNECT_TIMEOUT)?;
        // 写出失败时 socket 随返回关闭。
        (&socket)
            .write_all(&frame)
            .map_err(|e| Error::Disconnected(Arc::new(DisconnectReason::Write(e))))?;
        let link = Arc::new(Link::new(socket));
        let (reply_tx, reply) = sync_channel(1);
        let (message_tx, messages) = sync_channel(QUEUE);
        let reader = Arc::clone(&link);
        thread::Builder::new()
            .name("gopxl-stream".into())
            .spawn(move || {
                let Err(reason) = read_stream(&reader, &reply_tx, &message_tx);
                // 先记下原因再丢弃发送端，句柄看到通道断开时总能取到原因。
                reader.finish(reason);
            })
            .map_err(|source| Error::Connect {
                addr: self.addr(),
                source,
            })?;
        // 从这里起任何错误都经 Drop 关闭连接。
        let stream = StreamHandle { link, messages };
        match reply.recv_timeout(OPEN_TIMEOUT) {
            Ok(response) => error::check(request.method, request.path, response)?,
            Err(RecvTimeoutError::Timeout) => {
                return Err(Error::Timeout {
                    method: request.method,
                    path: request.path,
                    after: OPEN_TIMEOUT,
                });
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::Disconnected(stream.link.disconnected()));
            }
        };
        Ok(stream)
    }
}

impl StreamHandle {
    /// 取出下一条流消息，最多等待 `timeout`；超时返回 `Ok(None)`。连接断开后（已取完通道里的消息）返回
    /// `Err(Disconnected)`。
    ///
    /// 结束消息（`state` 为 `Complete` / `Abort`，`status` 为结束原因）照常取出，之后服务端删除流、连接保持打开：
    /// 调用方据此停止接收并 drop 句柄。`/dataSources` 的数据可能紧跟在结束消息之后（服务端每轮先写响应队列、再写
    /// 流队列），需要时在结束消息之后再取一会儿。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<StreamMessage>> {
        match self.messages.recv_timeout(timeout) {
            Ok(message) => Ok(Some(message)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(Error::Disconnected(self.link.disconnected()))
            }
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.link.shutdown();
    }
}

/// 读线程：应答交给 `reply`，流消息交给 `messages`，直到出错、EOF 或句柄被丢弃（发送失败，返回 `Closed`）。
///
/// `/dataSources` 的数据走服务端的另一个队列，可能先于应答到达：先缓存，应答发出之后再按顺序放进通道，所以句柄
/// 返回时它们排在最前面。应答之后的响应是孤立响应，与通知、未知 `type` 一起忽略。
fn read_stream(
    link: &Link,
    reply: &SyncSender<Response>,
    messages: &SyncSender<StreamMessage>,
) -> std::result::Result<Infallible, DisconnectReason> {
    let mut frames = FrameReader::new(HEADER_LEN, MAX_FRAME);
    // metrics 是小帧：每帧至少两次 read，经缓冲合并；大帧的帧体绕过缓冲直接读入。
    let mut socket = BufReader::new(&link.socket);
    // 应答之前到达的流消息；`None` 表示已经应答。
    let mut early = Some(Vec::new());
    loop {
        let frame = frames.read(&mut socket)?.ok_or(DisconnectReason::Eof)?;
        match Inbound::decode(frame)? {
            Inbound::Response(response) => {
                if let Some(early) = early.take() {
                    reply.send(response).map_err(|_| DisconnectReason::Closed)?;
                    for message in early {
                        messages
                            .send(message)
                            .map_err(|_| DisconnectReason::Closed)?;
                    }
                }
            }
            Inbound::Stream(message) => match &mut early {
                Some(early) => early.push(message),
                None => messages
                    .send(message)
                    .map_err(|_| DisconnectReason::Closed)?,
            },
            Inbound::Notification(_) | Inbound::Other(_) => {}
        }
    }
}
