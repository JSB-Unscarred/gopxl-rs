//! GDP 数据通道的消息解码：服务端经 TCP（默认 3601）单向推送测量数据，控制通道 `/dataSources` 流的 bin 也是同样的消息。
//!
//! 消息全部小端、紧密排列：`[u32 size][u16 type][CAS][类型相关部分]`，`size` 包含自身。CAS 与各类型的属性段是“段”：
//! 开头是包含自身的长度字段（CAS 为 `u32`，属性段与 Rendering 条目为 `u16`）。属性段之后的数据数组不在任何段内，
//! 长度来自属性里的计数。
//!
//! 输入来自网络，以下约定保证坏消息只会得到 `Err` 或 [`Body::Invalid`]：
//! - 段内读取以段尾为界，读完已知字段后跳到段尾：新版本服务端追加的字段被忽略（向前兼容），字段不全的段也读不到
//!   下一段的数据（C++ 越界读取，到 `EndRead` 才发现）；段长小于长度字段自身或超出外层段时报错；
//! - 可选的尾部字段（gdpId、图像的 scale / offset、表面强度格式、标签对齐与颜色、测量与字符串的标签位置）只在
//!   段内剩余字节够整组时读取（图像 1.3 字段与测量 / 字符串的标签位置，C++ 剩 1 字节就读，会越过段尾；gdpId 则
//!   C++ 总是读取）；
//! - 数组先确认字节够再分配，长度乘法溢出按截断处理，计数只决定循环次数不决定预分配：声明巨大计数的坏消息
//!   不会触发巨量分配；
//! - 读取经过 `bytes::Buf` 的 `try_get_*` 与带长度检查的切片，下面的 lint 禁止下标与 unwrap，读线程不会因为坏消息 panic。
//!
//! CAS 之后的部分解析失败时消息仍然返回（[`Body::Invalid`]），`cas.is_last_msg` 照样能结束数据集；未知类型
//! 也解析 CAS（C++ 跳过整条消息，丢掉 `isLastMsg` 使两个数据集合并，重写方案 §5.4 第 22 条）。
//!
//! 零拷贝：`u8` 数据（强度、图像像素、Mesh 纹理与用户通道、未知类型的消息体）是帧的切片，持有它们会让整帧存活；
//! 多字节数组解码为 `Vec`。文本按字节数读取并保留内嵌 NUL，非法 UTF-8 替换为 U+FFFD。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod image;
mod mesh;
mod profile;
mod rendering;
mod spots;
mod surface;

use std::mem;

use bytes::{Buf, Bytes, TryGetError};

use crate::control::Status;

pub use image::{Image, PixelFormat};
pub use mesh::{Mesh, MeshChannel, MeshData};
pub use profile::{ProfileAttrs, ProfilePointCloud, UniformProfile};
pub use rendering::{Label, LineSet, Plane, PointSet, Position, Ray, Region, Rendering};
pub use spots::{Spot, Spots};
pub use surface::{SurfaceAttrs, SurfacePointCloud, UniformSurface};

/// GDP 服务端的默认 TCP 端口（`GO_PXL_SDK_DEFAULT_GDP_SERVER_PORT`）。
pub const DEFAULT_PORT: u16 = 3601;

/// 消息头（`u32 size` + `u16 type`）的长度，即最短的消息；连接层用
/// [`FrameReader::new(HEADER_LEN, max)`](crate::frame::FrameReader::new) 逐条读取。
pub const HEADER_LEN: u32 = 6;

/// [`decode_message`] 与 [`decode_all`] 的错误，也是 [`Body::Invalid`] 的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GdpError {
    /// 数据短于字段声明的长度。
    #[error("GDP data is shorter than its fields declare")]
    Truncated,
    /// 段的长度字段小于自身宽度或超出外层段，或消息的 size 与帧长不符。
    #[error("GDP size field is inconsistent with the enclosing data")]
    Malformed,
}

/// 越界读取都意味着数据短于字段声明的长度。
impl From<TryGetError> for GdpError {
    fn from(_: TryGetError) -> Self {
        Self::Truncated
    }
}

/// 一条 GDP 消息，对应 C++ 的 `GoGdpMsg` 及其派生类。
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// 消息类型（C++ `MessageType`）：1 Signal、10 Null、11 Stamp、12–18 扫描数据、19 Measurement、20 String、
    /// 70 Rendering、71–74 特征；其余为 [`Body::Unknown`]。
    pub ty: u16,
    /// 公共属性段。
    pub cas: Cas,
    /// 类型相关部分。
    pub body: Body,
}

/// 公共属性段（Common Attribute Section），对应 C++ 的 `CommonAttributes`。
#[derive(Clone, Debug, PartialEq)]
pub struct Cas {
    /// 坐标空间：0 无（Stamp、Signal）、1 数据空间（mm，3D）、2 图像空间（像素，2D）。
    pub space_type: u8,
    /// 3×4 行主序变换 `xx xy xz xt yx yy yz yt zx zy zz zt`（隐含末行 `0 0 0 1`）；`None` 为恒等变换。
    pub transform: Option<[f32; 12]>,
    /// 包围盒 `x y z width length height`（mm，手册称 xyz 为中心）；`None` 为未定义。
    pub bounding_box: Option<[f32; 6]>,
    /// 数组的元素数；0 表示不是数组。
    pub array_count: u32,
    /// 在数组中的下标，`array_count ≥ 1` 时有意义。
    pub array_index: u32,
    /// 数据源 ID，例如 `scan:LMILaserLineProfiler:scanner-0:topUniformProfile`。
    pub data_source_id: String,
    /// 时间戳源 ID（目前是 scanner ID）。
    pub stamp_source_id: String,
    /// 数据集 ID（原 `FrameId`）；服务端的取值不可靠，组集不用它。
    pub data_set_id: u64,
    /// 是否为数据集的最后一条消息，接收端据此结束一个数据集。
    pub is_last_msg: bool,
    /// 输出 ID（`/controls/gocator/outputs` 的 `outputId`，0 有效）。旧服务端的 CAS 不含此字段，或值为 0xFFFF（未使用）
    /// 时为 `None`（C++ 总是读取，短 CAS 会读进类型相关部分并失败，重写方案 §5.4 第 27 条）。
    pub gdp_id: Option<u16>,
}

/// 消息的类型相关部分。
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// 类型 1：流上此前的扫描数据全部作废（扫描或数据流开始时发送）。消息体不解析。
    Signal,
    /// 类型 10：工具发出的状态消息，携带错误状态（`kOK` = 1）。
    Null(Status),
    /// 类型 11：帧的时间戳与编码器值。
    Stamp(Stamp),
    /// 类型 12。
    UniformProfile(UniformProfile),
    /// 类型 13。
    ProfilePointCloud(ProfilePointCloud),
    /// 类型 14。
    UniformSurface(UniformSurface),
    /// 类型 15。
    SurfacePointCloud(SurfacePointCloud),
    /// 类型 16。
    Image(Image),
    /// 类型 17。
    Spots(Spots),
    /// 类型 18。
    Mesh(Mesh),
    /// 类型 19：工具的测量值。
    Measurement(Measurement),
    /// 类型 20：工具输出的字符串。
    String(GdpString),
    /// 类型 70：工具的叠加图形。
    Rendering(Rendering),
    /// 类型 71。
    PointFeature(PointFeature),
    /// 类型 72。
    LineFeature(LineFeature),
    /// 类型 73。
    PlaneFeature(PlaneFeature),
    /// 类型 74。
    CircleFeature(CircleFeature),
    /// 未知类型（含 1.5 从不发送的 Health 100）：CAS 之后的原始字节，帧的零拷贝切片。
    Unknown(Bytes),
    /// 已知类型，但 CAS 之后的部分解析失败；CAS 仍然有效。
    Invalid(GdpError),
}

/// 时间戳（类型 11，`GoGdpStamp`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// 帧序号，从 0 开始。
    pub frame_index: u64,
    /// 时间刻度（C++ `Timestamp`）：Gocator 上 µs = 值 / 1.024，AVT 2D 相机上为 ns。
    pub timestamp: u64,
    /// 编码器计数。
    pub encoder: i64,
    /// 在索引标记处锁存的编码器计数。
    pub encoder_at_z: i64,
    /// 状态位：bit 0 传感器数字输入，bit 4 主控数字输入，bit 8–9 帧间脉冲计数（每帧清零，上限 3）。
    pub status: u64,
    /// 系统时间的秒（PTP UTC 或 ARB 时标；不可用时为 0）。
    pub system_time_sec: u64,
    /// 系统时间的纳秒部分。
    pub system_time_nsec: u64,
}

/// 测量值（类型 19，`GoGdpMeasurement`），字段直接跟在 CAS 之后。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    /// 测量值。
    pub value: f64,
    /// 判定：0 通过，1 失败（超出范围）。
    pub decision: u8,
    /// 标签位置 x、y、z（1.4 新增）；剩余不足 24 字节时为 `None`。
    pub position: Option<[f64; 3]>,
}

/// 字符串（类型 20，`GoGdpString`），字段直接跟在 CAS 之后。
#[derive(Clone, Debug, PartialEq)]
pub struct GdpString {
    /// 文本，按字节数读取，保留内嵌 NUL（C++ 在 NUL 处截断，重写方案 §5.4 第 26 条）。
    pub text: String,
    /// 判定（1.5 新增，目前未使用）；剩余 0 字节时为 0。
    pub decision: u8,
    /// 标签位置 x、y、z（1.5 新增）；剩余不足 24 字节时为 `None`。
    pub position: Option<[f64; 3]>,
}

/// 点特征（类型 71，`GoGdpFeaturePoint`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointFeature {
    /// 位置 x、y、z。
    pub position: [f64; 3],
}

/// 线特征（类型 72，`GoGdpFeatureLine`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineFeature {
    /// 线上一点 x、y、z。
    pub position: [f64; 3],
    /// 方向 x、y、z。
    pub direction: [f64; 3],
}

/// 平面特征（类型 73，`GoGdpFeaturePlane`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneFeature {
    /// 法向 x、y、z。
    pub normal: [f64; 3],
    /// 到原点的距离。
    pub distance: f64,
}

/// 圆特征（类型 74，`GoGdpFeatureCircle`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CircleFeature {
    /// 圆心 x、y、z。
    pub center: [f64; 3],
    /// 法向 x、y、z。
    pub normal: [f64; 3],
    /// 半径。
    pub radius: f64,
}

/// 解码一条完整的消息（含 `u32 size`），对应 C++ 的 `GetGdpMsgType` + `Deserialize`。
///
/// `size` 必须等于 `frame.len()` 且 ≥ 6，CAS 必须完好，否则返回 `Err`：字节流已经无法信任，由连接层决定断开。
/// CAS 之后的部分解析失败不返回 `Err`，而是 [`Body::Invalid`]。
pub fn decode_message(mut frame: Bytes) -> Result<Message, GdpError> {
    let message = section(&mut frame, 4)?;
    if frame.has_remaining() {
        return Err(GdpError::Malformed);
    }
    decode_sized(message)
}

/// 解码控制通道 `/dataSources` 流的 bin：连续的完整消息，边界由每条消息的 `size` 决定，`size` 的规则与
/// [`decode_message`] 相同。
pub fn decode_all(mut payload: Bytes) -> Result<Vec<Message>, GdpError> {
    let mut messages = Vec::new();
    while payload.has_remaining() {
        messages.push(decode_sized(section(&mut payload, 4)?)?);
    }
    Ok(messages)
}

/// `size` 之后的部分：类型、CAS 与类型相关部分。
fn decode_sized(mut buf: Bytes) -> Result<Message, GdpError> {
    let ty = buf.try_get_u16_le()?;
    let cas = Cas::decode(&mut buf)?;
    let body = decode_body(ty, &mut buf).unwrap_or_else(Body::Invalid);
    Ok(Message { ty, cas, body })
}

impl Cas {
    /// 读取 CAS 段：`spaceType` … `isLastMsg` 是 1.0 之前就有的必需字段，`gdpId` 可选。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 4)?;
        let space_type = s.try_get_u8()?;
        let transform = read_bool(s)?
            .then(|| array(s, Bytes::try_get_f32_le))
            .transpose()?;
        let bounding_box = read_bool(s)?
            .then(|| array(s, Bytes::try_get_f32_le))
            .transpose()?;
        Ok(Self {
            space_type,
            transform,
            bounding_box,
            array_count: s.try_get_u32_le()?,
            array_index: s.try_get_u32_le()?,
            data_source_id: text(s, 2)?,
            stamp_source_id: text(s, 2)?,
            data_set_id: s.try_get_u64_le()?,
            is_last_msg: read_bool(s)?,
            gdp_id: optional(s, Bytes::try_get_u16_le).filter(|&id| id != u16::MAX),
        })
    }
}

/// 按类型解码 CAS 之后的部分（C++ `MakeGoGdpMsg` 的分派）。
fn decode_body(ty: u16, buf: &mut Bytes) -> Result<Body, GdpError> {
    let read_f64 = Bytes::try_get_f64_le;
    Ok(match ty {
        1 => Body::Signal,
        10 => Body::Null(Status(section(buf, 2)?.try_get_i32_le()?)),
        11 => Body::Stamp(Stamp::decode(buf)?),
        12 => Body::UniformProfile(UniformProfile::decode(buf)?),
        13 => Body::ProfilePointCloud(ProfilePointCloud::decode(buf)?),
        14 => Body::UniformSurface(UniformSurface::decode(buf)?),
        15 => Body::SurfacePointCloud(SurfacePointCloud::decode(buf)?),
        16 => Body::Image(Image::decode(buf)?),
        17 => Body::Spots(Spots::decode(buf)?),
        18 => Body::Mesh(Mesh::decode(buf)?),
        19 => Body::Measurement(Measurement {
            value: buf.try_get_f64_le()?,
            decision: buf.try_get_u8()?,
            position: optional(buf, |b| array(b, read_f64)),
        }),
        20 => Body::String(GdpString {
            text: text(buf, 4)?,
            decision: optional(buf, Bytes::try_get_u8).unwrap_or(0),
            position: optional(buf, |b| array(b, read_f64)),
        }),
        70 => Body::Rendering(Rendering::decode(buf)?),
        71 => Body::PointFeature(PointFeature {
            position: array(buf, read_f64)?,
        }),
        72 => Body::LineFeature(LineFeature {
            position: array(buf, read_f64)?,
            direction: array(buf, read_f64)?,
        }),
        73 => Body::PlaneFeature(PlaneFeature {
            normal: array(buf, read_f64)?,
            distance: buf.try_get_f64_le()?,
        }),
        74 => Body::CircleFeature(CircleFeature {
            center: array(buf, read_f64)?,
            normal: array(buf, read_f64)?,
            radius: buf.try_get_f64_le()?,
        }),
        _ => Body::Unknown(mem::take(buf)),
    })
}

impl Stamp {
    /// 读取 58 字节的属性段。
    fn decode(buf: &mut Bytes) -> Result<Self, GdpError> {
        let s = &mut section(buf, 2)?;
        Ok(Self {
            frame_index: s.try_get_u64_le()?,
            timestamp: s.try_get_u64_le()?,
            encoder: s.try_get_i64_le()?,
            encoder_at_z: s.try_get_i64_le()?,
            status: s.try_get_u64_le()?,
            system_time_sec: s.try_get_u64_le()?,
            system_time_nsec: s.try_get_u64_le()?,
        })
    }
}

/// 打开一个以 `width` 字节小端长度开头的段（长度包含自身），返回段内的字节，`buf` 前进到段尾，
/// 对应 `kSerializer_BeginRead` + `EndRead`。
///
/// 段内读取以返回的切片为界，未读的字段随段尾一起跳过（向前兼容）。长度小于自身宽度（C++ 的 u64 下溢）或超出
/// 外层段时报 `Malformed`，防止错位读进下一段。
fn section(buf: &mut Bytes, width: usize) -> Result<Bytes, GdpError> {
    let size = buf.try_get_uint_le(width)?;
    usize::try_from(size)
        .ok()
        .and_then(|size| size.checked_sub(width))
        .and_then(|len| take(buf, len).ok())
        .ok_or(GdpError::Malformed)
}

/// 取出 `n` 字节的零拷贝切片。
fn take(buf: &mut Bytes, n: usize) -> Result<Bytes, GdpError> {
    if n <= buf.len() {
        Ok(buf.split_to(n))
    } else {
        Err(GdpError::Truncated)
    }
}

/// 两个尺寸的乘积。溢出时数据不可能放进消息，按截断处理（C++ 用 32 位回绕的乘法）。
fn product(a: usize, b: usize) -> Result<usize, GdpError> {
    a.checked_mul(b).ok_or(GdpError::Truncated)
}

/// 读取 `count` 个各占 `N` 字节的小端元素，`read` 把一个元素的字节转换为值。先确认字节够再分配，声明巨大计数的
/// 坏消息只得到 `Truncated`。
fn read_vec<T, const N: usize>(
    buf: &mut Bytes,
    count: usize,
    read: impl Fn([u8; N]) -> T,
) -> Result<Vec<T>, GdpError> {
    let data = take(buf, product(count, N)?)?;
    // chunks_exact 的每块恰好 N 字节，first_chunk 总是 Some。
    let items = data.chunks_exact(N).filter_map(<[u8]>::first_chunk);
    Ok(items.map(|b| read(*b)).collect())
}

/// 把 12 字节拆成 3 个 4 字节的小端值（`f32` 或 `u32` 的 x、y、z）。
fn triple<T>(b: [u8; 12], read: fn([u8; 4]) -> T) -> [T; 3] {
    [
        read([b[0], b[1], b[2], b[3]]),
        read([b[4], b[5], b[6], b[7]]),
        read([b[8], b[9], b[10], b[11]]),
    ]
}

/// 读取 `count` 个变长条目（Mesh 通道、Rendering 图元）。不按计数预分配：每个条目至少消耗几个字节，
/// 计数巨大而数据很短的坏消息在第一个缺失的条目处报错。
fn read_items<T>(
    buf: &mut Bytes,
    count: u32,
    mut read: impl FnMut(&mut Bytes) -> Result<T, GdpError>,
) -> Result<Vec<T>, GdpError> {
    let mut items = Vec::new();
    for _ in 0..count {
        items.push(read(buf)?);
    }
    Ok(items)
}

/// 依次读取 `N` 个同类型的定长字段（坐标、矩阵）。
fn array<T: Copy + Default, const N: usize>(
    buf: &mut Bytes,
    mut read: impl FnMut(&mut Bytes) -> Result<T, TryGetError>,
) -> Result<[T; N], TryGetError> {
    let mut out = [T::default(); N];
    for v in &mut out {
        *v = read(buf)?;
    }
    Ok(out)
}

/// 可选的尾部字段组（新版本追加的字段）：剩余字节够读完整组才读取，否则视为缺失，剩余字节随段尾跳过。
/// 在副本上试读，组的长度不必另写一遍。
fn optional<T>(
    buf: &mut Bytes,
    read: impl FnOnce(&mut Bytes) -> Result<T, TryGetError>,
) -> Option<T> {
    let mut trial = buf.clone();
    let value = read(&mut trial).ok()?;
    *buf = trial;
    Some(value)
}

/// 读取 `u8` 布尔值，非 0 为真（C++ 的 `byteValue > 0`）。
fn read_bool(buf: &mut Bytes) -> Result<bool, TryGetError> {
    Ok(buf.try_get_u8()? != 0)
}

/// 读取以 `width` 字节小端长度开头的文本（线上不带结尾 NUL），保留全部字节，非法 UTF-8 替换为 U+FFFD。
fn text(buf: &mut Bytes, width: usize) -> Result<String, GdpError> {
    let len = usize::try_from(buf.try_get_uint_le(width)?).unwrap_or(usize::MAX);
    let text = String::from_utf8_lossy(buf.get(..len).ok_or(GdpError::Truncated)?).into_owned();
    buf.advance(len);
    Ok(text)
}
