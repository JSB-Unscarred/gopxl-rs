//! 轮廓（类型 12、13）：46 字节的属性段之后是 W 个点（段外），再跟 iW 字节强度。

use bytes::{Buf, Bytes};

use super::{GdpError, read_vec, section, take};

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
    fn decode(buf: &mut Bytes) -> Result<(Self, usize, usize), GdpError> {
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
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
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
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{ProfileAttrs, ProfilePointCloud, UniformProfile};
    use crate::gdp::Body;
    use crate::gdp::tests::decode_fixture;
    use crate::hex;

    // F12 / F13（derived）：统一轮廓的 W 个 i16 与 iW 字节强度；点云的 (x, z) 交错排列，iW = 0 时强度为空。
    #[test]
    fn profile_fixtures() {
        let f12 = "5B 00 00 00 0C 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
            2E 00 03 00 00 00 03 00 00 00 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 F0 BF \
            00 00 00 00 00 00 24 40 00 00 C8 42 00 00 00 80 04 00 0A 14 1E";
        let f13 = "5A 00 00 00 0D 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
            2E 00 02 00 00 00 00 00 00 00 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 F0 BF \
            00 00 00 00 00 00 24 40 00 00 C8 42 02 00 04 00 00 80 00 00";
        let attrs = ProfileAttrs {
            x_scale: 0.5,
            z_scale: 0.25,
            x_offset: -1.0,
            z_offset: 10.0,
            exposure: 100.0,
        };
        let uniform = Body::UniformProfile(UniformProfile {
            attrs,
            ranges: vec![0, i16::MIN, 4],
            intensity: Bytes::from_static(&[10, 20, 30]),
        });
        let cloud = Body::ProfilePointCloud(ProfilePointCloud {
            attrs,
            points: vec![[2, 4], [i16::MIN, 0]],
            intensity: Bytes::new(),
        });
        for (fixture, expected) in [(f12, uniform), (f13, cloud)] {
            assert_eq!(decode_fixture(&hex(fixture)).body, expected);
        }
    }
}
