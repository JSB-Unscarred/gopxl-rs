//! 连接层的公共设施：控制通道的三种连接（主连接、流连接、文件传输连接）与 GDP 连接都经 [`connect`] 建立；
//! 有读线程的连接与句柄共享一个 [`Link`]。

use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};

use crate::control::DisconnectReason;
use crate::error::{Error, Result};

/// 写超时：与服务端的 10 s 写超时对称（协议假设 C1）。超时或只写出半帧后连接不再可用，由调用方拆掉。
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

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

/// 建立 TCP 连接：`TCP_NODELAY`（请求是小帧，SDK 从不设置）、keepalive 与写超时；读不设超时。
/// 端口 0 报 `InvalidArgument`（C++ 直接把 0 交给 socket）。
pub(crate) fn connect(addr: SocketAddr, timeout: Duration) -> Result<TcpStream> {
    if addr.port() == 0 {
        return Err(Error::InvalidArgument(format!("port of {addr} is 0")));
    }
    let setup = || {
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        SockRef::from(&stream).set_tcp_keepalive(&KEEPALIVE)?;
        Ok(stream)
    };
    setup().map_err(|source| Error::Connect { addr, source })
}

/// 读线程与句柄共享的连接。
///
/// 句柄 Drop 时 [`shutdown`](Self::shutdown)：之后的读写立即失败，对端收到 FIN。阻塞在 recv 上的读线程在 Linux
/// 上随即返回；Windows 上 shutdown 唤不醒它，要等对端关闭连接或再发数据（控制通道的服务端读到 EOF 即关闭连接，
/// 协议假设 C2），读线程这时退出并释放 socket。句柄不 join，所以 Drop 从不阻塞（C++ 先 join 后 shutdown，读线程卡在
/// 半帧中时永远挂住）。读线程退出前用 [`finish`](Self::finish) 记下原因，再丢弃通道的发送端，
/// 句柄看到通道断开时总能取到原因。
#[derive(Debug)]
pub(crate) struct Link {
    pub(crate) socket: TcpStream,
    reason: OnceLock<Arc<DisconnectReason>>,
}

impl Link {
    pub(crate) fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            reason: OnceLock::new(),
        }
    }

    /// 记下断开原因并 shutdown；只有第一次调用生效，并返回记下的原因。
    pub(crate) fn finish(&self, reason: DisconnectReason) -> Option<&Arc<DisconnectReason>> {
        self.reason.set(Arc::new(reason)).ok()?;
        self.shutdown();
        self.reason.get()
    }

    /// 断开原因；尚未断开时为 `None`。
    pub(crate) fn reason(&self) -> Option<&Arc<DisconnectReason>> {
        self.reason.get()
    }

    /// 已断开时的原因；只在确认已断开之后调用（通道断开、`is_connected` 为假），否则按 `Closed` 处理。
    pub(crate) fn disconnected(&self) -> Arc<DisconnectReason> {
        let reason = self.reason().cloned();
        reason.unwrap_or_else(|| Arc::new(DisconnectReason::Closed))
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

/// 取锁并忽略中毒：临界区内不调用用户代码，也没有会 panic 的操作，数据不会处于半更新状态。
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
