//! 路径片段的百分号编码。
//!
//! 用户用它构造 `extId=` 段；请求本身原样发送路径，不经过这里。路径里的空格写成 `%20`，不用表单编码的 `+`。

use std::fmt::Write;

/// 编码：ASCII 字母数字与 `- _ . ! ~ * ' ( )` 原样保留，其余每个字节写成大写的 `%XX`（空格为 `%20`）。
///
/// 只按 ASCII 判断，与 locale 无关：≥ 0x80 的字节一律编码。
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            // 写入 String 不会失败。
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// 解码：`%` 后跟两个十六进制数字（大小写均可）时得到该字节，否则 `%` 原样保留、后面的字符照常处理；`+` 不变。
///
/// 输入以 `%` 或 `%X` 结尾时 `%` 原样保留。结果可能不是 UTF-8（如 `%FF`），所以返回字节。
pub fn url_decode(s: &str) -> Vec<u8> {
    let hex = |b: u8| {
        char::from(b)
            .to_digit(16)
            .and_then(|d| u8::try_from(d).ok())
    };
    let mut out = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        let escaped = match tail {
            [hi, lo, after @ ..] if b == b'%' => hex(*hi)
                .zip(hex(*lo))
                .map(|(hi, lo)| ((hi << 4) | lo, after)),
            _ => None,
        };
        let (byte, after) = escaped.unwrap_or((b, tail));
        out.push(byte);
        rest = after;
    }
    out
}
