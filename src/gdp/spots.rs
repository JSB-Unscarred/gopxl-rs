//! 光斑（类型 17）：39 字节的属性段之后是 spotCount 个 6 字节的光斑（段外）。

use bytes::{Buf, Bytes};

use super::{GdpError, read_bool, read_vec, section};

/// 光斑（类型 17，`GoGdpSpots`），坐标是像素。
///
/// `column_based` 时 `x = slice × slice_scale + slice_offset`、`y = center × center_scale + center_offset`，
/// 否则两轴对调；比例为负表示该轴翻转。
#[derive(Clone, Debug, PartialEq)]
pub struct Spots {
    /// 曝光，原样透传。
    pub exposure: f32,
    /// 按列扫描（slice 是列）；否则按行。
    pub column_based: bool,
    /// slice 的比例。
    pub slice_scale: f32,
    /// slice 的偏移。
    pub slice_offset: f32,
    /// center 的比例。
    pub center_scale: f32,
    /// center 的偏移。
    pub center_offset: f32,
    /// 最大 slice 数（按行时为图像高度，按列时为宽度）。
    pub max_slice_count: u32,
    /// center 原始值的下限。
    pub center_min: u32,
    /// center 原始值的上限。
    pub center_max: u32,
    /// 光斑；C++ 的 `PointCount` 即 `spots.len()`。
    pub spots: Vec<Spot>,
}

/// 一个光斑（C++ `GdpSpot`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    /// 所在的 slice（行或列）。
    pub slice: u16,
    /// slice 内的中心原始值。
    pub center: u32,
}

impl Spots {
    /// 读取属性段与段外的光斑。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        let count = s.try_get_u32_le()? as usize;
        Ok(Self {
            exposure: s.try_get_f32_le()?,
            column_based: read_bool(s)?,
            slice_scale: s.try_get_f32_le()?,
            slice_offset: s.try_get_f32_le()?,
            center_scale: s.try_get_f32_le()?,
            center_offset: s.try_get_f32_le()?,
            max_slice_count: s.try_get_u32_le()?,
            center_min: s.try_get_u32_le()?,
            center_max: s.try_get_u32_le()?,
            spots: read_vec(buf, count, |[a, b, c, d, e, f]| Spot {
                slice: u16::from_le_bytes([a, b]),
                center: u32::from_le_bytes([c, d, e, f]),
            })?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Spot, Spots};
    use crate::gdp::Body;
    use crate::gdp::tests::decode_fixture;
    use crate::hex;

    // F17（derived）：属性段之后是紧密排列、不对齐的 (u16 slice, u32 center)。
    #[test]
    fn spots_fixture() {
        let f17 = "57 00 00 00 11 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
            27 00 02 00 00 00 00 00 C8 42 01 00 00 80 3F 00 00 00 00 00 00 00 3F 00 00 00 00 E0 01 00 00 00 00 00 00 \
            80 02 00 00 05 00 64 00 00 00 06 00 C8 00 00 00";
        let expected = Spots {
            exposure: 100.0,
            column_based: true,
            slice_scale: 1.0,
            slice_offset: 0.0,
            center_scale: 0.5,
            center_offset: 0.0,
            max_slice_count: 480,
            center_min: 0,
            center_max: 640,
            spots: vec![
                Spot {
                    slice: 5,
                    center: 100,
                },
                Spot {
                    slice: 6,
                    center: 200,
                },
            ],
        };
        assert_eq!(decode_fixture(&hex(f17)).body, Body::Spots(expected));
    }
}
