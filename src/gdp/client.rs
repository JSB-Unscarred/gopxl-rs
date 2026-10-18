//! GDP 接收：[`Client`] 拥有一条只读的 TCP 连接，读任务逐条解码消息，`receive` 在调用方的任务上按 `is_last_msg`
//! 组集。对应 C++ 的 `GoGdpClient`。
//!
//! 读任务与消费者之间是容量很小的有界通道：不调用 `receive` 时读任务等待，TCP 背压传到服务端，由服务端丢弃数据
//! （计入 `controlDrops`），与 C++ 的同步接收一样本地不积压。C++ 的异步队列无界。

use std::convert::Infallible;
use std::mem;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

use super::{HEADER_LEN, Message, decode_message};
use crate::error::{DisconnectReason, Error, Result};
use crate::frame::FrameReader;
use crate::net::{self, Link, MAX_FRAME};

/// 建立 GDP 连接的超时（SDK 默认值）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 消息通道的容量：读任务解码下一条时消费者处理上一条；一条表面数据可达十几 MB，容量小即本地内存小。
const QUEUE: usize = 4;

/// [`Client::receive`] 的错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RecvError {
    /// 期限内没有收到任何消息。
    #[error("no GDP message within the timeout")]
    Timeout,
    /// 期限内收到了消息但数据集未结束；已收到的消息保留，下一次 `receive` 接着组装。
    #[error("GDP data set incomplete after {messages} messages")]
    Incomplete {
        /// 当前数据集已收到的消息数。
        messages: usize,
    },
    /// 连接已断开（已取完断开之前收到的消息）；未结束的数据集不返回。
    #[error("GDP connection lost: {0}")]
    Disconnected(Arc<DisconnectReason>),
}

/// GDP 客户端，拥有连接与读任务；owned、`Send`，可以移到自己的任务里循环 `receive`（C++ 的
/// `ReceiveDataAsync`）。Drop 即关闭连接并中止读任务（见 `Link`）。从不写 socket。
#[derive(Debug)]
pub struct Client {
    addr: SocketAddr,
    link: Arc<Link>,
    messages: mpsc::Receiver<Message>,
    /// 当前数据集已收到的消息。
    partial: Vec<Message>,
}

impl Client {
    /// 连接 GDP 服务端（超时 5 s），在当前 tokio 运行时上启动读任务。
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = net::connect(addr, CONNECT_TIMEOUT).await?;
        let link = Link::new(&stream).map_err(|source| Error::Connect { addr, source })?;
        let link = Arc::new(link);
        let (tx, messages) = mpsc::channel(QUEUE);
        let task = tokio::spawn({
            let link = Arc::clone(&link);
            async move {
                let Err(reason) = read_messages(stream, &tx).await;
                link.finish(reason);
            }
        });
        link.set_reader(task.abort_handle());
        Ok(Self {
            addr,
            link,
            messages,
            partial: Vec::new(),
        })
    }

    /// 接收一个完整的数据集（以 `is_last_msg` 的消息结束），最多等待 `limit`。对应 C++ 的 `ReceiveDataSync` 加
    /// `DataSet()`；`GoDataSet` 即返回的 `Vec`。`Duration::MAX` 即一直等待。
    pub async fn receive(&mut self, limit: Duration) -> Result<Vec<Message>, RecvError> {
        let start = Instant::now();
        loop {
            let remaining = limit.saturating_sub(start.elapsed());
            match timeout(remaining, self.messages.recv()).await {
                Ok(Some(message)) => {
                    let last = message.cas.is_last_msg;
                    self.partial.push(message);
                    if last {
                        return Ok(mem::take(&mut self.partial));
                    }
                }
                Ok(None) => return Err(RecvError::Disconnected(self.link.disconnected())),
                Err(_) if self.partial.is_empty() => return Err(RecvError::Timeout),
                Err(_) => {
                    return Err(RecvError::Incomplete {
                        messages: self.partial.len(),
                    });
                }
            }
        }
    }

    /// 重新连接，丢弃服务端与本地积压的数据和未完成的数据集，对应 C++ 的 `ClearData`。失败时保留原连接。
    pub async fn clear_data(&mut self) -> Result<()> {
        *self = Self::connect(self.addr).await?;
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.link.close();
    }
}

/// 读任务：逐条读取、解码并放进通道，直到出错、EOF 或句柄被丢弃（发送失败，返回 `Closed`）。
///
/// 信封或 CAS 坏掉的消息丢弃（无法知道它是否结束数据集），连接照常读下去：GDP 消息自带长度，坏消息不影响后面的帧。
async fn read_messages(
    mut stream: TcpStream,
    messages: &mpsc::Sender<Message>,
) -> std::result::Result<Infallible, DisconnectReason> {
    let mut frames = FrameReader::new(HEADER_LEN, MAX_FRAME);
    loop {
        let frame = net::read_frame(&mut stream, &mut frames, Duration::MAX)
            .await?
            .ok_or(DisconnectReason::Eof)?;
        if let Ok(message) = decode_message(frame) {
            messages
                .send(message)
                .await
                .map_err(|_| DisconnectReason::Closed)?;
        }
    }
}
