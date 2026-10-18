//! 连接层的公共设施：四种连接（主连接、流连接、文件传输连接、GDP 连接）都经 [`connect`] 建立，帧经
//! [`read_frame`] 读取、[`write_all`] 写出；有读任务的连接与句柄共享一个 [`Link`]。

use std::io::{self, ErrorKind};
use std::net::{Shutdown, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::AbortHandle;
use tokio::time::timeout;

use crate::error::{DisconnectReason, Error, Result};
use crate::frame::{FrameError, FrameReader};

/// 写超时：与服务端的 10 s 写超时对称（协议假设 C1）。按每块 [`WRITE_CHUNK`] 计，即 10 s 内毫无进展才算超时，
/// 大文件上传只要求每块在 10 s 内写出。超时或只写出半帧后连接不再可用，由调用方拆掉。
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// 写出的分块，写超时按块计。
const WRITE_CHUNK: usize = 64 * 1024;

/// keepalive：空闲 30 s 后每 10 s 探测一次。只等通知或流数据的连接没有写出，断电的设备只能靠它发现。
const KEEPALIVE: TcpKeepalive = TcpKeepalive::new()
    .with_time(Duration::from_secs(30))
    .with_interval(Duration::from_secs(10));

/// 一帧的长度上限：64 位 1 GiB、32 位 256 MiB（GDP 消息不超过它是协议假设 G2）。长度字段在分配之前与它比较，坏帧
/// 不会让进程分配巨量内存。
pub(crate) const MAX_FRAME: u32 = if usize::BITS >= 64 {
    1 << 30
} else {
    256 << 20
};

/// 建立 TCP 连接（超时 `limit`）：`TCP_NODELAY`（请求是小帧，SDK 从不设置）与 keepalive；读不设超时，写超时见
/// [`write_all`]。端口 0 报 `InvalidArgument`（C++ 直接把 0 交给 socket）。
pub(crate) async fn connect(addr: SocketAddr, limit: Duration) -> Result<TcpStream> {
    if addr.port() == 0 {
        return Err(Error::InvalidArgument(format!("port of {addr} is 0")));
    }
    let setup = async {
        let stream = timeout(limit, TcpStream::connect(addr))
            .await
            .map_err(|_| io::Error::from(ErrorKind::TimedOut))??;
        stream.set_nodelay(true)?;
        SockRef::from(&stream).set_tcp_keepalive(&KEEPALIVE)?;
        Ok::<_, io::Error>(stream)
    };
    setup
        .await
        .map_err(|source| Error::Connect { addr, source })
}

/// 整帧写出，每块 [`WRITE_CHUNK`] 带 10 s 超时（tokio 的 socket 没有 `SO_SNDTIMEO`），超时报 `TimedOut`。
pub(crate) async fn write_all<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
) -> io::Result<()> {
    for chunk in frame.chunks(WRITE_CHUNK) {
        timeout(WRITE_TIMEOUT, writer.write_all(chunk))
            .await
            .map_err(|_| io::Error::from(ErrorKind::TimedOut))??;
    }
    Ok(())
}

/// 读下一帧。`idle` 是连续没有收到任何字节的时长上限（`Duration::MAX` 即不限），超时报 `Io(TimedOut)`；帧边界上的
/// EOF 为 `Ok(None)`，帧中途 EOF 报 `Io(UnexpectedEof)`。
pub(crate) async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    frames: &mut FrameReader,
    idle: Duration,
) -> std::result::Result<Option<Bytes>, FrameError> {
    loop {
        if let Some(frame) = frames.next_frame()? {
            return Ok(Some(frame));
        }
        let n = timeout(idle, reader.read_buf(frames.buffer()))
            .await
            .map_err(|_| io::Error::from(ErrorKind::TimedOut))??;
        if n == 0 {
            return if frames.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::from(ErrorKind::UnexpectedEof).into())
            };
        }
    }
}

/// 读任务与句柄共享的连接状态。
///
/// 持有连接的句柄（`Connection`、`StreamHandle`、`gdp::Client`）Drop 时 [`close`](Self::close)：记下原因、shutdown
/// （对端立即收到 FIN，不等别处释放写半边）并中止读任务，读任务里的半个 socket 随之释放，所以 Drop 从不阻塞、不留
/// 任务，读任务卡在半帧中时也一样。读任务自己结束时用 [`finish`](Self::finish) 记下原因，句柄之后的调用都报这个原因。
#[derive(Debug)]
pub(crate) struct Link {
    /// 同一个 socket 的另一个句柄，只用来 shutdown：tokio 的读写两半由读任务与连接的其他部分持有，这里不碰它们。
    socket: std::net::TcpStream,
    reason: OnceLock<Arc<DisconnectReason>>,
    reader: OnceLock<AbortHandle>,
}

impl Link {
    pub(crate) fn new(stream: &TcpStream) -> io::Result<Self> {
        Ok(Self {
            socket: SockRef::from(stream).try_clone()?.into(),
            reason: OnceLock::new(),
            reader: OnceLock::new(),
        })
    }

    /// 登记读任务，[`finish`](Self::finish) 时中止它。
    pub(crate) fn set_reader(&self, reader: AbortHandle) {
        let _ = self.reader.set(reader);
    }

    /// 记下断开原因、shutdown 并中止读任务；只有第一次调用生效并返回记下的原因，之后的调用返回 `None`。
    pub(crate) fn finish(&self, reason: DisconnectReason) -> Option<&Arc<DisconnectReason>> {
        self.reason.set(Arc::new(reason)).ok()?;
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.get() {
            reader.abort();
        }
        self.reason.get()
    }

    /// 持有连接的句柄 Drop 时以 `Closed` 断开。
    pub(crate) fn close(&self) {
        self.finish(DisconnectReason::Closed);
    }

    /// 断开原因；尚未断开时为 `None`。
    pub(crate) fn reason(&self) -> Option<&Arc<DisconnectReason>> {
        self.reason.get()
    }

    /// 已断开时的原因；只在确认已断开之后调用（通道关闭、`is_connected` 为假），否则按 `Closed` 处理。
    pub(crate) fn disconnected(&self) -> Arc<DisconnectReason> {
        let reason = self.reason().cloned();
        reason.unwrap_or_else(|| Arc::new(DisconnectReason::Closed))
    }
}

/// 取锁并忽略中毒：临界区内不调用用户代码、不跨 await，也没有会 panic 的操作，数据不会处于半更新状态。
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
