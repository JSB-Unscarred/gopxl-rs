//! GDP 接收：[`GdpClient`] 拥有一条只读的 TCP 连接，读线程 `gopxl-gdp` 逐条解码消息，`receive` 在调用方线程上
//! 按 `is_last_msg` 组集。对应 C++ 的 `GoGdpClient`。
//!
//! 读线程与消费者之间是容量很小的有界通道：不调用 `receive` 时读线程阻塞，TCP 背压传到服务端，由服务端丢弃数据
//! （计入 `controlDrops`），与 C++ 的同步接收一样本地不积压。C++ 的异步队列无界。
//! socket 不设读超时：Windows 上 `SO_RCVTIMEO` 超时之后 socket 状态不确定，可能丢失字节，GDP 流随之错位。

use std::convert::Infallible;
use std::io::BufReader;
use std::mem;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use super::{HEADER_LEN, Message, decode_message};
use crate::control::DisconnectReason;
use crate::error::{Error, Result};
use crate::frame::FrameReader;
use crate::net::{self, Link, MAX_FRAME};

/// 建立 GDP 连接的超时（SDK 默认值）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 消息通道的容量：读线程解码下一条时消费者处理上一条；一条表面数据可达十几 MB，容量小即本地内存小。
const QUEUE: usize = 4;

/// [`GdpClient::receive`] 的错误。
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

/// GDP 客户端，拥有连接与读线程；owned、`Send`，可以移到用户自己的线程里循环 `receive`（C++ 的
/// `ReceiveDataAsync`）。Drop 即关闭连接，不 join 读线程（见 `Link`）。从不写 socket。
///
/// GDP 服务端从不读取，不会因为本端关闭而关闭连接：Windows 上传感器空闲时 drop 或 `clear_data`，旧的读线程与
/// socket 留到服务端推送下一条数据时才释放（已 shutdown 的 socket 收到数据即重置连接）。服务端一侧与 C++ 关闭
/// socket 时相同，都在下一次写出时才发现连接已关闭。
#[derive(Debug)]
pub struct GdpClient {
    addr: SocketAddr,
    link: Arc<Link>,
    messages: Receiver<Message>,
    /// 当前数据集已收到的消息。
    partial: Vec<Message>,
}

impl GdpClient {
    /// 连接 GDP 服务端（超时 5 s），启动读线程。
    pub fn connect(addr: SocketAddr) -> Result<Self> {
        let link = Arc::new(Link::new(net::connect(addr, CONNECT_TIMEOUT)?));
        let (tx, messages) = sync_channel(QUEUE);
        let reader = Arc::clone(&link);
        thread::Builder::new()
            .name("gopxl-gdp".into())
            .spawn(move || {
                let Err(reason) = read_messages(&reader, &tx);
                reader.finish(reason);
            })
            .map_err(|source| Error::Connect { addr, source })?;
        Ok(Self {
            addr,
            link,
            messages,
            partial: Vec::new(),
        })
    }

    /// 接收一个完整的数据集（以 `is_last_msg` 的消息结束），最多等待 `timeout`。对应 C++ 的 `ReceiveDataSync` 加
    /// `DataSet()`；`GoDataSet` 即返回的 `Vec`。`Duration::MAX` 即一直等待。
    pub fn receive(&mut self, timeout: Duration) -> Result<Vec<Message>, RecvError> {
        let start = Instant::now();
        loop {
            match self
                .messages
                .recv_timeout(timeout.saturating_sub(start.elapsed()))
            {
                Ok(message) => {
                    let last = message.cas.is_last_msg;
                    self.partial.push(message);
                    if last {
                        return Ok(mem::take(&mut self.partial));
                    }
                }
                Err(RecvTimeoutError::Timeout) if self.partial.is_empty() => {
                    return Err(RecvError::Timeout);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(RecvError::Incomplete {
                        messages: self.partial.len(),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RecvError::Disconnected(self.link.disconnected()));
                }
            }
        }
    }

    /// 重新连接，丢弃服务端与本地积压的数据和未完成的数据集，对应 C++ 的 `ClearData`。失败时保留原连接。
    pub fn clear_data(&mut self) -> Result<()> {
        *self = Self::connect(self.addr)?;
        Ok(())
    }
}

impl Drop for GdpClient {
    fn drop(&mut self) {
        self.link.shutdown();
    }
}

/// 读线程：逐条读取、解码并放进通道，直到出错、EOF 或句柄被丢弃（发送失败，返回 `Closed`）。
///
/// 信封或 CAS 坏掉的消息丢弃（无法知道它是否结束数据集），连接照常读下去：GDP 消息自带长度，坏消息不影响后面的帧。
fn read_messages(
    link: &Link,
    messages: &SyncSender<Message>,
) -> std::result::Result<Infallible, DisconnectReason> {
    let mut frames = FrameReader::new(HEADER_LEN, MAX_FRAME);
    let mut socket = BufReader::new(&link.socket);
    loop {
        let frame = frames.read(&mut socket)?.ok_or(DisconnectReason::Eof)?;
        if let Ok(message) = decode_message(frame) {
            messages
                .send(message)
                .map_err(|_| DisconnectReason::Closed)?;
        }
    }
}
