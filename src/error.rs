//! 连接层与门面的错误类型 [`Error`]。变体按调用方能做什么划分：重试或重连（`Connect`、`Timeout`、
//! `Disconnected`），查看服务端的回复（`Request`、`Payload`），修正参数（`InvalidArgument`、`Validation`）。

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::control::{DisconnectReason, Method, Response};
use crate::schema::Issue;

/// 本 crate 连接层的 `Result`。
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// 连接层的错误，对应 C++ 的 `GoChannelError`（前三个变体）与 `GoRequestError`（`Request`）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// 建立 TCP 连接、设置 socket 或启动连接的线程失败。
    #[error("connect to {addr} failed")]
    Connect {
        /// 对端地址。
        addr: SocketAddr,
        /// 底层错误。
        #[source]
        source: io::Error,
    },
    /// 请求已写出，期限内没有应答。主连接上的槽位成为墓碑，迟到的应答被它吸收，连接照常可用。
    #[error("{} {path}: no response within {after:?}", method.as_str())]
    Timeout {
        /// 请求的方法。
        method: Method,
        /// 请求的路径。
        path: String,
        /// 等待的时长。
        after: Duration,
    },
    /// 连接已拆掉；同一条连接上所有挂起的请求得到同一个原因。调用方只能重新连接。
    #[error("connection lost: {0}")]
    Disconnected(Arc<DisconnectReason>),
    /// 服务端的 `status` 不是 OK。
    #[error(transparent)]
    Request(Box<RequestError>),
    /// 响应成功，但载荷缺少要读取的字段，或字段类型不对（C++ 抛 `GoJsonError` / `GoResourceError`）。
    #[error("{path}: response payload has no valid {field:?}")]
    Payload {
        /// 请求的路径。
        path: String,
        /// 载荷中的字段名或 JSON Pointer。
        field: String,
    },
    /// 资源开启了校验（[`Resource::set_validation`](crate::resource::Resource::set_validation)），补丁中 `pointer` 处的值不符合
    /// schema，请求没有发出，对应 C++ 的 `GoResourceValidationError`。
    #[error("{path}: {pointer:?} fails schema validation: {}", list(issues))]
    Validation {
        /// 资源路径。
        path: String,
        /// 补丁中不合格的值的 JSON Pointer。
        pointer: String,
        /// 发现的问题，非空。
        issues: Vec<Issue>,
    },
    /// 参数无法发送：端口为 0、帧超过 4 GiB、主连接上的 `stream` / `cancelStream`、无法匹配的订阅表达式。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// 服务端拒绝了请求（`status` 不是 OK），携带完整的响应；`payload.errors` 里通常有本地化的说明。
#[derive(Debug)]
pub struct RequestError {
    /// 请求的方法。
    pub method: Method,
    /// 请求的路径。
    pub path: String,
    /// 服务端的响应。
    pub response: Response,
}

impl std::error::Error for RequestError {}

/// 输出 `read /x: notFound (-999): <第一条 errors 的 description>`。
impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            method,
            path,
            response,
        } = self;
        write!(f, "{} {path}: {}", method.as_str(), response.status)?;
        let errors = response.payload.pointer("/errors/0/description");
        if let Some(description) = errors.and_then(crate::Value::as_str) {
            write!(f, ": {description}")?;
        }
        Ok(())
    }
}

/// 按 `status` 判定成败：只看顶层 `status`，与 SDK 一致；`status` 为 OK 时 `payload.errors` 不升级为错误。
pub(crate) fn check(method: Method, path: String, response: Response) -> Result<Response> {
    if response.status.is_ok() {
        Ok(response)
    } else {
        Err(Error::Request(Box::new(RequestError {
            method,
            path,
            response,
        })))
    }
}

/// `Validation` 的 `Display`：各问题以 `; ` 分隔。
fn list(issues: &[Issue]) -> String {
    let issues: Vec<_> = issues.iter().map(Issue::to_string).collect();
    issues.join("; ")
}
