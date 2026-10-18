//! 订阅：每个本地订阅一个有界队列，读线程按服务端的匹配规则投递，消费者在自己的线程上拉取。对应 C++ 的
//! `Sub` / `UnSub` 与 `SetSubHandler`；回调式的 `AddNotificationListener` 即在自己的线程里循环取出。
//!
//! C++ 每条通知起一个线程（乱序、线程数无上限），回调在锁内执行会重入死锁，`sub` 失败时监听器留在表里，本地
//! 分派也与服务端的匹配规则不一致。这里：
//! - 读线程只把通知放进队列，从不执行用户代码；队列满了丢最老的并记下条数（[`Notice::Lagged`]），读线程从不阻塞；
//! - 同一个表达式的本地订阅共享服务端的一个订阅：每次 `subscribe` 都发送自己的 `sub`（服务端对同一文本先删后加，
//!   重复无害，协议假设 S1），Drop 时若没有别的本地订阅用同一表达式就发送 `unSub`。登记、注销与写出都在写锁内，所以
//!   `unSub` 在服务端排在之前的 `sub` 之后、之后的 `sub` 之前；超时之后才执行的 `sub` 也会被它撤销。

use std::collections::VecDeque;
use std::mem;
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::Duration;

use super::client::{Client, Transaction, encode};
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
/// owned、`Send`，可以移到别的线程。只收到服务端按这个表达式会投递的通知。Drop 时注销：是该表达式最后一个
/// 本地订阅时写出 `unSub`，不等待应答（C++ 的析构等待 `UnSub` 的应答，最多阻塞 5 s）。
#[derive(Debug)]
pub struct Subscription {
    client: Client,
    queue: Arc<SubQueue>,
}

/// 一个订阅的队列，由 `Subscription` 拥有，读线程经 `Weak` 投递。
#[derive(Debug)]
pub(super) struct SubQueue {
    expr: SubscriptionExpr,
    state: Mutex<Queue>,
    ready: Condvar,
}

/// [`SubQueue::pop`] 的结果。
enum Pop {
    Notice(Notice),
    Timeout,
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
    pub fn subscribe(&self, expr: &str) -> Result<Subscription> {
        let expr =
            SubscriptionExpr::new(expr).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let request = Request::new(Method::Sub, expr.text());
        let frame = encode(&request)?;
        let queue = Arc::new(SubQueue {
            expr,
            state: Mutex::default(),
            ready: Condvar::new(),
        });
        let shared = &self.shared;
        // 写锁内先登记队列再写出 sub：应答之后紧接着的通知不会漏掉，别的订阅 Drop 时也能看到它。
        let mut writer = lock(&shared.writer);
        lock(&shared.dispatch).subs.push(Arc::downgrade(&queue));
        let reply = shared.write(&mut writer, &request, &frame);
        drop(writer);
        // 从这里起由 Drop 注销队列、必要时退订，包括 sub 失败或超时的情况。
        let subscription = Subscription {
            client: self.clone(),
            queue,
        };
        Transaction::new(reply, request)
            .timeout(SUB_TIMEOUT)
            .response()?;
        Ok(subscription)
    }
}

impl Subscription {
    /// 取出下一项，最多等待 `timeout`（`Duration::MAX` 即一直等待）；超时返回 `Ok(None)`。连接断开后返回
    /// `Err(Disconnected)`，未取走的通知丢弃。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<Notice>> {
        match self.queue.pop(timeout) {
            Pop::Notice(notice) => Ok(Some(notice)),
            Pop::Timeout => Ok(None),
            Pop::Closed => Err(Error::Disconnected(self.client.shared.link.disconnected())),
        }
    }

    /// 取出下一项，一直等待（`recv_timeout(Duration::MAX)`）。
    pub fn recv(&self) -> Result<Notice> {
        loop {
            if let Some(notice) = self.recv_timeout(Duration::MAX)? {
                return Ok(notice);
            }
        }
    }

    /// 取出已到达的下一项，不等待（`recv_timeout(Duration::ZERO)`）。
    pub fn try_recv(&self) -> Result<Option<Notice>> {
        self.recv_timeout(Duration::ZERO)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let shared = &self.client.shared;
        let text = self.queue.expr.text();
        let mut writer = lock(&shared.writer);
        let mut dispatch = lock(&shared.dispatch);
        dispatch
            .subs
            .retain(|weak| weak.as_ptr() != Arc::as_ptr(&self.queue));
        let shared_by_others = dispatch
            .subs
            .iter()
            .filter_map(Weak::upgrade)
            .any(|queue| queue.expr.text() == text);
        drop(dispatch);
        if shared_by_others {
            return;
        }
        let request = Request::new(Method::UnSub, text);
        // 不等应答：接收端随即丢弃，槽位成为墓碑。连接已断开时服务端自然忘记订阅，写出失败无妨。
        if let Ok(frame) = encode(&request) {
            let _ = shared.write(&mut writer, &request, &frame);
        }
    }
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
        self.ready.notify_all();
    }

    /// 取出一项，最多等待 `timeout`。丢弃的条数先于剩下的通知取出，消费者知道在哪里出现了缺口。
    fn pop(&self, timeout: Duration) -> Pop {
        let (mut queue, _) = self
            .ready
            .wait_timeout_while(lock(&self.state), timeout, |queue| {
                queue.lagged == 0 && queue.notices.is_empty() && !queue.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        if queue.lagged > 0 {
            Pop::Notice(Notice::Lagged(mem::take(&mut queue.lagged)))
        } else if let Some(notification) = queue.notices.pop_front() {
            Pop::Notice(Notice::Event(notification))
        } else if queue.closed {
            Pop::Closed
        } else {
            Pop::Timeout
        }
    }
}

/// 读线程调用：按订阅表达式投递一条通知（`/` 上的 `embeddedUpdated` 投给每个订阅）。
pub(super) fn deliver(subs: &[Weak<SubQueue>], notification: Notification) {
    let notification = Arc::new(notification);
    for queue in subs.iter().filter_map(Weak::upgrade) {
        if queue.expr.delivers(&notification.event, &notification.path) {
            queue.push(&notification);
        }
    }
}

/// 拆连接时关闭所有订阅队列，阻塞在 `recv_timeout` 上的消费者随即返回。
pub(super) fn close_all(subs: &[Weak<SubQueue>]) {
    for queue in subs.iter().filter_map(Weak::upgrade) {
        queue.close();
    }
}

// 丢最老与 Lagged 计数是自有算法，出错时通知静默丢失，消费者察觉不到缺口。
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use super::{CAPACITY, Notice, Pop, SubQueue};
    use crate::control::{EventType, Notification, SubscriptionExpr};

    // 队列满了丢最老的：消费者先看到丢掉的条数，再按顺序看到剩下的通知；关闭后丢弃未取走的通知。
    #[test]
    fn overflow_reports_lag_before_remaining_notices() {
        let queue = SubQueue {
            expr: SubscriptionExpr::new("/*").unwrap(),
            state: Mutex::default(),
            ready: Condvar::new(),
        };
        for i in 0..CAPACITY + 3 {
            queue.push(&Arc::new(Notification {
                event: EventType::Updated,
                path: format!("/{i}"),
                payload: crate::Value::Nil,
            }));
        }
        let pop = || queue.pop(Duration::ZERO);
        assert!(matches!(pop(), Pop::Notice(Notice::Lagged(3))));
        assert!(matches!(pop(), Pop::Notice(Notice::Event(n)) if n.path == "/3"));
        queue.close();
        assert!(matches!(queue.pop(Duration::MAX), Pop::Closed));
    }
}
