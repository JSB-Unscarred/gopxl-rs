//! 叠加图形（类型 70）：16 字节的属性段给出 7 种图元的个数，随后按顺序是各图元，每个以自己的 `u16` 段开头。

use bytes::{Buf, Bytes};

use super::{GdpError, array, optional, read_bool, read_items, read_vec, section, text, triple};

/// 叠加图形（类型 70，`GoGdpRendering` / `GoGraphics`）。各计数按 `u16` 读取（C++ 用 `k16s`，≥ 32768 时跳过循环
/// 并错位，重写方案 §5.4 第 26 条）。
#[derive(Clone, Debug, PartialEq)]
pub struct Rendering {
    /// 点集。
    pub point_sets: Vec<PointSet>,
    /// 线集。
    pub line_sets: Vec<LineSet>,
    /// 区域。
    pub regions: Vec<Region>,
    /// 平面。
    pub planes: Vec<Plane>,
    /// 射线。
    pub rays: Vec<Ray>,
    /// 标签。
    pub labels: Vec<Label>,
    /// 位置标记。
    pub positions: Vec<Position>,
}

/// 点集（`GoPointSet`）。
#[derive(Clone, Debug, PartialEq)]
pub struct PointSet {
    /// 点的大小。
    pub size: f32,
    /// 颜色 0xAARRGGBB。
    pub color: u32,
    /// 形状，取值未公开，原样保留。
    pub shape: i32,
    /// 点 x、y、z（在段外，紧跟段尾）。
    pub points: Vec<[f32; 3]>,
}

/// 线集（`GoLineSet`）。
#[derive(Clone, Debug, PartialEq)]
pub struct LineSet {
    /// 线宽。
    pub width: f32,
    /// 颜色 0xAARRGGBB。
    pub color: u32,
    /// 起点是否画箭头。
    pub start_arrow: bool,
    /// 终点是否画箭头。
    pub end_arrow: bool,
    /// 点 x、y、z（在段外，紧跟段尾），相邻两点构成一条线。
    pub points: Vec<[f32; 3]>,
}

/// 区域（`GoRegion`）。类型字节在区域的段之前、段外。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Region {
    /// 类型 0（`GoProfileRegion2d`）。
    Profile2d {
        /// x。
        x: f64,
        /// z。
        z: f64,
        /// 宽度。
        width: f64,
        /// 高度。
        height: f64,
        /// 绕 Y 轴的角度。
        angle_y: f64,
    },
    /// 类型 1（`GoRegion3d`）。
    Region3d {
        /// x。
        x: f64,
        /// y。
        y: f64,
        /// z。
        z: f64,
        /// 宽度（X）。
        width: f64,
        /// 长度（Y）。
        length: f64,
        /// 高度（Z）。
        height: f64,
        /// 绕 Z 轴的角度。
        angle_z: f64,
    },
    /// 类型 2（`GoSurfaceRegion2d`，SDK 预留）。
    Surface2d {
        /// x。
        x: f64,
        /// y。
        y: f64,
        /// 宽度（X）。
        width: f64,
        /// 长度（Y）。
        length: f64,
        /// 绕 Z 轴的角度。
        angle_z: f64,
    },
    /// 未知的区域类型，段被整体跳过（C++ 不消耗段而错位，.NET 抛异常，重写方案 §5.4 第 26 条）。
    Unknown(u8),
}

/// 平面（`GoPlane`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// 到原点的距离。
    pub distance: f32,
    /// 法向 x、y、z。
    pub normal: [f32; 3],
}

/// 射线（`GoRay`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    /// 起点 x、y、z。
    pub position: [f32; 3],
    /// 方向 x、y、z。
    pub direction: [f32; 3],
    /// 线宽。
    pub width: f32,
    /// 颜色 0xAARRGGBB。
    pub color: u32,
}

/// 标签（`GoLabel`）。
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// 文本，保留内嵌 NUL（C++ 在 NUL 处截断）。
    pub text: String,
    /// 位置 x、y、z。
    pub position: [f64; 3],
    /// 对齐（1.5 新增）：0 居中，1 / 2 中左 / 中右，4 / 5 / 6 中上 / 左上 / 右上，8 / 9 / 10 中下 / 左下 / 右下。
    /// 段内剩余 ≥ 8 字节才与 `color` 一起读取（C++ 不读取）。
    pub alignment: Option<i32>,
    /// 颜色 0xAARRGGBB（1.5 新增），与 `alignment` 同时出现。
    pub color: Option<u32>,
}

/// 位置标记（`GoPosition`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// 位置 x、y、z。
    pub position: [f64; 3],
    /// 轴：0 无、1 X、2 Y、3 Z。
    pub axis: u8,
}

impl Rendering {
    /// 读取计数段与各图元。计数段是独立的切片，按图元的顺序从中依次取出计数。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let counts = &mut section(buf, 2)?;
        let mut count = || counts.try_get_u16_le().map(u32::from);
        Ok(Self {
            point_sets: read_items(buf, count()?, PointSet::decode)?,
            line_sets: read_items(buf, count()?, LineSet::decode)?,
            regions: read_items(buf, count()?, Region::decode)?,
            planes: read_items(buf, count()?, Plane::decode)?,
            rays: read_items(buf, count()?, Ray::decode)?,
            labels: read_items(buf, count()?, Label::decode)?,
            positions: read_items(buf, count()?, Position::decode)?,
        })
    }
}

/// 读取段外、紧跟段尾的 `count` 个 `f32` 点。
fn read_points(buf: &mut Bytes, count: u16) -> Result<Vec<[f32; 3]>, GdpError> {
    read_vec(buf, count.into(), |b| triple(b, f32::from_le_bytes))
}

impl PointSet {
    /// 读取 16 字节的段与段外的点。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        Ok(Self {
            size: s.try_get_f32_le()?,
            color: s.try_get_u32_le()?,
            shape: s.try_get_i32_le()?,
            points: read_points(buf, s.try_get_u16_le()?)?,
        })
    }
}

impl LineSet {
    /// 读取 14 字节的段与段外的点。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        Ok(Self {
            width: s.try_get_f32_le()?,
            color: s.try_get_u32_le()?,
            start_arrow: read_bool(s)?,
            end_arrow: read_bool(s)?,
            points: read_points(buf, s.try_get_u16_le()?)?,
        })
    }
}

impl Region {
    /// 读取段外的类型字节与区域的段。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let ty = buf.try_get_u8()?;
        let s = &mut section(buf, 2)?;
        Ok(match ty {
            0 => {
                let [x, z, width, height, angle_y] = array(s, Bytes::try_get_f64_le)?;
                Self::Profile2d {
                    x,
                    z,
                    width,
                    height,
                    angle_y,
                }
            }
            1 => {
                let [x, y, z, width, length, height, angle_z] = array(s, Bytes::try_get_f64_le)?;
                Self::Region3d {
                    x,
                    y,
                    z,
                    width,
                    length,
                    height,
                    angle_z,
                }
            }
            2 => {
                let [x, y, width, length, angle_z] = array(s, Bytes::try_get_f64_le)?;
                Self::Surface2d {
                    x,
                    y,
                    width,
                    length,
                    angle_z,
                }
            }
            _ => Self::Unknown(ty),
        })
    }
}

impl Plane {
    /// 读取 18 字节的段。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        Ok(Self {
            distance: s.try_get_f32_le()?,
            normal: array(s, Bytes::try_get_f32_le)?,
        })
    }
}

impl Ray {
    /// 读取 34 字节的段。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        Ok(Self {
            position: array(s, Bytes::try_get_f32_le)?,
            direction: array(s, Bytes::try_get_f32_le)?,
            width: s.try_get_f32_le()?,
            color: s.try_get_u32_le()?,
        })
    }
}

impl Label {
    /// 读取 28+L 字节（1.5 为 36+L）的段。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        let text = text(s, 2)?;
        let position = array(s, Bytes::try_get_f64_le)?;
        let (alignment, color) =
            optional(s, |s| Ok((s.try_get_i32_le()?, s.try_get_u32_le()?))).unzip();
        Ok(Self {
            text,
            position,
            alignment,
            color,
        })
    }
}

impl Position {
    /// 读取 27 字节的段。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        Ok(Self {
            position: array(s, Bytes::try_get_f64_le)?,
            axis: s.try_get_u8()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{Label, LineSet, Plane, PointSet, Position, Ray, Region, Rendering};
    use crate::gdp::tests::{decode_fixture, message, with_junk};
    use crate::gdp::{Body, GdpError, decode_message};
    use crate::hex;

    /// F70（derived）：计数 (2, 1, 2, 0, 0, 2, 1)；第二个区域是类型 3、6 字节段的未知区域；第一个标签是旧式，
    /// 第二个带 1.5 的对齐与颜色。C++ 在未知区域处错位，.NET 的点集吞掉其余数据。
    const F70: &str = "17 01 00 00 46 00 1E 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 FF FF FF FF FF FF FF FF 01 FF FF \
        10 00 02 00 01 00 02 00 00 00 00 00 02 00 01 00 \
        10 00 00 00 00 40 00 00 FF FF 00 00 00 00 01 00 00 00 80 3F 00 00 00 40 00 00 40 40 \
        10 00 00 00 80 3F 00 FF 00 FF 01 00 00 00 00 00 \
        0E 00 00 00 80 3F FF 00 00 FF 01 00 02 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 80 3F 00 00 80 3F 00 00 80 3F \
        00 2A 00 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 00 00 00 00 00 00 08 40 00 00 00 00 00 00 10 40 00 00 00 00 00 00 00 00 \
        03 06 00 DE AD BE EF \
        1E 00 02 00 4F 4B 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 00 00 00 00 00 00 08 40 \
        26 00 02 00 48 69 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 00 00 00 00 00 00 08 40 05 00 00 00 FF FF FF FF \
        1B 00 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 00 00 00 00 00 00 08 40 03";

    fn f70() -> Rendering {
        let label = |text: &str, alignment, color| Label {
            text: text.into(),
            position: [1.0, 2.0, 3.0],
            alignment,
            color,
        };
        Rendering {
            point_sets: vec![
                PointSet {
                    size: 2.0,
                    color: 0xFFFF_0000,
                    shape: 0,
                    points: vec![[1.0, 2.0, 3.0]],
                },
                PointSet {
                    size: 1.0,
                    color: 0xFF00_FF00,
                    shape: 1,
                    points: Vec::new(),
                },
            ],
            line_sets: vec![LineSet {
                width: 1.0,
                color: 0xFF00_00FF,
                start_arrow: true,
                end_arrow: false,
                points: vec![[0.0; 3], [1.0; 3]],
            }],
            regions: vec![
                Region::Profile2d {
                    x: 1.0,
                    z: 2.0,
                    width: 3.0,
                    height: 4.0,
                    angle_y: 0.0,
                },
                Region::Unknown(3),
            ],
            planes: Vec::new(),
            rays: Vec::new(),
            labels: vec![
                label("OK", None, None),
                label("Hi", Some(5), Some(0xFFFF_FFFF)),
            ],
            positions: vec![Position {
                position: [1.0, 2.0, 3.0],
                axis: 3,
            }],
        }
    }

    // F70（derived）：各图元的段布局、段外的点数组、跳过未知区域、标签的 1.5 尾部；第二条（derived）补上
    // 平面（18 字节段）与射线（34 字节段）。
    #[test]
    fn rendering_fixtures() {
        assert_eq!(decode_fixture(&hex(F70)).body, Body::Rendering(f70()));

        let planes_and_rays = message(
            70,
            "10 00 00 00 00 00 00 00 01 00 01 00 00 00 00 00 \
             12 00 00 00 00 40 00 00 00 00 00 00 00 00 00 00 80 3F \
             22 00 00 00 80 3F 00 00 00 40 00 00 40 40 00 00 00 00 00 00 00 00 00 00 80 3F 00 00 80 3F FF FF FF FF",
        );
        let expected = Rendering {
            point_sets: Vec::new(),
            line_sets: Vec::new(),
            regions: Vec::new(),
            planes: vec![Plane {
                distance: 2.0,
                normal: [0.0, 0.0, 1.0],
            }],
            rays: vec![Ray {
                position: [1.0, 2.0, 3.0],
                direction: [0.0, 0.0, 1.0],
                width: 1.0,
                color: 0xFFFF_FFFF,
            }],
            labels: Vec::new(),
            positions: Vec::new(),
        };
        assert_eq!(
            decode_fixture(&planes_and_rays).body,
            Body::Rendering(expected)
        );
    }

    // 计数按 u16 读取（重写方案 §5.4 第 26 条）：pointSetCount 0x8000 不会被当成负数跳过，数据不足时报 Truncated，
    // 且不按计数预分配。
    #[test]
    fn large_count_is_unsigned() {
        let mut bytes = hex(F70);
        bytes[38..40].copy_from_slice(&0x8000_u16.to_le_bytes());
        let msg = decode_message(Bytes::from(bytes)).unwrap();
        assert!(msg.cas.is_last_msg);
        assert_eq!(msg.body, Body::Invalid(GdpError::Truncated));
    }

    // 向前兼容约定：点集段、旧式标签段与 1.5 标签段末尾多出 1–3 字节时结果不变（点数组在段外；
    // 旧式标签剩余不足 8 字节，对齐与颜色仍为 None）。
    #[test]
    fn trailing_bytes_in_items_are_skipped() {
        let bytes = hex(F70);
        for (at, size) in [(68, 52), (214, 184), (252, 214)] {
            for n in 1..=3 {
                let msg = decode_message(with_junk(&bytes, at, &[0, size], n)).unwrap();
                assert_eq!(msg.body, Body::Rendering(f70()), "{n} bytes at {at}");
            }
        }
    }
}
