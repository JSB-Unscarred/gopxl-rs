//! 主控制连接：[`Connection`] 拥有连接，[`Client`] 发请求，[`Transaction`] 等应答。
//!
//! 线上的请求不带 id，响应按发送顺序配对：[`ControlCore`] 维护挂起请求的等待队列，每个响应交给队首。
//!
//! 一条连接一个读任务，请求在调用方的任务上写出：
//! - 调用方持写锁，先在等待队列登记（[`ControlCore::register`]）再写出整帧，等待队列的顺序因此等于线上顺序；
//!   写出失败或超时（10 s）时可能只写出了半帧，立即断开连接；
//! - 读任务无超时地读帧、解码，再取分派锁（保护等待队列与订阅表）交给 `ControlCore`，通知放进订阅队列；
//! - 锁的顺序固定为写锁（tokio `Mutex`，跨 await 持有）→ 分派锁（std `Mutex`，不跨 await）。读任务只取分派锁，
//!   所以服务端暂停读取、调用方的写出被阻塞时，读任务仍在读；若读任务也等写锁，客户端与服务端会同时卡在写出上、
//!   谁都不读。读任务也从不等待消费者（订阅队列满了丢最老的），读得慢的客户端由服务端的 10 s 写超时断开。
//!
//! 断开连接（[`Shared::teardown`]）只执行一次：记下原因、shutdown 并中止读任务、所有挂起的请求以同一个原因失败、
//! 关闭订阅队列。读错误、EOF、坏帧、写失败与 `Connection` 的 Drop 都走这里；之后 [`Client::is_connected`] 为假，
//! 每个等待都返回 `Disconnected(原因)`，断开只经这个错误通知调用方。

use std::convert::Infallible;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

use super::subscription::{self, SubQueue};
use super::{ControlCore, HEADER_LEN, Inbound, Method, ReplyRx, Request, Response};
use crate::error::{self, DisconnectReason, Error, Result};
use crate::frame::FrameReader;
use crate::net::{self, Link, MAX_FRAME, lock};
use crate::value::Value;

/// 建立控制连接的超时。
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// 请求等待应答的默认超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// 主控制连接的唯一所有者，不能 clone。
///
/// Drop 即断开连接：所有挂起的请求以 [`DisconnectReason::Closed`] 失败，读任务中止。请求、订阅与流经
/// [`client`](Self::client) 发出。
#[derive(Debug)]
pub struct Connection {
    client: Client,
}

impl Connection {
    /// 连接 `addr`（超时 1 s），在当前 tokio 运行时上启动读任务。
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = net::connect(addr, CONNECT_TIMEOUT).await?;
        let link = Link::new(&stream).map_err(|source| Error::Connect { addr, source })?;
        let (reader, writer) = stream.into_split();
        let shared = Arc::new(Shared {
            addr,
            link,
            writer: tokio::sync::Mutex::new(writer),
            dispatch: Mutex::default(),
            writes: AtomicU64::new(0),
        });
        let task = tokio::spawn({
            let shared = Arc::clone(&shared);
            async move {
                let Err(reason) = shared.read_frames(reader).await;
                shared.teardown(reason);
            }
        });
        shared.link.set_reader(task.abort_handle());
        Ok(Self {
            client: Client { shared },
        })
    }

    /// 发请求、订阅、开流的句柄；clone 可以移到别的任务或存进别的对象，但不让连接存活。
    pub fn client(&self) -> &Client {
        &self.client
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.client.shared.teardown(DisconnectReason::Closed);
    }
}

/// 主连接的句柄：发请求、订阅、开流；可以 clone、移到别的任务，但不让连接存活。`Connection` Drop 之后，经它
/// 发出的请求立即以 `Disconnected(Closed)` 失败。
#[derive(Clone)]
pub struct Client {
    pub(super) shared: Arc<Shared>,
}

/// 连接的共享状态，由 `Client`（经 `Arc`）与读任务共同持有。
pub(super) struct Shared {
    addr: SocketAddr,
    pub(super) link: Link,
    /// 写锁，锁内是 socket 的写半边：整帧写出期间持有，保证帧不交错、等待队列的顺序等于线上顺序；订阅的登记与
    /// 注销也在它里面。读任务持有读半边，不取这把锁。
    pub(super) writer: tokio::sync::Mutex<OwnedWriteHalf>,
    /// 分派锁：读任务处理每一帧时持有。
    pub(super) dispatch: Mutex<Dispatch>,
    /// 经这条连接发出的变更请求数，见 [`Client::write_count`]。
    writes: AtomicU64,
}

/// 分派锁保护的状态：读任务按帧更新等待队列，并把通知投给订阅队列。
#[derive(Default)]
pub(super) struct Dispatch {
    core: ControlCore,
    /// 本地订阅的队列；`Subscription` 拥有队列，这里只持 `Weak`，注销时自行移除。
    pub(super) subs: Vec<Weak<SubQueue>>,
}

impl Shared {
    /// 读任务：读帧、解码、分派，直到出错或 EOF，返回断开的原因。
    async fn read_frames(
        &self,
        mut reader: OwnedReadHalf,
    ) -> std::result::Result<Infallible, DisconnectReason> {
        let mut frames = FrameReader::new(HEADER_LEN, MAX_FRAME);
        loop {
            let frame = net::read_frame(&mut reader, &mut frames, Duration::MAX)
                .await?
                .ok_or(DisconnectReason::Eof)?;
            let inbound = Inbound::decode(frame)?;
            let mut dispatch = lock(&self.dispatch);
            if let Some(notification) = dispatch.core.on_inbound(inbound) {
                subscription::deliver(&dispatch.subs, notification);
            }
        }
    }

    /// 断开连接，只有第一次调用生效。shutdown 在 `fail_all` 之前，所以 `fail_all` 之后才登记的请求写出必然失败，
    /// 不会在等待队列里一直挂着。
    pub(super) fn teardown(&self, reason: DisconnectReason) {
        let Some(reason) = self.link.finish(reason) else {
            return;
        };
        let mut dispatch = lock(&self.dispatch);
        dispatch.core.fail_all(reason);
        subscription::close_all(&dispatch.subs);
    }

    /// 可能改变服务端状态的请求（`create`、`update`、`delete`、`call`）计入变更请求数。
    pub(super) fn count_write(&self, method: Method) {
        if matches!(
            method,
            Method::Create | Method::Update | Method::Delete | Method::Call
        ) {
            self.writes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 登记并写出一帧；`writer` 是写锁里的写半边，调用方持锁。
    pub(super) async fn write(
        &self,
        writer: &mut OwnedWriteHalf,
        request: &Request,
        frame: &[u8],
    ) -> Result<ReplyRx> {
        if let Some(reason) = self.link.reason() {
            return Err(Error::Disconnected(Arc::clone(reason)));
        }
        let reply = lock(&self.dispatch).core.register();
        // 在写锁内计数，见 `Client::write_count`。
        self.count_write(request.method);
        if let Err(error) = net::write_all(writer, frame).await {
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

/// 等待应答：`status` 不是 OK 时为 `Err(Request)`；超时为 `Err(Timeout)`，请求在等待队列里的位置保留，迟到的
/// 应答到达时被丢弃；发送端被丢弃只可能是连接已经释放。
pub(super) async fn await_reply(
    reply: ReplyRx,
    limit: Duration,
    method: Method,
    path: String,
) -> Result<Response> {
    match timeout(limit, reply).await {
        Ok(Ok(Ok(response))) => error::check(method, path, response),
        Ok(Ok(Err(reason))) => Err(Error::Disconnected(reason)),
        Ok(Err(_)) => Err(Error::Disconnected(Arc::new(DisconnectReason::Closed))),
        Err(_) => Err(Error::Timeout {
            method,
            path,
            after: limit,
        }),
    }
}

impl Client {
    /// 任意请求的事务，[`Transaction::response`] 时才写出；`listSub`、`clearSub`、`listStream` 也经它发送。
    pub fn send(&self, request: Request) -> Transaction {
        Transaction {
            client: self.clone(),
            request,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// `read`。
    pub fn read(&self, path: &str) -> Transaction {
        self.send(Request::new(Method::Read, path))
    }

    /// `update`，`payload` 是补丁。
    pub fn update(&self, path: &str, payload: impl Into<Value>) -> Transaction {
        self.send(Request::with_payload(Method::Update, path, payload))
    }

    /// `create`，`payload` 是创建参数。
    pub fn create(&self, path: &str, payload: impl Into<Value>) -> Transaction {
        self.send(Request::with_payload(Method::Create, path, payload))
    }

    /// `delete`。
    pub fn delete(&self, path: &str) -> Transaction {
        self.send(Request::new(Method::Delete, path))
    }

    /// `call`，`payload` 是命令或动作的参数。
    pub fn call(&self, path: &str, payload: impl Into<Value>) -> Transaction {
        self.send(Request::with_payload(Method::Call, path, payload))
    }

    /// 写出 `request`，不等应答（迟到的应答到达时被丢弃），退订用。
    pub(super) async fn fire(&self, request: &Request) -> Result<()> {
        let frame = encode(request)?;
        let mut writer = self.shared.writer.lock().await;
        self.shared
            .write(&mut writer, request, &frame)
            .await
            .map(drop)
    }

    /// 服务端地址。
    pub fn addr(&self) -> SocketAddr {
        self.shared.addr
    }

    /// 至今经这条连接发出的 `create`、`update`、`delete`、`call` 数（含文件传输），供资源层判断缓存是否过期。
    ///
    /// 主连接的请求在写锁内计数：先读计数、再发出读请求，若读到的计数之后又有变更请求，下一次比较就会发现；
    /// 计数没变时，所有已计数的变更都排在这次读取之前，服务端按顺序执行。文件传输在另一条连接上，完成后才计数。
    pub(crate) fn write_count(&self) -> u64 {
        self.shared.writes.load(Ordering::Relaxed)
    }

    /// 连接是否仍然可用；断开之后为 `false`。断开的原因由之后的任一请求、订阅或流以 `Disconnected` 给出。
    pub fn is_connected(&self) -> bool {
        self.shared.link.reason().is_none()
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("addr", &self.shared.addr)
            .field("connected", &self.is_connected())
            .finish()
    }
}

/// 一个待发送的请求及其超时：[`response`](Self::response) 写出请求并等待应答，丢弃则什么也不发。
#[must_use = "the request is only sent when response() is awaited"]
#[derive(Debug)]
pub struct Transaction {
    client: Client,
    request: Request,
    timeout: Duration,
}

impl Transaction {
    /// 替换默认的 3 s 超时。
    pub fn timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// 写出请求并等待应答。超时只覆盖等待应答，从写出之后开始计。
    ///
    /// `status` 不是 OK 时返回 `Err(Request)`；超时返回 `Err(Timeout)`，迟到的应答被丢弃，连接照常可用。`stream`
    /// 与 `cancelStream` 报 `InvalidArgument`：流用 [`Client::open_stream`] 在自己的连接上开，原因见
    /// [`StreamHandle`](crate::control::StreamHandle)。
    pub async fn response(self) -> Result<Response> {
        let Self {
            client,
            request,
            timeout,
        } = self;
        if matches!(request.method, Method::Stream | Method::CancelStream) {
            return Err(Error::InvalidArgument(format!(
                "{} is not allowed on the main connection, use open_stream",
                request.method.as_str()
            )));
        }
        let frame = encode(&request)?;
        let reply = {
            let mut writer = client.shared.writer.lock().await;
            client.shared.write(&mut writer, &request, &frame).await?
        };
        await_reply(reply, timeout, request.method, request.path).await
    }
}
