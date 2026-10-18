//! MessagePack 编解码：控制通道消息体（carrier 类型 0xB000）的格式。
//!
//! 编码规则固定，同一个值总是得到同样的字节，便于抓包比对：整数用最短格式，浮点数一律写 float64（`CB`），对象的
//! 键按字节序（[`Map`] 本身有序）。解码接受任意整数宽度和键序：服务端按字段的类型选宽度（正的 32 位有符号整数
//! 也写成 `D2`，31 字节的字符串写成 `D9 1F`），键按插入顺序。
//!
//! 解码的输入来自网络，以下约定保证坏帧只会得到 `Err`：
//! - 嵌套深度上限 [`MAX_DEPTH`]，递归不会栈溢出（栈溢出直接终止进程）；
//! - 数组的预分配不超过剩余字节数，声明 2³²−1 个元素的坏帧不会触发巨量分配；
//! - 读取都经过 `bytes::Buf` 的 `try_get_*` 与带长度检查的 `take`，下面的 lint 禁止下标与 unwrap，
//!   读任务不会因为坏帧 panic。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use bytes::{Buf, Bytes, TryGetError};

use super::{Map, Value};
use crate::frame::take;

/// 解码允许的最大嵌套层数，与 `serde_json` 的递归上限相同。
pub const MAX_DEPTH: usize = 128;

/// [`encode`] 的错误：字符串、二进制、数组或对象的长度超过 `u32::MAX`，MessagePack 无法表示。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("MessagePack length exceeds u32::MAX")]
pub struct EncodeError;

/// [`decode`] 的错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// 输入在一个值的中途结束，或声明的长度超出输入。
    #[error("MessagePack input ends in the middle of a value")]
    Truncated,
    /// 保留的 marker `0xC1`。
    #[error("reserved MessagePack marker 0xC1")]
    ReservedMarker,
    /// 对象的键不是字符串。
    #[error("MessagePack map key is not a string")]
    NonStringKey,
    /// 嵌套超过 [`MAX_DEPTH`] 层。
    #[error("MessagePack nesting exceeds {} levels", MAX_DEPTH)]
    TooDeep,
    /// 顶层值之后还有字节。
    #[error("trailing bytes after the MessagePack value")]
    TrailingBytes,
}

/// 越界读取都意味着输入在值的中途结束。
impl From<TryGetError> for DecodeError {
    fn from(_: TryGetError) -> Self {
        Self::Truncated
    }
}

/// 把 `value` 编码后追加到 `out`。出错时 `out` 末尾可能留有部分输出，应整体丢弃。
pub fn encode(value: &Value, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    match value {
        Value::Nil => out.push(0xC0),
        Value::Bool(v) => out.push(if *v { 0xC3 } else { 0xC2 }),
        Value::Int(v) => match u64::try_from(*v) {
            Ok(v) => write_uint(out, v),
            Err(_) => write_negative(out, *v),
        },
        Value::UInt(v) => write_uint(out, *v),
        Value::Float(v) => put(out, 0xCB, &v.to_be_bytes()),
        Value::Str(v) => write_str(out, v)?,
        Value::Bin(v) => {
            write_len(out, &BIN, v.len())?;
            out.extend_from_slice(v);
        }
        Value::Array(items) => {
            write_len(out, &ARRAY, items.len())?;
            for item in items {
                encode(item, out)?;
            }
        }
        Value::Map(map) => {
            write_len(out, &MAP, map.len())?;
            for (key, value) in map {
                write_str(out, key)?;
                encode(value, out)?;
            }
        }
    }
    Ok(())
}

/// 写出 marker 与紧随其后的大端字段。
fn put(out: &mut Vec<u8>, marker: u8, field: &[u8]) {
    out.push(marker);
    out.extend_from_slice(field);
}

/// 非负整数的最短格式：fixint、`CC`、`CD`、`CE`、`CF`。
fn write_uint(out: &mut Vec<u8>, v: u64) {
    if let Ok(n) = u8::try_from(v) {
        if n >= 0x80 {
            out.push(0xCC);
        }
        out.push(n);
    } else if let Ok(n) = u16::try_from(v) {
        put(out, 0xCD, &n.to_be_bytes());
    } else if let Ok(n) = u32::try_from(v) {
        put(out, 0xCE, &n.to_be_bytes());
    } else {
        put(out, 0xCF, &v.to_be_bytes());
    }
}

/// 负整数的最短格式：−32..−1 用负 fixint，其余用 `D0`、`D1`、`D2`、`D3`。
fn write_negative(out: &mut Vec<u8>, v: i64) {
    if let Ok(n) = i8::try_from(v) {
        if n < -32 {
            out.push(0xD0);
        }
        out.extend_from_slice(&n.to_be_bytes());
    } else if let Ok(n) = i16::try_from(v) {
        put(out, 0xD1, &n.to_be_bytes());
    } else if let Ok(n) = i32::try_from(v) {
        put(out, 0xD2, &n.to_be_bytes());
    } else {
        put(out, 0xD3, &v.to_be_bytes());
    }
}

fn write_str(out: &mut Vec<u8>, v: &str) -> Result<(), EncodeError> {
    write_len(out, &STR, v.len())?;
    out.extend_from_slice(v.as_bytes());
    Ok(())
}

/// 一族长度头：fix 格式的基值与长度上限，以及 8 / 16 / 32 位长度的 marker；不存在的格式为 `None`。
struct Family {
    fix: Option<(u8, u8)>,
    len8: Option<u8>,
    len16: u8,
    len32: u8,
}

const STR: Family = Family {
    fix: Some((0xA0, 31)),
    len8: Some(0xD9),
    len16: 0xDA,
    len32: 0xDB,
};
const BIN: Family = Family {
    fix: None,
    len8: Some(0xC4),
    len16: 0xC5,
    len32: 0xC6,
};
const ARRAY: Family = Family {
    fix: Some((0x90, 15)),
    len8: None,
    len16: 0xDC,
    len32: 0xDD,
};
const MAP: Family = Family {
    fix: Some((0x80, 15)),
    len8: None,
    len16: 0xDE,
    len32: 0xDF,
};

/// 写出 `family` 中能容纳 `len` 的最短长度头。
fn write_len(out: &mut Vec<u8>, family: &Family, len: usize) -> Result<(), EncodeError> {
    match (family.fix, family.len8, u8::try_from(len)) {
        (Some((base, max)), _, Ok(n)) if n <= max => out.push(base | n),
        (_, Some(marker), Ok(n)) => put(out, marker, &[n]),
        _ => {
            if let Ok(n) = u16::try_from(len) {
                put(out, family.len16, &n.to_be_bytes());
            } else {
                let n = u32::try_from(len).map_err(|_| EncodeError)?;
                put(out, family.len32, &n.to_be_bytes());
            }
        }
    }
    Ok(())
}

/// 解码一条完整的 MessagePack 文档。
///
/// - `Bin` 是 `body` 的零拷贝切片，持有它会让整个 `body` 存活；
/// - 整数归一化（见 [`Value`]），float32 拓宽为 `f64`；
/// - 非法 UTF-8 的字符串替换为 U+FFFD：服务端的字符串不保证 UTF-8，不应让整条消息失败；
/// - 重复的键后者覆盖前者；
/// - ext 当作 `Bin` 并丢掉类型字节（服务端不发送 ext，容忍即可）。
pub fn decode(body: &Bytes) -> Result<Value, DecodeError> {
    let mut buf = body.clone();
    let value = read_value(&mut buf, 0)?;
    if buf.has_remaining() {
        Err(DecodeError::TrailingBytes)
    } else {
        Ok(value)
    }
}

fn read_value(buf: &mut Bytes, depth: usize) -> Result<Value, DecodeError> {
    let marker = buf.try_get_u8()?;
    // 变长格式按 marker 依次使用 1、2、4 字节的长度字段（数组与对象只有 2、4 字节），用移位算出宽度。
    Ok(match marker {
        0x00..=0x7F => Value::Int(marker.into()),
        0x80..=0x8F => read_map(buf, (marker & 0x0F).into(), depth)?,
        0x90..=0x9F => read_array(buf, (marker & 0x0F).into(), depth)?,
        0xA0..=0xBF => read_str(buf, (marker & 0x1F).into())?,
        0xC0 => Value::Nil,
        0xC1 => return Err(DecodeError::ReservedMarker),
        0xC2 => Value::Bool(false),
        0xC3 => Value::Bool(true),
        0xC4..=0xC6 => {
            let n = read_len(buf, 1 << (marker - 0xC4))?;
            Value::Bin(take(buf, n)?)
        }
        // ext 8 / 16 / 32：长度、类型字节、数据
        0xC7..=0xC9 => {
            let n = read_len(buf, 1 << (marker - 0xC7))?;
            buf.try_get_u8()?;
            Value::Bin(take(buf, n)?)
        }
        0xCA => Value::Float(buf.try_get_f32()?.into()),
        0xCB => Value::Float(buf.try_get_f64()?),
        0xCC => Value::Int(buf.try_get_u8()?.into()),
        0xCD => Value::Int(buf.try_get_u16()?.into()),
        0xCE => Value::Int(buf.try_get_u32()?.into()),
        0xCF => Value::from(buf.try_get_u64()?),
        0xD0 => Value::Int(buf.try_get_i8()?.into()),
        0xD1 => Value::Int(buf.try_get_i16()?.into()),
        0xD2 => Value::Int(buf.try_get_i32()?.into()),
        0xD3 => Value::Int(buf.try_get_i64()?),
        // fixext 1 / 2 / 4 / 8 / 16：类型字节、数据
        0xD4..=0xD8 => {
            buf.try_get_u8()?;
            Value::Bin(take(buf, 1 << (marker - 0xD4))?)
        }
        0xD9..=0xDB => {
            let n = read_len(buf, 1 << (marker - 0xD9))?;
            read_str(buf, n)?
        }
        0xDC | 0xDD => {
            let n = read_len(buf, 2 << (marker - 0xDC))?;
            read_array(buf, n, depth)?
        }
        0xDE | 0xDF => {
            let n = read_len(buf, 2 << (marker - 0xDE))?;
            read_map(buf, n, depth)?
        }
        0xE0..=0xFF => Value::Int(i8::from_be_bytes([marker]).into()),
    })
}

/// 读取 `width` 字节的大端长度字段。
fn read_len(buf: &mut Bytes, width: usize) -> Result<usize, DecodeError> {
    Ok(usize::try_from(buf.try_get_uint(width)?).unwrap_or(usize::MAX))
}

fn read_str(buf: &mut Bytes, n: usize) -> Result<Value, DecodeError> {
    Ok(Value::Str(
        String::from_utf8_lossy(&take(buf, n)?).into_owned(),
    ))
}

fn read_array(buf: &mut Bytes, n: usize, depth: usize) -> Result<Value, DecodeError> {
    let depth = enter(depth)?;
    // 每个元素至少 1 字节，剩余字节数是安全的预分配上界。
    let mut items = Vec::with_capacity(n.min(buf.remaining()));
    for _ in 0..n {
        items.push(read_value(buf, depth)?);
    }
    Ok(Value::Array(items))
}

fn read_map(buf: &mut Bytes, n: usize, depth: usize) -> Result<Value, DecodeError> {
    let depth = enter(depth)?;
    let mut map = Map::new();
    for _ in 0..n {
        let Value::Str(key) = read_value(buf, depth)? else {
            return Err(DecodeError::NonStringKey);
        };
        map.insert(key, read_value(buf, depth)?);
    }
    Ok(Value::Map(map))
}

/// 进入一层容器，返回子元素的深度。
fn enter(depth: usize) -> Result<usize, DecodeError> {
    if depth < MAX_DEPTH {
        Ok(depth + 1)
    } else {
        Err(DecodeError::TooDeep)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{DecodeError, MAX_DEPTH, decode};
    use crate::value::Value;

    fn decoded(bytes: &[u8]) -> Result<Value, DecodeError> {
        decode(&Bytes::copy_from_slice(bytes))
    }

    // 内存安全约定：深度上限内的嵌套可以解码，超过一层即报错；声明巨大长度的短输入报 Truncated，不预分配。
    // lint 只禁止 panic，拦不住递归栈溢出与按网络输入分配巨量内存。
    #[test]
    fn decode_rejects_depth_and_length_bombs() {
        let nested = |levels: usize| {
            let mut bytes = vec![0x91; levels - 1];
            bytes.push(0x90);
            bytes
        };
        assert!(decoded(&nested(MAX_DEPTH)).is_ok());
        assert_eq!(decoded(&nested(MAX_DEPTH + 1)), Err(DecodeError::TooDeep));

        for marker in [0xDD, 0xDF, 0xDB, 0xC6] {
            assert_eq!(
                decoded(&[marker, 0xFF, 0xFF, 0xFF, 0xFF]),
                Err(DecodeError::Truncated)
            );
        }
    }
}
