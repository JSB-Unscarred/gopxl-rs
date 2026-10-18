//! 流：每个流一条自有连接。
//!
//! [`Client::open_stream`] 对同一台设备再开一条控制连接，只写出一个 `stream` 请求；之后读任务把流消息放进有界
//! 通道。为什么每个流一条连接见 [`StreamHandle`]。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use super::client::{CONNECT_TIMEOUT, Client, await_reply, encode};
use super::{HEADER_LEN, Inbound, Method, Reply, Request, StreamMessage};
use crate::error::{DisconnectReason, Error, Result};
use crate::frame::FrameReader;
use crate::net::{self, Link, MAX_FRAME};

/// 等待 `stream` 应答的超时。
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// 流消息通道的容量。消费者跟不上时读任务暂停读取，服务端随之写不出（服务端写超时 10 s 后断开连接，
/// [`StreamHandle::recv`] 返回 `Disconnected`，见[协议假设 T3](crate::docs::assumptions)）；容量小，`/dataSources`
/// 的大消息不会在本地积压。
const QUEUE: usize = 4;

/// 一个流，拥有自己的连接与读任务，可以移到任何任务。Drop 即关闭连接并中止读任务，从不发 `cancelStream`：
/// 服务端在连接关闭时清理流。
///
/// 每个流一条连接，因为流的应答之后还可能出现不属于任何请求的响应：流自行结束与 `cancelStream` 同时发生时，
/// `cancelStream` 会得到一个多余的响应；服务端的 `removeStream` / `streamStart` 失败时也会多发一个响应。在主连接
/// 上它们会被当作下一个请求的应答，此后响应与请求全部错配；在流自己的连接上，应答之后的响应一律忽略。
#[derive(Debug)]
pub struct StreamHandle {
    link: Arc<Link>,
    messages: mpsc::Receiver<StreamMessage>,
}

impl Client {
    /// 在新连接上开始流 `path`（例如 `/system/metrics`、`/dataSources/<id>/data`），`payload` 原样放进请求
    /// （例如 `{"period": 微秒}`），等待应答（5 s）。应答的 `status` 不是 OK 时报 `Request`。服务端允许每个流
    /// 一条连接是[协议假设 T1](crate::docs::assumptions)。
    pub async fn open_stream(&self, path: &str, payload: impl Into<Value>) -> Result<StreamHandle> {
        let request = Request::with_payload(Method::Stream, path, payload);
        let frame = encode(&request)?;
        let stream = net::connect(self.addr(), CONNECT_TIMEOUT).await?;
        let link = Link::new(&stream).map_err(|source| Error::Connect {
            addr: self.addr(),
            source,
        })?;
        let (reader, mut writer) = stream.into_split();
        // 写出失败时 socket 随返回关闭。
        net::write_all(&mut writer, &frame)
            .await
            .map_err(|e| Error::Disconnected(Arc::new(DisconnectReason::Write(e))))?;
        let link = Arc::new(link);
        let (reply_tx, reply) = oneshot::channel();
        let (message_tx, messages) = mpsc::channel(QUEUE);
        let task = tokio::spawn({
            let link = Arc::clone(&link);
            async move {
                // 写半边随任务存活：丢弃它会 shutdown 写方向，服务端读到 FIN 即清理流。
                let _writer = writer;
                let mut reply = Some(reply_tx);
                let Err(reason) = read_stream(reader, &mut reply, &message_tx).await;
                // 应答之前就断开时让 open_stream 拿到原因。
                if let (Some(reason), Some(reply)) = (link.finish(reason), reply) {
                    let _ = reply.send(Err(Arc::clone(reason)));
                }
            }
        });
        link.set_reader(task.abort_handle());
        // 从这里起任何错误都经 Drop 关闭连接。
        let handle = StreamHandle { link, messages };
        await_reply(reply, OPEN_TIMEOUT, request.method, request.path).await?;
        Ok(handle)
    }
}

impl StreamHandle {
    /// 取出下一条流消息，一直等待。连接断开后（已取完通道里的消息）返回 `Err(Disconnected)`。
    ///
    /// 结束消息（`state` 为 `Complete` / `Abort`，`status` 为结束原因）照常取出，之后服务端删除流，连接仍然打开：
    /// 调用方据此停止接收并 drop 句柄。`/dataSources` 的数据可能紧跟在结束消息之后（服务端每轮先写响应队列、再写
    /// 流队列），需要时在结束消息之后再取一会儿。
    pub async fn recv(&mut self) -> Result<StreamMessage> {
        self.messages
            .recv()
            .await
            .ok_or_else(|| Error::Disconnected(self.link.disconnected()))
    }

    /// 取出下一条流消息，最多等待 `limit`；超时返回 `Ok(None)`。
    pub async fn recv_timeout(&mut self, limit: Duration) -> Result<Option<StreamMessage>> {
        match timeout(limit, self.messages.recv()).await {
            Ok(Some(message)) => Ok(Some(message)),
            Ok(None) => Err(Error::Disconnected(self.link.disconnected())),
            Err(_) => Ok(None),
        }
    }

    /// 取出已到达的下一条流消息，不等待。
    pub fn try_recv(&mut self) -> Result<Option<StreamMessage>> {
        match self.messages.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(Error::Disconnected(self.link.disconnected()))
            }
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.link.close();
    }
}

/// 读任务：应答交给 `reply`，流消息交给 `messages`，直到出错、EOF 或句柄被丢弃（发送失败，返回 `Closed`）。
///
/// `/dataSources` 的数据走服务端的另一个队列，可能先于应答到达：先缓存，应答发出之后再按顺序放进通道，所以句柄
/// 返回时它们排在最前面。应答之后的响应不属于任何请求，与通知、未知 `type` 一起忽略。
async fn read_stream(
    mut reader: OwnedReadHalf,
    reply: &mut Option<oneshot::Sender<Reply>>,
    messages: &mpsc::Sender<StreamMessage>,
) -> std::result::Result<Infallible, DisconnectReason> {
    let mut frames = FrameReader::new(HEADER_LEN, MAX_FRAME);
    // 应答之前到达的流消息；`None` 表示已经应答。
    let mut early = Some(Vec::new());
    loop {
        let frame = net::read_frame(&mut reader, &mut frames, Duration::MAX)
            .await?
            .ok_or(DisconnectReason::Eof)?;
        match Inbound::decode(frame)? {
            Inbound::Response(response) => {
                if let (Some(early), Some(reply)) = (early.take(), reply.take()) {
                    reply
                        .send(Ok(response))
                        .map_err(|_| DisconnectReason::Closed)?;
                    for message in early {
                        messages
                            .send(message)
                            .await
                            .map_err(|_| DisconnectReason::Closed)?;
                    }
                }
            }
            Inbound::Stream(message) => match &mut early {
                Some(early) => early.push(message),
                None => messages
                    .send(message)
                    .await
                    .map_err(|_| DisconnectReason::Closed)?,
            },
            Inbound::Notification(_) | Inbound::Other => {}
        }
    }
}

use crate::value::Value;
