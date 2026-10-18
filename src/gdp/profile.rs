//! 轮廓（类型 12、13）：46 字节的属性段之后是 W 个点（段外），再跟 iW 字节强度。

use bytes::{Buf, Bytes};

use super::{Error, read_vec, section, take};

/// 轮廓属性（C++ `GoGdpProfileBase`）。毫米坐标为 `offset + scale × 原始值`，SDK 不提供换算。
/// C++ 的 `Width`、`IntensityWidth` 即点数组与强度的长度。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileAttrs {
    /// X 分辨率（mm）；统一轮廓的 x 原始值是点的下标。
    pub x_scale: f64,
    /// Z 分辨率（mm）。
    pub z_scale: f64,
    /// X 偏移（mm）。
    pub x_offset: f64,
    /// Z 偏移（mm）。
    pub z_offset: f64,
    /// 曝光，原样透传（手册为 µs，−1 表示不可用）。
    pub exposure: f32,
}

/// 统一轮廓（类型 12，`GoGdpProfileUniform`）。
#[derive(Clone, Debug, PartialEq)]
pub struct UniformProfile {
    /// 属性。
    pub attrs: ProfileAttrs,
    /// W 个 z 原始值；`i16::MIN`（`k16S_NULL`）表示无效。
    pub ranges: Vec<i16>,
    /// iW 字节强度，帧的零拷贝切片；不带强度时为空。
    pub intensity: Bytes,
}

/// 轮廓点云（类型 13，`GoGdpProfilePointCloud`）。
#[derive(Clone, Debug, PartialEq)]
pub struct ProfilePointCloud {
    /// 属性。
    pub attrs: ProfileAttrs,
    /// W 个 `(x, z)` 原始值；x 为 `i16::MIN` 表示无效（与 SDK 示例相同）。
    pub points: Vec<[i16; 2]>,
    /// iW 字节强度，帧的零拷贝切片；不带强度时为空。
    pub intensity: Bytes,
}

impl ProfileAttrs {
    /// 读取属性段，另返回点数 W 与强度字节数 iW。
    fn decode(buf: &mut Bytes) -> Result<(Self, usize, usize), Error> {
        let s = &mut section(buf, 2)?;
        let width = s.try_get_u32_le()? as usize;
        let intensity_width = s.try_get_u32_le()? as usize;
        let attrs = Self {
            x_scale: s.try_get_f64_le()?,
            z_scale: s.try_get_f64_le()?,
            x_offset: s.try_get_f64_le()?,
            z_offset: s.try_get_f64_le()?,
            exposure: s.try_get_f32_le()?,
        };
        Ok((attrs, width, intensity_width))
    }
}

impl UniformProfile {
    /// 读取属性段与段外的数据。数据不足报 `Truncated`（C++ 的 `kERROR_INCOMPLETE`）。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, Error> {
        let (attrs, width, intensity_width) = ProfileAttrs::decode(buf)?;
        Ok(Self {
            attrs,
            ranges: read_vec(buf, width, i16::from_le_bytes)?,
            intensity: take(buf, intensity_width)?,
        })
    }
}

impl ProfilePointCloud {
    /// 读取属性段与段外的数据。数据不足报 `Truncated`。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, Error> {
        let (attrs, width, intensity_width) = ProfileAttrs::decode(buf)?;
        Ok(Self {
            attrs,
            points: read_vec(buf, width, |[a, b, c, d]| {
                [i16::from_le_bytes([a, b]), i16::from_le_bytes([c, d])]
            })?,
            intensity: take(buf, intensity_width)?,
        })
    }
}
