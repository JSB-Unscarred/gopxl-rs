//! 网格（类型 18）：67 字节的属性段之后是 channelCount 个通道，每个通道是 26 字节的段加段外的数据。

use bytes::{Buf, Bytes};

use super::{Error, array, read_bool, read_items, read_vec, section, take, triple};

/// 网格（类型 18）。
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    /// 是否有数据。
    pub has_data: bool,
    /// 系统通道数（目前为 6）。
    pub system_channel_count: u32,
    /// 用户通道数的上限（目前为 5）。
    pub max_user_channel_count: u32,
    /// 用户通道数。
    pub user_channel_count: u32,
    /// 偏移 x、y、z。
    pub offset: [f64; 3],
    /// 范围 x、y、z。
    pub range: [f64; 3],
    /// 通道，按线上顺序；下标不等于通道 ID。
    pub channels: Vec<MeshChannel>,
}

/// 网格的一个通道。
#[derive(Clone, Debug, PartialEq)]
pub struct MeshChannel {
    /// 通道 ID：0 顶点、1 面、2 面法向、3 顶点法向、4 顶点纹理、5 顶点曲率，≥ 6 为用户通道。
    pub id: u32,
    /// 通道类型（0 无效、1 顶点 … 6 顶点曲率），原样保留。
    pub ty: u32,
    /// 通道状态，原样保留；取值的编号尚未确定（0 Error … 5 Full，或手册中的 −1..4）。
    pub state: i32,
    /// 标志。
    pub flag: u32,
    /// 已使用的元素数。
    pub used: u32,
    /// 通道数据：已分配的 allocateCount 个元素（不是 `used` 个）。
    pub data: MeshData,
}

/// 通道数据，元素类型由通道 ID 决定，不看通道类型。
#[derive(Clone, Debug, PartialEq)]
pub enum MeshData {
    /// ID 0、2、3：`f32` 的 x、y、z。
    Points(Vec<[f32; 3]>),
    /// ID 1：三个顶点下标。
    Facets(Vec<[u32; 3]>),
    /// ID 5：每顶点一个 `f32`。
    Curvature(Vec<f32>),
    /// ID 4（纹理）与用户通道：字节，帧的零拷贝切片。
    Bytes(Bytes),
}

impl Mesh {
    /// 读取属性段与各通道。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, Error> {
        let s = &mut section(buf, 2)?;
        let has_data = read_bool(s)?;
        let system_channel_count = s.try_get_u32_le()?;
        let max_user_channel_count = s.try_get_u32_le()?;
        let user_channel_count = s.try_get_u32_le()?;
        let channel_count = s.try_get_u32_le()?;
        Ok(Self {
            has_data,
            system_channel_count,
            max_user_channel_count,
            user_channel_count,
            offset: array(s, Bytes::try_get_f64_le)?,
            range: array(s, Bytes::try_get_f64_le)?,
            channels: read_items(buf, channel_count, MeshChannel::decode)?,
        })
    }
}

impl MeshChannel {
    /// 读取通道段与段外的 allocateCount 个元素。`read_vec` / `take` 先确认字节够再分配，巨大的 allocateCount
    /// 只得到 `Truncated`。
    fn decode(buf: &mut Bytes) -> Result<Self, Error> {
        let s = &mut section(buf, 2)?;
        let id = s.try_get_u32_le()?;
        let ty = s.try_get_u32_le()?;
        let state = s.try_get_i32_le()?;
        let flag = s.try_get_u32_le()?;
        let allocated = s.try_get_u32_le()? as usize;
        let used = s.try_get_u32_le()?;
        let data = match id {
            0 | 2 | 3 => {
                MeshData::Points(read_vec(buf, allocated, |b| triple(b, f32::from_le_bytes))?)
            }
            1 => MeshData::Facets(read_vec(buf, allocated, |b| triple(b, u32::from_le_bytes))?),
            5 => MeshData::Curvature(read_vec(buf, allocated, f32::from_le_bytes)?),
            _ => MeshData::Bytes(take(buf, allocated)?),
        };
        Ok(Self {
            id,
            ty,
            state,
            flag,
            used,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::gdp::{Body, Error, decode_message};

    /// 空白分隔的十六进制文本转为字节。
    fn hex(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    /// 手工构造的网格消息：顶点通道（allocateCount 2、usedCount 1）与 3 字节的用户通道。
    const F18: &str = "B6 00 00 00 12 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
        43 00 01 06 00 00 00 05 00 00 00 01 00 00 00 02 00 00 00 \
        00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
        00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 F0 3F \
        1A 00 00 00 00 00 01 00 00 00 03 00 00 00 00 00 00 00 02 00 00 00 01 00 00 00 \
        00 00 80 3F 00 00 00 40 00 00 40 40 00 00 00 00 00 00 00 00 00 00 00 00 \
        1A 00 06 00 00 00 00 00 00 00 05 00 00 00 34 12 00 00 03 00 00 00 03 00 00 00 AA BB CC";

    // 内存安全约定：声明巨大 allocateCount 的通道在分配之前报 Truncated，CAS 仍然有效。检查的是所有 GDP 数组共用的
    // `read_vec` / `take`：先确认字节够再分配。lint 只禁止 panic，拦不住按网络输入分配巨量内存。
    #[test]
    fn huge_allocate_count_is_truncated() {
        let mut bytes = hex(F18);
        bytes[121..125].fill(0xFF);
        let msg = decode_message(Bytes::from(bytes)).unwrap();
        assert!(msg.cas.is_last_msg);
        assert_eq!(msg.body, Body::Invalid(Error::Truncated));
    }
}
