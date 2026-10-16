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
