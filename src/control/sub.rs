//! 订阅表达式的匹配：按服务端的规则判断一条通知是否属于某个订阅（`GoApi::SubscriptionExpression`）。
//!
//! 服务端为每个表达式生成两个 ECMAScript 正则，对通知的路径做整串匹配：
//! - R（`created`、`updated`、`deleted` 与未知事件）：转义 `. : + $`；`**` → `*` → `.*`，所以 `*` 跨越 `/`；
//!   `?` → `.`；结尾的 `/.*` 换成 `(/.*)?`，所以 `/tools/*` 也匹配 `/tools`；
//! - E（`embeddedUpdated`）：以 `/` 开头时每个 `/` 换成 `(/` 并在末尾补 n−1 个 `)?` 和一个 `)`，于是匹配表达式
//!   路径的每一级祖先（根除外）；转义与通配同 R，但 `?` 不转换，是正则量词。另外，`/` 上的 `embeddedUpdated`
//!   投给每个订阅。
//!
//! 本地逐字移植服务端的变换，再用 regex-lite 编译，订阅收到的通知因此与服务端投递的一致。

use std::borrow::Cow;

use regex_lite::Regex;

use super::EventType;

/// 一个订阅表达式（`sub` 请求的 `path`），判断服务端会不会因为它投递一条通知。
#[derive(Clone, Debug)]
pub struct SubscriptionExpr {
    text: String,
    is_match: Regex,
    is_embedded_match: Regex,
}

/// [`SubscriptionExpr::new`] 的错误：表达式含正则元字符或 NUL，或生成的正则无法编译。携带原表达式。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unsupported subscription expression {0:?}")]
pub struct SubExprError(pub String);

impl SubscriptionExpr {
    /// 编译表达式。
    ///
    /// 含 `| ( ) [ ] { } ^ \` 时报错：服务端把它们原样交给 ECMAScript 正则，而 regex-lite 在字符类、反向引用、
    /// 断言等语法上与之不同，本地无法保证匹配一致。不含它们时两边只用到字面字符、`.`、`.*`、分组与 `?` 量词；
    /// `(?R)` 让 `.` 与 ECMAScript 一样不匹配 `\r`、`\n`，再按字节匹配，结果与服务端相同。含 NUL（服务端在 NUL
    /// 处截断表达式）、生成的正则无法编译（例如 `?`）或超过 regex-lite 的嵌套上限（约 16 级 `/`，已知的资源路径
    /// 不超过 10 级）时同样报错。叠加的量词（`/a???`）照常编译，部分服务端平台会拒绝这种表达式。
    pub fn new(text: &str) -> Result<Self, SubExprError> {
        let error = || SubExprError(text.to_owned());
        if text.contains(['|', '(', ')', '[', ']', '{', '}', '^', '\\', '\0']) {
            return Err(error());
        }
        let compile =
            |source: String| Regex::new(&format!("(?R)^(?:{source})$")).map_err(|_| error());
        let bytes = bytes(text);
        Ok(Self {
            text: text.to_owned(),
            is_match: compile(match_source(&bytes))?,
            is_embedded_match: compile(embedded_source(&bytes))?,
        })
    }

    /// 表达式原文，即 `sub` / `unSub` 请求的 `path`。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 服务端是否会因为这个订阅投递该事件（`AsyncChannel::IsSubscribed`；服务端只按这一规则投递是
    /// [协议假设 S2](crate::docs::assumptions)）。
    pub fn delivers(&self, event: &EventType, path: &str) -> bool {
        let bytes = bytes(path);
        match event {
            EventType::EmbeddedUpdated => path == "/" || self.is_embedded_match.is_match(&bytes),
            _ => self.is_match.is_match(&bytes),
        }
    }
}

/// 服务端按字节匹配：R 的 `.` 只吃一个字节，E 的 `?` 只作用于前一个字节。
/// 把每个字节映射为 U+0000..=U+00FF（regex-lite 不把其中的非 ASCII 字符当作元字符），这里的匹配就同样按字节；
/// 纯 ASCII 时直接借用，不分配。
fn bytes(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.bytes().map(char::from).collect())
    }
}

/// R 的正则源串（`WildcardToRegex`）。服务端的 `FindAndReplaceAll` 从替换文本之后继续查找，等价于 `str::replace`。
fn match_source(text: &str) -> String {
    let source = wildcards(&escape(text)).replace('?', ".");
    match source.strip_suffix("/.*") {
        Some(head) => format!("{head}(/.*)?"),
        None => source,
    }
}

/// E 的正则源串（`WildcardToRegexEmbedded`）：不以 `/` 开头的表达式不加分组。
fn embedded_source(text: &str) -> String {
    let nested = if text.starts_with('/') {
        let levels = text.matches('/').count();
        format!("{}{})", text.replace('/', "(/"), ")?".repeat(levels - 1))
    } else {
        text.to_owned()
    };
    wildcards(&escape(&nested))
}

/// 转义服务端会转义的四个字符（`EscapeRegexHelper`），其余元字符由 [`SubscriptionExpr::new`] 拒绝。
fn escape(text: &str) -> String {
    text.replace('.', r"\.")
        .replace(':', r"\:")
        .replace('+', r"\+")
        .replace('$', r"\$")
}

/// `**` → `*` → `.*`（`ConvertRegexWildCards`）。
fn wildcards(text: &str) -> String {
    text.replace("**", "*").replace('*', ".*")
}
