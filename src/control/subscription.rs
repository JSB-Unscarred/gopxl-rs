//! 订阅：每个本地订阅一个有界队列，读任务按服务端的匹配规则投递，消费者在自己的任务上拉取。对应 C++ 的
//! `Sub` / `UnSub` 与 `SetSubHandler`；回调式的 `AddNotificationListener` 即在自己的任务里循环 `recv`。
//!
//! C++ 每条通知起一个线程（乱序、线程数无上限），回调在锁内执行会重入死锁，`sub` 失败时监听器留在表里，本地
//! 分派也与服务端的匹配规则不一致。这里：
//! - 读任务只把通知放进队列，从不执行用户代码；队列满了丢最老的并记下条数（[`Notice::Lagged`]），读任务从不等待；
//! - 同一个表达式的本地订阅共享服务端的一个订阅：每次 `subscribe` 都发送自己的 `sub`（服务端对同一文本先删后加，
//!   重复无害，协议假设 S1），最后一个本地订阅退订时发送 `unSub`。登记、注销与写出都在写锁内，所以 `unSub` 在
//!   服务端排在之前的 `sub` 之后、之后的 `sub` 之前；超时之后才执行的 `sub` 也会被它撤销。
//!   [`Subscription::unsubscribe`] 等写出完成；Drop 不能 await，把 `unSub` 交给一个 task，运行时已停时只能让
//!   服务端在连接关闭时忘记订阅。

use std::collections::VecDeque;
use std::mem;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::timeout;

use super::client::{Client, Shared, await_reply, encode};
use super::{Method, Notification, Request, SubscriptionExpr};
use crate::error::{Error, Result};
use crate::net::lock;

/// 每个订阅队列的容量。通知很小（只有 `/system`、`/replay` 的通知带载荷），只按条数设上限。
const CAPACITY: usize = 1024;

/// `sub` 等待应答的超时（SDK 为 5 s）。
const SUB_TIMEOUT: Duration = Duration::from_secs(5);

/// 订阅队列里取出的一项。
#[derive(Clone, Debug)]
pub enum Notice {
    /// 一条通知。投递给多个订阅时共享同一份。
    Event(Arc<Notification>),
    /// 队列满了，在此之前丢掉了这么多条最老的通知。
    Lagged(u64),
}

/// 一个本地订阅，对应 C++ 的 `Sub` 加上 `SetSubHandler` 中按路径的筛选。
///
/// owned、`Send`，可以移到别的任务。只收到服务端按这个表达式会投递的通知。[`unsubscribe`](Self::unsubscribe) 或
/// Drop 时注销：是该表达式最后一个本地订阅时写出 `unSub`，不等待应答（C++ 的析构等待 `UnSub` 的应答，最多阻塞 5 s）。
#[derive(Debug)]
pub struct Subscription {
    client: Client,
    queue: Arc<SubQueue>,
}

/// 一个订阅的队列，由 `Subscription` 拥有，读任务经 `Weak` 投递。
#[derive(Debug)]
pub(super) struct SubQueue {
    expr: SubscriptionExpr,
    state: Mutex<Queue>,
    ready: Notify,
}

/// [`SubQueue::pop`] 取到的项；`pop` 返回 `None` 表示队列为空。
enum Pop {
    Notice(Notice),
    Closed,
}

#[derive(Debug, Default)]
struct Queue {
    notices: VecDeque<Arc<Notification>>,
    /// 上次取出之后丢掉的条数。
    lagged: u64,
    closed: bool,
}

impl Client {
    /// 订阅表达式 `expr`（例如 `/system`、`/tools/*`），等待 `sub` 的应答（5 s）。
    ///
    /// 表达式含正则元字符等本地无法与服务端一致匹配的内容时报 `InvalidArgument`（见 [`SubscriptionExpr::new`]）。
    pub async fn subscribe(&self, expr: &str) -> Result<Subscription> {
        let expr =
            SubscriptionExpr::new(expr).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let request = Request::new(Method::Sub, expr.text());
        let frame = encode(&request)?;
        let queue = Arc::new(SubQueue {
            expr,
            state: Mutex::default(),
            ready: Notify::new(),
        });
        let shared = &self.shared;
        // 写锁内先登记队列再写出 sub：应答之后紧接着的通知不会漏掉，别的订阅退订时也能看到它。
        let reply = {
            let mut writer = shared.writer.lock().await;
            lock(&shared.dispatch).subs.push(Arc::downgrade(&queue));
            shared.write(&mut writer, &request, &frame).await
        };
        // 从这里起由 Drop 注销队列、必要时退订，包括 sub 失败或超时的情况。
        let subscription = Subscription {
            client: self.clone(),
            queue,
        };
        await_reply(reply?, SUB_TIMEOUT, request.method, request.path).await?;
        Ok(subscription)
    }
}

impl Subscription {
    /// 取出下一项，一直等待。连接断开后返回 `Err(Disconnected)`，未取走的通知丢弃。
    pub async fn recv(&self) -> Result<Notice> {
        loop {
            // 先登记等待再查队列：close 的 notify_waiters 不会漏掉这次等待。
            let notified = self.queue.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match self.queue.pop() {
                Some(Pop::Notice(notice)) => return Ok(notice),
                Some(Pop::Closed) => {
                    return Err(Error::Disconnected(self.client.shared.link.disconnected()));
                }
                None => notified.await,
            }
        }
    }

    /// 取出下一项，最多等待 `limit`；超时返回 `Ok(None)`。
    pub async fn recv_timeout(&self, limit: Duration) -> Result<Option<Notice>> {
        match timeout(limit, self.recv()).await {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }

    /// 取出已到达的下一项，不等待。
    pub fn try_recv(&self) -> Result<Option<Notice>> {
        match self.queue.pop() {
            Some(Pop::Notice(notice)) => Ok(Some(notice)),
            Some(Pop::Closed) => Err(Error::Disconnected(self.client.shared.link.disconnected())),
            None => Ok(None),
        }
    }

    /// 注销并等 `unSub` 写出（是该表达式最后一个本地订阅时才写），不等应答：迟到的应答由墓碑吸收。注销与写出在
    /// 写锁内，别的任务此刻订阅同一表达式也不会被误撤销。连接已断开时报 `Disconnected`，服务端早已忘记订阅。
    pub async fn unsubscribe(self) -> Result<()> {
        let shared = &self.client.shared;
        let mut writer = shared.writer.lock().await;
        let Some(request) = unregister(shared, &self.queue) else {
            return Ok(());
        };
        let frame = encode(&request)?;
        shared.write(&mut writer, &request, &frame).await.map(drop)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // 不能在 Drop 里 await：注销是同步的，unSub 交给一个 task，运行时已停时只能让服务端在连接关闭时忘记订阅。
        // task 写出之前别的任务若订阅了同一表达式，unSub 会把它一起撤销；要确定的顺序用 unsubscribe。
        if let Some(request) = unregister(&self.client.shared, &self.queue) {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let client = self.client.clone();
                runtime.spawn(async move {
                    let _ = client.fire(&request).await;
                });
            }
        }
    }
}

/// 从分派表移除 `queue`；是该表达式最后一个本地订阅时返回要写出的 `unSub`。队列已不在表里（第二次调用）时为
/// `None`。
fn unregister(shared: &Shared, queue: &Arc<SubQueue>) -> Option<Request> {
    let mut dispatch = lock(&shared.dispatch);
    let before = dispatch.subs.len();
    dispatch
        .subs
        .retain(|weak| weak.as_ptr() != Arc::as_ptr(queue));
    if dispatch.subs.len() == before {
        return None;
    }
    let text = queue.expr.text();
    let shared_by_others = dispatch
        .subs
        .iter()
        .filter_map(Weak::upgrade)
        .any(|other| other.expr.text() == text);
    (!shared_by_others).then(|| Request::new(Method::UnSub, text))
}

impl SubQueue {
    /// 放入一条通知；队列满了丢最老的。
    fn push(&self, notification: &Arc<Notification>) {
        let mut queue = lock(&self.state);
        if queue.closed {
            return;
        }
        if queue.notices.len() == CAPACITY {
            queue.notices.pop_front();
            queue.lagged += 1;
        }
        queue.notices.push_back(Arc::clone(notification));
        drop(queue);
        self.ready.notify_one();
    }

    /// 连接已断开：关闭并丢弃未取走的通知（调用方要重连并重读状态）。
    fn close(&self) {
        let mut queue = lock(&self.state);
        *queue = Queue {
            closed: true,
            ..Queue::default()
        };
        drop(queue);
        self.ready.notify_waiters();
    }

    /// 取出一项，不等待。丢弃的条数先于剩下的通知取出，消费者知道在哪里出现了缺口。
    fn pop(&self) -> Option<Pop> {
        let mut queue = lock(&self.state);
        if queue.lagged > 0 {
            Some(Pop::Notice(Notice::Lagged(mem::take(&mut queue.lagged))))
        } else if let Some(notification) = queue.notices.pop_front() {
            Some(Pop::Notice(Notice::Event(notification)))
        } else if queue.closed {
            Some(Pop::Closed)
        } else {
            None
        }
    }
}

/// 读任务调用：按订阅表达式投递一条通知（`/` 上的 `embeddedUpdated` 投给每个订阅）。
pub(super) fn deliver(subs: &[Weak<SubQueue>], notification: Notification) {
    let notification = Arc::new(notification);
    for queue in subs.iter().filter_map(Weak::upgrade) {
        if queue.expr.delivers(&notification.event, &notification.path) {
            queue.push(&notification);
        }
    }
}

/// 拆连接时关闭所有订阅队列，等待中的消费者随即返回。
pub(super) fn close_all(subs: &[Weak<SubQueue>]) {
    for queue in subs.iter().filter_map(Weak::upgrade) {
        queue.close();
    }
}

// 丢最老与 Lagged 计数是自有算法，出错时通知静默丢失，消费者察觉不到缺口。
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::Notify;

    use super::{CAPACITY, Notice, Pop, SubQueue};
    use crate::control::{EventType, Notification, SubscriptionExpr};

    // 队列满了丢最老的：消费者先看到丢掉的条数，再按顺序看到剩下的通知；关闭后丢弃未取走的通知。
    #[test]
    fn overflow_reports_lag_before_remaining_notices() {
        let queue = SubQueue {
            expr: SubscriptionExpr::new("/*").unwrap(),
            state: Mutex::default(),
            ready: Notify::new(),
        };
        for i in 0..CAPACITY + 3 {
            queue.push(&Arc::new(Notification {
                event: EventType::Updated,
                path: format!("/{i}"),
                payload: crate::Value::Nil,
            }));
        }
        assert!(matches!(queue.pop(), Some(Pop::Notice(Notice::Lagged(3)))));
        assert!(matches!(queue.pop(), Some(Pop::Notice(Notice::Event(n))) if n.path == "/3"));
        queue.close();
        assert!(matches!(queue.pop(), Some(Pop::Closed)));
    }
}
