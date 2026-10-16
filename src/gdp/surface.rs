//! 表面（类型 14、15）：属性段之后是 L×W 个点（段外，行主序），再跟 iL 行强度。

use bytes::{Buf, Bytes};

use super::{GdpError, PixelFormat, array, optional, product, read_bool, read_vec, section, take};

/// 表面属性（C++ `GoGdpSurfaceBase`）。毫米坐标为 `offset + scale × 原始值`（统一表面的 x、y 原始值是列、行下标），
/// SDK 不提供换算。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceAttrs {
    /// 行数 L。
    pub length: u32,
    /// 列数 W。
    pub width: u32,
    /// 强度行数 iL，与 L 无约定关系。
    pub intensity_length: u32,
    /// 每行强度像素数 iW，与 W 无约定关系。
    pub intensity_width: u32,
    /// X 分辨率（mm）。
    pub x_scale: f64,
    /// Y 分辨率（mm）。
    pub y_scale: f64,
    /// Z 分辨率（mm）。
    pub z_scale: f64,
    /// X 偏移（mm）。
    pub x_offset: f64,
    /// Y 偏移（mm）。
    pub y_offset: f64,
    /// Z 偏移（mm）。
    pub z_offset: f64,
    /// 表面 ID，数据变化时改变。
    pub surface_id: u32,
    /// 曝光，原样透传。
    pub exposure: f32,
    /// 强度的像素格式（1.5 新增）：有强度（iL、iW 都非 0）时，段内剩余 ≥ 4 字节才读取，否则为旧式的
    /// `GREYSCALE_8BPP`；不带强度时为 `NULL`。
    pub intensity_format: PixelFormat,
}

impl SurfaceAttrs {
    /// 每行强度的字节数（C++ `IntensityRowSize`）：旧式格式每像素 1 字节（包括 16 位与 BGRX），
    /// PFNC 格式为 `ceil(iW × 位数 / 8)`。
    pub fn intensity_row_size(&self) -> usize {
        if self.intensity_format.is_legacy() {
            self.intensity_width as usize
        } else {
            self.intensity_format.row_bytes(self.intensity_width)
        }
    }

    /// 读取属性段。点云在 exposure 之后、强度格式之前多一个 isAdjacent，一并返回（统一表面为 false）。
    fn decode(buf: &mut Bytes, point_cloud: bool) -> Result<(Self, bool), GdpError> {
        let s = &mut section(buf, 2)?;
        let length = s.try_get_u32_le()?;
        let width = s.try_get_u32_le()?;
        let intensity_length = s.try_get_u32_le()?;
        let intensity_width = s.try_get_u32_le()?;
        let [x_scale, y_scale, z_scale] = array(s, Bytes::try_get_f64_le)?;
        let [x_offset, y_offset, z_offset] = array(s, Bytes::try_get_f64_le)?;
        let surface_id = s.try_get_u32_le()?;
        let exposure = s.try_get_f32_le()?;
        let is_adjacent = point_cloud && read_bool(s)?;
        let intensity_format = if intensity_length > 0 && intensity_width > 0 {
            optional(s, Bytes::try_get_i32_le).map_or(PixelFormat::GREYSCALE_8BPP, PixelFormat)
        } else {
            PixelFormat::NULL
        };
        let attrs = Self {
            length,
            width,
            intensity_length,
            intensity_width,
            x_scale,
            y_scale,
            z_scale,
            x_offset,
            y_offset,
            z_offset,
            surface_id,
            exposure,
            intensity_format,
        };
        Ok((attrs, is_adjacent))
    }

    /// 点数 L×W。
    fn point_count(&self) -> Result<usize, GdpError> {
        product(self.length as usize, self.width as usize)
    }

    /// 紧跟点数组的 iL 行强度。每块数据只检查 `剩余 ≥ 需要`，与 GDP-TCP 的 SDK 规则相同。
    fn take_intensity(&self, buf: &mut Bytes) -> Result<Bytes, GdpError> {
        let len = product(self.intensity_length as usize, self.intensity_row_size())?;
        take(buf, len)
    }
}

/// 统一表面（类型 14，`GoGdpSurfaceUniform`）。
#[derive(Clone, Debug, PartialEq)]
pub struct UniformSurface {
    /// 属性。
    pub attrs: SurfaceAttrs,
    /// L×W 个 z 原始值，行主序；`i16::MIN` 表示无效。
    pub ranges: Vec<i16>,
    /// iL 行、每行 [`intensity_row_size`](SurfaceAttrs::intensity_row_size) 字节的强度，帧的零拷贝切片。
    pub intensity: Bytes,
}

/// 表面点云（类型 15，`GoGdpSurfacePointCloud`）。
#[derive(Clone, Debug, PartialEq)]
pub struct SurfacePointCloud {
    /// 属性。
    pub attrs: SurfaceAttrs,
    /// 相邻的点是否相连（可以画成网格），不表示有序。
    pub is_adjacent: bool,
    /// L×W 个 `(x, y, z)` 原始值，行主序；z 为 `i16::MIN` 表示无效（与 SDK 示例相同）。
    pub points: Vec<[i16; 3]>,
    /// iL 行、每行 [`intensity_row_size`](SurfaceAttrs::intensity_row_size) 字节的强度，帧的零拷贝切片。
    pub intensity: Bytes,
}

impl UniformSurface {
    /// 读取属性段与段外的数据。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let (attrs, _) = SurfaceAttrs::decode(buf, false)?;
        Ok(Self {
            attrs,
            ranges: read_vec(buf, attrs.point_count()?, i16::from_le_bytes)?,
            intensity: attrs.take_intensity(buf)?,
        })
    }
}

impl SurfacePointCloud {
    /// 读取属性段与段外的数据。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let (attrs, is_adjacent) = SurfaceAttrs::decode(buf, true)?;
        Ok(Self {
            attrs,
            is_adjacent,
            points: read_vec(buf, attrs.point_count()?, |[a, b, c, d, e, f]| {
                [
                    i16::from_le_bytes([a, b]),
                    i16::from_le_bytes([c, d]),
                    i16::from_le_bytes([e, f]),
                ]
            })?,
            intensity: attrs.take_intensity(buf)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{SurfaceAttrs, SurfacePointCloud, UniformSurface};
    use crate::gdp::tests::decode_fixture;
    use crate::gdp::{Body, PixelFormat};
    use crate::hex;

    // F14a / F14b / F15（derived）：旧式 74 字节属性段的强度格式缺省为 8 位灰度；1.5 的 Mono12p iW=5 每行 8 字节；
    // 点云的 isAdjacent 位于 exposure 之后、格式之前，旧式格式 1 每像素 1 字节。
    #[test]
    fn surface_fixtures() {
        let f14_legacy = "74 00 00 00 0E 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
            4A 00 01 00 00 00 02 00 00 00 01 00 00 00 02 00 00 00 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 00 40 \
            00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 F0 BF 00 00 00 00 00 00 00 00 00 00 00 00 00 00 24 40 \
            07 00 00 00 00 00 C8 42 01 00 00 80 11 22";
        let f14_mono12p = "7E 00 00 00 0E 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
            4E 00 01 00 00 00 02 00 00 00 01 00 00 00 05 00 00 00 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 00 40 \
            00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 F0 BF 00 00 00 00 00 00 00 00 00 00 00 00 00 00 24 40 \
            07 00 00 00 00 00 C8 42 47 00 0C 01 01 00 00 80 01 02 03 04 05 06 07 08";
        let f15 = "7A 00 00 00 0F 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
            4F 00 01 00 00 00 01 00 00 00 01 00 00 00 01 00 00 00 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 00 40 \
            00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 F0 BF 00 00 00 00 00 00 00 00 00 00 00 00 00 00 24 40 \
            07 00 00 00 00 00 C8 42 01 01 00 00 00 01 00 02 00 03 00 FF";
        let legacy = SurfaceAttrs {
            length: 1,
            width: 2,
            intensity_length: 1,
            intensity_width: 2,
            x_scale: 0.5,
            y_scale: 2.0,
            z_scale: 0.25,
            x_offset: -1.0,
            y_offset: 0.0,
            z_offset: 10.0,
            surface_id: 7,
            exposure: 100.0,
            intensity_format: PixelFormat::GREYSCALE_8BPP,
        };
        let cases = [
            (
                f14_legacy,
                Body::UniformSurface(UniformSurface {
                    attrs: legacy,
                    ranges: vec![1, i16::MIN],
                    intensity: Bytes::from_static(&[0x11, 0x22]),
                }),
            ),
            (
                f14_mono12p,
                Body::UniformSurface(UniformSurface {
                    attrs: SurfaceAttrs {
                        intensity_width: 5,
                        intensity_format: PixelFormat::MONO12P,
                        ..legacy
                    },
                    ranges: vec![1, i16::MIN],
                    intensity: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8]),
                }),
            ),
            (
                f15,
                Body::SurfacePointCloud(SurfacePointCloud {
                    attrs: SurfaceAttrs {
                        width: 1,
                        intensity_width: 1,
                        ..legacy
                    },
                    is_adjacent: true,
                    points: vec![[1, 2, 3]],
                    intensity: Bytes::from_static(&[0xFF]),
                }),
            ),
        ];
        for (fixture, expected) in cases {
            assert_eq!(decode_fixture(&hex(fixture)).body, expected);
        }
    }
}
