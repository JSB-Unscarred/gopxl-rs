//! 主控制连接：[`Client`] 拥有连接，[`Handle`] 发请求，[`Transaction`] 等应答。对应 C++ 的 `GoRestClient`。
//!
//! 一条连接一个读线程，请求在调用方线程上写出：
//! - 调用方在写锁内先 [`ControlCore::register`] 再写出整帧，槽位顺序因此等于线上顺序；写出失败或超时（10 s）
//!   可能只写出了半帧，立即拆连接；
//! - 读线程 `gopxl-control` 无超时地阻塞读帧，在锁外解码，再取分派锁交给 [`ControlCore`]，通知放进订阅队列；
//! - 锁的顺序固定为写锁 → 分派锁。读线程只取分派锁：服务端停止读取、写出被阻塞时读线程照常读，否则双方都在写、
//!   都不读；读线程也从不等待消费者（订阅队列满了丢最老的），服务端的 10 s 写超时会断开读得慢的客户端。
//!
//! 拆连接（[`Shared::teardown`]）只执行一次：记下原因、shutdown、所有挂起的请求以同一个原因失败、关闭订阅队列。
//! 读错误、EOF、坏帧、写失败与 `Client` 的 Drop 都走这里，不依赖读线程醒来（见 `Link`）；之后
//! [`Handle::is_connected`] 为假，每个阻塞调用都返回 `Disconnected(原因)`，不另设断开回调。C++ 读错误时不让挂起的
//! 请求失败，EOF 时忙等，`IsConnected` 一直为真。

use std::convert::Infallible;
use std::fmt;
use std::io::{BufReader, Write};
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::Duration;

use super::subscription::{self, SubQueue};
use super::{
    ControlCore, DisconnectReason, HEADER_LEN, Inbound, Method, ReplyRx, Request, Response,
};
use crate::error::{self, Error, Result};
use crate::frame::FrameReader;
use crate::net::{self, Link, MAX_FRAME, lock};
use crate::value::Value;

/// 建立控制连接的超时（SDK 默认值）。
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// 请求等待应答的默认超时（SDK 的 `CheckResponse(3000)`）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// 主控制连接的唯一 owner，对应 C++ 的 `GoRestClient` / `GoSystem` 的连接部分。
///
/// 不 `Clone`；Drop 即断开（C++ 的 `Disconnect`）：拆连接、所有挂起的请求以 [`DisconnectReason::Closed`] 失败，
/// 不触发断开回调；不 join 读线程（见 `Link`）。另一线程正阻塞在写出上时，它最多再等 10 s 写超时。经 [`Deref`]
/// 直接调用 [`Handle`] 的方法。
#[derive(Debug)]
pub struct Client {
    handle: Handle,
}

impl Client {
    /// 连接 `addr`（超时 1 s），启动读线程。
    pub fn connect(addr: SocketAddr) -> Result<Self> {
        let shared = Arc::new(Shared {
            addr,
            link: Link::new(net::connect(addr, CONNECT_TIMEOUT)?),
            writer: Mutex::default(),
            dispatch: Mutex::default(),
            writes: AtomicU64::new(0),
        });
        let reader = Arc::clone(&shared);
        thread::Builder::new()
            .name("gopxl-control".into())
            .spawn(move || {
                let Err(reason) = reader.read_frames();
                reader.teardown(reason);
            })
            .map_err(|source| Error::Connect { addr, source })?;
        Ok(Self {
            handle: Handle { shared },
        })
    }

    /// 一个新的 [`Handle`]，可以移到别的线程或存进别的对象。
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

impl Deref for Client {
    type Target = Handle;

    fn deref(&self) -> &Handle {
        &self.handle
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.handle.shared.teardown(DisconnectReason::Closed);
    }
}

/// 主连接的 owned 句柄：发请求、订阅、开流，不拥有连接。`Client` Drop 之后，经它发出的请求立即以
/// `Disconnected(Closed)` 失败。
#[derive(Clone)]
pub struct Handle {
    pub(super) shared: Arc<Shared>,
}

/// 连接的共享状态，由 `Handle`（经 `Arc`）与读线程共同持有。
pub(super) struct Shared {
    addr: SocketAddr,
    pub(super) link: Link,
    /// 写锁：整帧写出期间持有，保证帧不交错、槽位顺序等于线上顺序；订阅的登记与注销也在它里面。
    pub(super) writer: Mutex<()>,
    /// 分派锁：读线程处理每一帧时持有。
    pub(super) dispatch: Mutex<Dispatch>,
    /// 经这条连接发出的变更请求数，见 [`Handle::write_count`]。
    writes: AtomicU64,
}

/// 分派锁保护的状态：读线程按帧更新 FIFO 对应器与订阅队列。
#[derive(Default)]
pub(super) struct Dispatch {
    core: ControlCore,
    /// 本地订阅的队列；`Subscription` 拥有队列，这里只持 `Weak`，Drop 时自行移除。
    pub(super) subs: Vec<Weak<SubQueue>>,
}

impl Shared {
    /// 读线程：读帧、解码、分派，直到出错或 EOF，返回拆连接的原因。
    fn read_frames(&self) -> std::result::Result<Infallible, DisconnectReason> {
        let mut frames = FrameReader::new(HEADER_LEN, MAX_FRAME);
        let mut socket = BufReader::new(&self.link.socket);
        loop {
            let frame = frames.read(&mut socket)?.ok_or(DisconnectReason::Eof)?;
            let inbound = Inbound::decode(frame)?;
            let mut dispatch = lock(&self.dispatch);
            if let Some(notification) = dispatch.core.on_inbound(inbound) {
                subscription::deliver(&dispatch.subs, notification);
            }
        }
    }

    /// 拆连接，只有第一次调用生效。shutdown 在 `fail_all` 之前：之后登记的槽位，其写出必然失败。
    pub(super) fn teardown(&self, reason: DisconnectReason) {
        let Some(reason) = self.link.finish(reason) else {
            return;
        };
        let mut dispatch = lock(&self.dispatch);
        dispatch.core.fail_all(reason);
        subscription::close_all(&dispatch.subs);
    }

    /// 可能改变服务端状态的请求（与 C++ 触发 `NonIdempotentRequestHandler` 的方法相同）计入写计数。
    pub(super) fn count_write(&self, method: Method) {
        if matches!(
            method,
            Method::Create | Method::Update | Method::Delete | Method::Call
        ) {
            self.writes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 登记并写出一帧。`_writer` 证明调用方持有写锁。
    pub(super) fn write(
        &self,
        _writer: &MutexGuard<'_, ()>,
        request: &Request,
        frame: &[u8],
    ) -> Result<ReplyRx> {
        if let Some(reason) = self.link.reason() {
            return Err(Error::Disconnected(Arc::clone(reason)));
        }
        let reply = lock(&self.dispatch).core.register();
        // 在写锁内计数，见 `Handle::write_count`。
        self.count_write(request.method);
        if let Err(error) = (&self.link.socket).write_all(frame) {
            self.teardown(DisconnectReason::Write(error));
            return Err(Error::Disconnected(self.link.disconnected()));
        }
        Ok(reply)
    }
}

/// 把请求编码为整帧；超过 4 GiB 时报 `InvalidArgument`，此时尚未写出任何字节，连接不受影响。
pub(super) fn encode(request: &Request) -> Result<Vec<u8>> {
    let mut frame = Vec::new();
    request.encode(&mut frame).map_err(|_| {
        Error::InvalidArgument(format!(
            "{} {} exceeds 4 GiB",
            request.method.as_str(),
            request.path
        ))
    })?;
    Ok(frame)
}

impl Handle {
    /// 写出任意请求，返回等待应答的 [`Transaction`]。
    ///
    /// 写出失败时返回的 `Transaction` 立即以 `Disconnected` 完成，调用方只在 [`Transaction::response`] 处处理错误。
    /// `stream` 与 `cancelStream` 以 `InvalidArgument` 完成：流在自己的连接上（[`open_stream`](Self::open_stream)），
    /// 主连接上流的结束与 `cancelStream` 竞争会产生孤立响应，使 FIFO 错位。
    pub fn send(&self, request: Request) -> Transaction {
        let reply = if matches!(request.method, Method::Stream | Method::CancelStream) {
            Err(Error::InvalidArgument(format!(
                "{} is not allowed on the main connection, use open_stream",
                request.method.as_str()
            )))
        } else {
            encode(&request).and_then(|frame| {
                self.shared
                    .write(&lock(&self.shared.writer), &request, &frame)
            })
        };
        Transaction::new(reply, request)
    }

    /// `read`，对应 C++ 的 `Read`。
    pub fn read(&self, path: &str) -> Transaction {
        self.send(Request::new(Method::Read, path))
    }

    /// `update`，`payload` 是补丁，对应 C++ 的 `Update`。
    pub fn update(&self, path: &str, payload: impl Into<Value>) -> Transaction {
        self.send(Request::with_payload(Method::Update, path, payload))
    }

    /// `create`，`payload` 是创建参数，对应 C++ 的 `Create`。
    pub fn create(&self, path: &str, payload: impl Into<Value>) -> Transaction {
        self.send(Request::with_payload(Method::Create, path, payload))
    }

    /// `delete`，对应 C++ 的 `Delete`。
    pub fn delete(&self, path: &str) -> Transaction {
        self.send(Request::new(Method::Delete, path))
    }

    /// `call`，`payload` 是命令或动作的参数，对应 C++ 的 `Call`。
    pub fn call(&self, path: &str, payload: impl Into<Value>) -> Transaction {
        self.send(Request::with_payload(Method::Call, path, payload))
    }

    /// 服务端地址。
    pub fn addr(&self) -> SocketAddr {
        self.shared.addr
    }

    /// 至今经这条连接发出的 `create`、`update`、`delete`、`call` 数（含文件传输），供资源层判断缓存是否过期，取代
    /// C++ 的 `SetNonIdempotentRequestHandler`（回调捕获裸指针、只有一个槽位）。
    ///
    /// 主连接的请求在写锁内计数：先读计数、再发出读请求，若读到的计数之后又有变更请求，下一次比较就会发现；
    /// 计数没变时，所有已计数的变更都排在这次读取之前，服务端按顺序执行。文件传输在另一条连接上，完成后才计数。
    pub(crate) fn write_count(&self) -> u64 {
        self.shared.writes.load(Ordering::Relaxed)
    }

    /// 连接是否仍然可用；拆连接之后为 `false`，对应 C++ 的 `IsConnected`。断开的原因由之后的任一请求、订阅或流
    /// 以 `Disconnected` 给出（C++ 的 `SetDisconnectHandler`）。
    pub fn is_connected(&self) -> bool {
        self.shared.link.reason().is_none()
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("addr", &self.shared.addr)
            .field("connected", &self.is_connected())
            .finish()
    }
}

/// 一个已写出的请求，等待它的应答，对应 C++ 的 `GoTransaction`。
///
/// 丢弃即 fire-and-forget：请求已经写出，迟到的应答由墓碑槽位吸收。
#[must_use = "the request is already sent; call response() to wait for its reply, or drop it to ignore the reply"]
#[derive(Debug)]
pub struct Transaction {
    reply: Result<ReplyRx>,
    method: Method,
    path: String,
    timeout: Duration,
}

impl Transaction {
    pub(super) fn new(reply: Result<ReplyRx>, request: Request) -> Self {
        Self {
            reply,
            method: request.method,
            path: request.path,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// 替换默认的 3 s 超时。
    pub fn timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// 等待应答，对应 C++ 的 `GetResponse` / `CheckResponse`。超时从这里开始计，只覆盖等待应答。
    ///
    /// `status` 不是 OK 时返回 `Err(Request)`；超时返回 `Err(Timeout)`，槽位成为墓碑，连接照常可用。
    pub fn response(self) -> Result<Response> {
        let Self {
            reply,
            method,
            path,
            timeout,
        } = self;
        match reply?.recv_timeout(timeout) {
            Ok(Ok(response)) => error::check(method, path, response),
            Ok(Err(reason)) => Err(Error::Disconnected(reason)),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout {
                method,
                path,
                after: timeout,
            }),
            // 槽位总以响应或 fail_all 完成；发送端被丢弃只可能是连接已经释放。
            Err(RecvTimeoutError::Disconnected) => {
                Err(Error::Disconnected(Arc::new(DisconnectReason::Closed)))
            }
        }
    }
}
