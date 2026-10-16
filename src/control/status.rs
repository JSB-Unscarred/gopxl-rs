//! 状态码 `kStatus`：响应、流消息与 GDP Null 消息携带的结果码。

use std::fmt;

/// 状态码，对应 `kStatus`（`k32s`）。1 表示成功，0 与负数表示失败；服务端可能返回表外的负数，同样是失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Status(pub i32);

/// 从一张表同时生成常量与 [`Status::name`]，两份清单不会不一致。
macro_rules! statuses {
    ($($(#[$doc:meta])* $name:ident = $code:literal => $text:literal,)*) => {
        $($(#[$doc])* pub const $name: Self = Self($code);)*

        /// REST API 手册中的名字（`"ok"`、`"notFound"` …），表外的值为 `None`。手册未列出 −975，这里用 `"overheat"`。
        pub fn name(self) -> Option<&'static str> {
            match self.0 {
                $($code => Some($text),)*
                _ => None,
            }
        }
    };
}

impl Status {
    statuses! {
        /// `kOK`：操作成功。
        OK = 1 => "ok",
        /// `kERROR`：一般错误。
        ERROR = 0 => "error",
        /// `kERROR_STATE`：当前状态下无法完成操作。
        STATE = -1000 => "state",
        /// `kERROR_NOT_FOUND`：未找到。
        NOT_FOUND = -999 => "notFound",
        /// `kERROR_COMMAND`：命令无法识别。
        COMMAND = -998 => "command",
        /// `kERROR_PARAMETER`：参数无效。
        PARAMETER = -997 => "parameter",
        /// `kERROR_UNIMPLEMENTED`：功能未实现。
        UNIMPLEMENTED = -996 => "unimplemented",
        /// `kERROR_MEMORY`：内存不足。
        MEMORY = -994 => "memory",
        /// `kERROR_TIMEOUT`：超时。
        TIMEOUT = -993 => "timeout",
        /// `kERROR_INCOMPLETE`：数据不完整（例如缓冲区不足）。
        INCOMPLETE = -992 => "incomplete",
        /// `kERROR_STREAM`：通信流（网络、文件）出错。
        STREAM = -991 => "stream",
        /// `kERROR_CLOSED`：资源已不可用（例如流的资源被删除）。
        CLOSED = -990 => "closed",
        /// `kERROR_VERSION`：版本不兼容。
        VERSION = -989 => "version",
        /// `kERROR_ABORT`：操作被中止。
        ABORT = -988 => "abort",
        /// `kERROR_ALREADY_EXISTS`：与已有项冲突。
        ALREADY_EXISTS = -987 => "alreadyExists",
        /// `kERROR_NETWORK`：网络设置或资源错误。
        NETWORK = -986 => "network",
        /// `kERROR_HEAP`：堆错误（泄漏或重复释放）。
        HEAP = -985 => "heap",
        /// `kERROR_FORMAT`：数据解析或格式错误。
        FORMAT = -984 => "format",
        /// `kERROR_READ_ONLY`：对象只读。
        READ_ONLY = -983 => "readOnly",
        /// `kERROR_WRITE_ONLY`：对象只写。
        WRITE_ONLY = -982 => "writeOnly",
        /// `kERROR_BUSY`：忙，无法处理请求。
        BUSY = -981 => "busy",
        /// `kERROR_CONFLICT`：配置或资源冲突。
        CONFLICT = -980 => "conflict",
        /// `kERROR_OS`：操作系统报告的错误。
        OS = -979 => "os",
        /// `kERROR_DEVICE`：硬件设备错误。
        DEVICE = -978 => "device",
        /// `kERROR_FULL`：资源已占满。
        FULL = -977 => "full",
        /// `kERROR_IN_PROGRESS`：操作进行中，尚未完成。
        IN_PROGRESS = -976 => "inProgress",
        /// `kERROR_OVERHEAT`：过热，系统功能受限。
        OVERHEAT = -975 => "overheat",
    }

    /// 是否成功（`== OK`）。与 SDK 的 `!= kOK` 判断一致：0 与其他值都是失败。
    pub fn is_ok(self) -> bool {
        self == Self::OK
    }
}

/// 输出 `notFound (-999)`；表外的值只输出数字。
impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} ({})", self.0),
            None => write!(f, "{}", self.0),
        }
    }
}
