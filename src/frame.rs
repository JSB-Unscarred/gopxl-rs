//! 长度前缀帧的读取：控制通道的 carrier 帧与 GDP 消息都以“包含自身 4 字节的小端 `u32` 长度”开头。
//!
//! 输入来自网络，以下约定防止坏帧拖垮进程：
//! - 长度先校验在 `[min, max]` 内再分配，声明 4 GiB 的坏帧不会触发巨量分配；
//! - 每帧按长度精确分配，返回的 `Bytes` 独占这块内存：解码得到的切片（`Value::Bin`、GDP 数据）只会让自己所在的帧
//!   存活，不会钉住共享的读缓冲；
//! - 已读的字节保存在读取器里：非阻塞 socket 返回 `WouldBlock` 后再次调用，从断点继续（GDP 接收用非阻塞 socket）。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::io::{self, Read};

use bytes::{Buf, Bytes};

/// [`FrameReader::read`] 的错误。出错后字节流已经错位，连接只能关闭。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// 读取失败，或帧在中途遇到 EOF（`UnexpectedEof`）。`WouldBlock` 不丢失已读的字节。
    #[error(transparent)]
    Io(#[from] io::Error),
    /// 长度字段（携带其值）不在构造读取器时给出的 `[min, max]` 内。
    #[error("frame length {0} is out of range")]
    Length(u32),
}

/// 从字节流中逐帧读取，每帧是一个包含长度字段在内的完整 `Bytes`。
///
/// 每帧至少两次 `read`（长度字段与帧体），小帧多的连接（GDP）应把 socket 包进 `BufReader`；
/// 大帧的帧体会绕过 `BufReader` 的缓冲区直接读入，不多一次复制。
#[derive(Debug)]
pub struct FrameReader {
    min: u32,
    max: u32,
    /// 当前帧已读到的字节。
    frame: Vec<u8>,
}

impl FrameReader {
    /// `min` 是协议的帧头长度（不足即坏帧），`max` 是连接愿意为一帧分配的上限。
    pub fn new(min: u32, max: u32) -> Self {
        Self {
            min: min.max(4),
            max,
            frame: Vec::new(),
        }
    }

    /// 读取下一帧。在帧边界上遇到 EOF 返回 `Ok(None)`。
    pub fn read(&mut self, r: &mut impl Read) -> Result<Option<Bytes>, FrameError> {
        if self.frame.len() < 4 {
            fill(r, &mut self.frame, 4)?;
            if self.frame.is_empty() {
                return Ok(None);
            }
        }
        let len = self
            .frame
            .as_slice()
            .try_get_u32_le()
            .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        if !(self.min..=self.max).contains(&len) {
            return Err(FrameError::Length(len));
        }
        let len = len as usize;
        self.frame
            .reserve_exact(len.saturating_sub(self.frame.len()));
        fill(r, &mut self.frame, len)?;
        Ok(Some(Bytes::from(std::mem::take(&mut self.frame))))
    }
}

/// 把 `buf` 读到 `len` 字节。一个字节都没读到就遇到 EOF 时返回 `Ok`、`buf` 不变（只可能发生在帧边界）；
/// 读到一部分后 EOF 报 `UnexpectedEof`。
fn fill(r: &mut impl Read, buf: &mut Vec<u8>, len: usize) -> io::Result<()> {
    let start = buf.len();
    let want = u64::try_from(len.saturating_sub(start)).unwrap_or(u64::MAX);
    // read_to_end 遇到 WouldBlock 等错误时，已读的字节也留在 buf 里。
    r.take(want).read_to_end(buf)?;
    if buf.len() < len && !buf.is_empty() {
        Err(io::ErrorKind::UnexpectedEof.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::{FrameError, FrameReader};

    /// 每次只交出一个字节，并在每个字节之前先返回一次 WouldBlock，模拟最碎的非阻塞读取。
    struct Trickle {
        bytes: Vec<u8>,
        pos: usize,
        blocked: bool,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.blocked = !self.blocked;
            if self.blocked {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            match (self.bytes.get(self.pos), buf.first_mut()) {
                (Some(&b), Some(slot)) => {
                    *slot = b;
                    self.pos += 1;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(body.len() + 4)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        out.extend_from_slice(body);
        out
    }

    // 增量读取约定：逐字节、穿插 WouldBlock 的输入得到与整块输入相同的帧，帧边界上的 EOF 是 None。
    // 保留原因：真实 socket 上难以复现半帧，出错时静默丢字节、帧错位。
    #[test]
    fn trickled_input_yields_the_same_frames() {
        let frames = [frame(b"ab"), frame(b""), frame(&[7; 300])];
        let mut reader = Trickle {
            bytes: frames.concat(),
            pos: 0,
            blocked: false,
        };
        let mut decoder = FrameReader::new(4, 1024);
        let mut got = Vec::new();
        loop {
            match decoder.read(&mut reader) {
                Ok(Some(f)) => got.push(f.to_vec()),
                Ok(None) => break,
                Err(FrameError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(got, frames);
    }

    // 内存安全约定：声明 4 GiB 的长度在分配之前报错。
    // 保留原因：lint 只禁止 panic，拦不住按网络输入分配巨量内存。
    #[test]
    fn huge_length_is_rejected_before_allocation() {
        let huge = [0xFF_u8; 4];
        assert!(matches!(
            FrameReader::new(14, 1024).read(&mut &huge[..]),
            Err(FrameError::Length(u32::MAX))
        ));
    }
}
