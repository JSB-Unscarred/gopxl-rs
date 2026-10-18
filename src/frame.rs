//! 长度前缀帧的切分：控制通道的 carrier 帧与 GDP 消息都以“包含自身 4 字节的小端 `u32` 长度”开头。
//!
//! [`FrameReader`] 不做 I/O：驱动把收到的字节追加进 [`buffer`](FrameReader::buffer)，再用
//! [`next_frame`](FrameReader::next_frame) 切帧，任何读取方式都能驱动它（连接层是 tokio 的 `read_buf`，测试直接
//! 追加切片）。输入来自网络，以下约定防止坏帧拖垮进程：
//! - 长度先校验在 `[min, max]` 内再预留空间，声明 4 GiB 的坏帧不会触发巨量分配；
//! - 大于一块读取的帧按长度精确预留，到齐时整块内存就是这一帧，不复制也不与别的帧共享；小帧从读缓冲复制出来，
//!   读缓冲循环使用。返回的 `Bytes` 都独占自己的内存，解码得到的切片（`Value::Bin`、GDP 数据）只会让自己所在的
//!   帧存活。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::io;
use std::mem;

use bytes::{Buf, Bytes, TryGetError};

/// 取出 `n` 字节的零拷贝切片，不够时报 `TryGetError`（各编解码器把它转成自己的“截断”错误）。帧与消息体里
/// 所有按长度切片的读取都经过它，先检查再切，坏长度不会越界。
pub(crate) fn take(buf: &mut Bytes, n: usize) -> Result<Bytes, TryGetError> {
    if n <= buf.len() {
        Ok(buf.split_to(n))
    } else {
        Err(TryGetError {
            requested: n,
            available: buf.len(),
        })
    }
}

/// [`FrameReader::next_frame`] 与驱动的错误。出错后字节流已经错位，连接只能关闭。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// 读取失败、读取超时（`TimedOut`），或帧在中途遇到 EOF（`UnexpectedEof`）；由驱动在读取出错时构造。
    #[error(transparent)]
    Io(#[from] io::Error),
    /// 长度字段（携带其值）不在构造读取器时给出的 `[min, max]` 内。
    #[error("frame length {0} is out of range")]
    Length(u32),
}

/// 一次读取预留的空间：小帧多的连接（GDP 的测量值、通知）一次读进多帧；长度超过它的帧按长度精确预留。
const READ_CHUNK: usize = 8 * 1024;

/// 把字节流切成帧，每帧是一个包含长度字段在内、独占内存的 `Bytes`。
#[derive(Debug)]
pub struct FrameReader {
    min: u32,
    max: u32,
    /// 已收到的字节：`start` 之前是已切出的帧，之后是当前帧的开头。
    buf: Vec<u8>,
    start: usize,
}

impl FrameReader {
    /// `min` 是协议的帧头长度（不足即坏帧，小于 4 时取 4，即长度字段本身），`max` 是连接愿意为一帧分配的上限。
    pub fn new(min: u32, max: u32) -> Self {
        Self {
            min: min.max(4),
            max,
            buf: Vec::new(),
            start: 0,
        }
    }

    /// 当前帧的长度字段：还没收到 4 字节时为 `None`，越界时为 `Err`。
    fn declared(&self) -> Option<Result<usize, FrameError>> {
        let mut head = self.buf.get(self.start..)?;
        let len = head.try_get_u32_le().ok()?;
        Some(if (self.min..=self.max).contains(&len) {
            Ok(len as usize)
        } else {
            Err(FrameError::Length(len))
        })
    }

    /// 供驱动追加字节的缓冲区，在 [`next_frame`](Self::next_frame) 返回 `None` 之后调用。已切出的帧在这里丢弃，
    /// 剩下的字节不足一帧，每次读取之前最多把它们搬到缓冲开头一次；长度已知且超过一块读取时按长度精确预留，否则
    /// 预留一块。
    pub fn buffer(&mut self) -> &mut Vec<u8> {
        self.buf.drain(..self.start);
        self.start = 0;
        match self.declared() {
            Some(Ok(len)) if len > READ_CHUNK => {
                self.buf.reserve_exact(len.saturating_sub(self.buf.len()));
            }
            _ => self.buf.reserve(READ_CHUNK),
        }
        &mut self.buf
    }

    /// 切出下一帧；字节不够时为 `Ok(None)`。
    pub fn next_frame(&mut self) -> Result<Option<Bytes>, FrameError> {
        let Some(len) = self.declared().transpose()? else {
            return Ok(None);
        };
        let Some(end) = self.start.checked_add(len) else {
            return Ok(None);
        };
        if self.buf.len() < end {
            return Ok(None);
        }
        if self.start == 0 && len > READ_CHUNK {
            // 大帧：整块内存就是这一帧；后面已到达的少量字节（预留过量时才有）搬到新缓冲。
            let rest = self.buf.split_off(len);
            return Ok(Some(Bytes::from(mem::replace(&mut self.buf, rest))));
        }
        let Some(frame) = self.buf.get(self.start..end) else {
            return Ok(None);
        };
        let frame = Bytes::copy_from_slice(frame);
        self.start = end;
        Ok(Some(frame))
    }

    /// 已收到的字节是否都已切成帧，即 EOF 落在帧边界上。
    pub fn is_empty(&self) -> bool {
        self.start == self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameError, FrameReader, READ_CHUNK};

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(body.len() + 4)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        out.extend_from_slice(body);
        out
    }

    // 增量约定：逐字节追加与整块追加得到相同的帧，小帧（复制）与大帧（独占缓冲）各走一条路径，切完后缓冲为空。
    // 真实 socket 上难以复现半帧，出错时静默丢字节、帧错位。
    #[test]
    fn trickled_and_bulk_input_yield_the_same_frames() {
        let frames = [
            frame(b"ab"),
            frame(b""),
            frame(&[7; READ_CHUNK + 1]),
            frame(b"tail"),
        ];
        let input = frames.concat();
        let mut trickled = FrameReader::new(4, 1 << 20);
        let mut got = Vec::new();
        for &byte in &input {
            trickled.buffer().push(byte);
            while let Some(f) = trickled.next_frame().unwrap() {
                got.push(f.to_vec());
            }
        }
        assert_eq!(got, frames);
        assert!(trickled.is_empty());

        let mut bulk = FrameReader::new(4, 1 << 20);
        bulk.buffer().extend_from_slice(&input);
        let mut got = Vec::new();
        while let Some(f) = bulk.next_frame().unwrap() {
            got.push(f.to_vec());
        }
        assert_eq!(got, frames);
        assert!(bulk.is_empty());
    }

    // 内存安全约定：声明 4 GiB 的长度在预留空间之前报错，buffer() 也不按它预留。
    // lint 只禁止 panic，拦不住按网络输入分配巨量内存。
    #[test]
    fn huge_length_is_rejected_before_allocation() {
        let mut reader = FrameReader::new(14, 1024);
        reader.buffer().extend_from_slice(&[0xFF; 4]);
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Length(u32::MAX))
        ));
        assert!(reader.buffer().capacity() < 1 << 20);
    }
}
