//! 图像（类型 16）与像素格式。

use std::fmt;

use bytes::{Buf, Bytes};

use super::{Error, array, optional, product, read_bool, section, take};

/// 图像（类型 16）。
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// 行数。
    pub height: u32,
    /// 每行像素数。
    pub width: u32,
    /// 每像素字节数；GenICam PFNC 像素格式（见 [`PixelFormat`]）为 0。
    pub pixel_size: u32,
    /// 像素格式。
    pub pixel_format: PixelFormat,
    /// 拜耳滤镜：0 无、1 BG/GR、2 GB/RG、3 RG/GB、4 GR/BG。
    pub color_filter: i32,
    /// 曝光，原样透传。
    pub exposure: f32,
    /// X 方向翻转。
    pub flipped_x: bool,
    /// Y 方向翻转。
    pub flipped_y: bool,
    /// 转置，在翻转之后应用。
    pub transposed: bool,
    /// X、Y 分辨率（mm，1.3 新增），与 `offset` 是一组可选尾部字段，旧服务端为 `None`。
    pub scale: Option<[f64; 2]>,
    /// X、Y 偏移（mm），1.3 新增，与 `scale` 同时出现。
    pub offset: Option<[f64; 2]>,
    /// `height` 行、每行 [`row_size`](Image::row_size) 字节的像素，帧的零拷贝切片。
    pub pixels: Bytes,
}

impl Image {
    /// 每行字节数：`pixel_size == 0 && color_filter == 0` 时按格式位数紧密排列、向上取整到字节，否则为
    /// `width × pixel_size`。溢出 `usize`（32 位目标）时取 `usize::MAX`，使解码时的长度检查报 `Truncated`。
    pub fn row_size(&self) -> usize {
        if self.pixel_size == 0 && self.color_filter == 0 {
            self.pixel_format.row_bytes(self.width)
        } else {
            usize::try_from(u64::from(self.width) * u64::from(self.pixel_size))
                .unwrap_or(usize::MAX)
        }
    }

    /// 读取属性段（31 字节，1.3 起 63 字节）与段外的像素。
    pub(super) fn decode(buf: &mut Bytes) -> Result<Self, Error> {
        let s = &mut section(buf, 2)?;
        let height = s.try_get_u32_le()?;
        let width = s.try_get_u32_le()?;
        let pixel_size = s.try_get_u32_le()?;
        let pixel_format = PixelFormat(s.try_get_i32_le()?);
        let color_filter = s.try_get_i32_le()?;
        let _reserved = s.try_get_u16_le()?;
        let exposure = s.try_get_f32_le()?;
        let flipped_x = read_bool(s)?;
        let flipped_y = read_bool(s)?;
        let transposed = read_bool(s)?;
        let (scale, offset) = optional(s, |s| {
            Ok((
                array(s, Bytes::try_get_f64_le)?,
                array(s, Bytes::try_get_f64_le)?,
            ))
        })
        .unzip();
        let mut image = Self {
            height,
            width,
            pixel_size,
            pixel_format,
            color_filter,
            exposure,
            flipped_x,
            flipped_y,
            transposed,
            scale,
            offset,
            pixels: Bytes::new(),
        };
        image.pixels = take(buf, product(height as usize, image.row_size())?)?;
        Ok(image)
    }
}

/// 像素格式（线上为 `i32`）：0–6 是旧式格式，其余按 `GenICam` PFNC 像素格式命名，值为
/// `ColorType | PixelSize | id`，第 2 字节是每像素位数。未知值原样保留。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelFormat(pub i32);

impl PixelFormat {
    /// 每像素位数：旧式格式查表，其余取第 2 字节；未知的 7–0xFFFF 为 0。
    pub fn bits(self) -> u32 {
        match self.0 {
            0 => 0,
            1 | 2 => 8,
            3 => 32,
            4 => 1,
            5 => 16,
            6 => 24,
            _ => {
                let [_, _, bits, _] = self.0.to_le_bytes();
                bits.into()
            }
        }
    }

    /// 是否为旧式格式 0–6。
    pub fn is_legacy(self) -> bool {
        (0..=6).contains(&self.0)
    }

    /// `width` 个像素按位数紧密排列、向上取整到字节的长度；溢出处理同 [`Image::row_size`]。
    pub(super) fn row_bytes(self, width: u32) -> usize {
        let bits = u64::from(width) * u64::from(self.bits());
        usize::try_from(bits.div_ceil(8)).unwrap_or(usize::MAX)
    }
}

/// 名字或十六进制值。
impl fmt::Debug for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "PixelFormat({:#010x})", self.0),
        }
    }
}

/// 从一张表同时生成常量与 [`PixelFormat::name`]，避免两份清单不一致。
macro_rules! pixel_formats {
    ($($konst:ident = $code:literal => $name:literal,)*) => {
        impl PixelFormat {
            $(
                #[doc = concat!("`", $name, "`（`", stringify!($code), "`）。")]
                pub const $konst: Self = Self($code);
            )*

            /// 格式名（0 为 `"Null"`）；未知值为 `None`。
            pub fn name(self) -> Option<&'static str> {
                match self.0 {
                    $($code => Some($name),)*
                    _ => None,
                }
            }
        }
    };
}

// 数值按 GenICam PFNC 像素格式命名的编码：BayerXX16 从 0x0110002E 起，第 2 字节为 16 位。
pixel_formats! {
    NULL = 0 => "Null",
    GREYSCALE_8BPP = 1 => "Greyscale_8BPP",
    CFA_8BPP = 2 => "CFA_8BPP",
    BGRX_8BPC = 3 => "BGRX_8BPC",
    GREYSCALE_1BPP = 4 => "Greyscale_1BPP",
    GREYSCALE_16BPP = 5 => "Greyscale_16BPP",
    BGR_8BPC = 6 => "BGR_8BPC",
    MONO8 = 0x0108_0001 => "Mono8",
    MONO10 = 0x0110_0003 => "Mono10",
    MONO10P = 0x010A_0046 => "Mono10p",
    MONO12 = 0x0110_0005 => "Mono12",
    MONO12_PACKED = 0x010C_0006 => "Mono12Packed",
    MONO12P = 0x010C_0047 => "Mono12p",
    MONO14 = 0x0110_0025 => "Mono14",
    MONO16 = 0x0110_0007 => "Mono16",
    BAYER_GR8 = 0x0108_0008 => "BayerGR8",
    BAYER_RG8 = 0x0108_0009 => "BayerRG8",
    BAYER_GB8 = 0x0108_000A => "BayerGB8",
    BAYER_BG8 = 0x0108_000B => "BayerBG8",
    BAYER_GR10 = 0x0110_000C => "BayerGR10",
    BAYER_RG10 = 0x0110_000D => "BayerRG10",
    BAYER_GB10 = 0x0110_000E => "BayerGB10",
    BAYER_BG10 = 0x0110_000F => "BayerBG10",
    BAYER_GR12 = 0x0110_0010 => "BayerGR12",
    BAYER_RG12 = 0x0110_0011 => "BayerRG12",
    BAYER_GB12 = 0x0110_0012 => "BayerGB12",
    BAYER_BG12 = 0x0110_0013 => "BayerBG12",
    BAYER_GR12_PACKED = 0x010C_002A => "BayerGR12Packed",
    BAYER_RG12_PACKED = 0x010C_002B => "BayerRG12Packed",
    BAYER_GB12_PACKED = 0x010C_002C => "BayerGB12Packed",
    BAYER_BG12_PACKED = 0x010C_002D => "BayerBG12Packed",
    BAYER_GR10P = 0x010A_0056 => "BayerGR10p",
    BAYER_RG10P = 0x010A_0058 => "BayerRG10p",
    BAYER_GB10P = 0x010A_0054 => "BayerGB10p",
    BAYER_BG10P = 0x010A_0052 => "BayerBG10p",
    BAYER_GR12P = 0x010C_0057 => "BayerGR12p",
    BAYER_RG12P = 0x010C_0059 => "BayerRG12p",
    BAYER_GB12P = 0x010C_0055 => "BayerGB12p",
    BAYER_BG12P = 0x010C_0053 => "BayerBG12p",
    BAYER_GR16 = 0x0110_002E => "BayerGR16",
    BAYER_RG16 = 0x0110_002F => "BayerRG16",
    BAYER_GB16 = 0x0110_0030 => "BayerGB16",
    BAYER_BG16 = 0x0110_0031 => "BayerBG16",
    RGB8 = 0x0218_0014 => "Rgb8",
    BGR8 = 0x0218_0015 => "Bgr8",
    RGB10 = 0x0230_0018 => "Rgb10",
    BGR10 = 0x0230_0019 => "Bgr10",
    RGB12 = 0x0230_001A => "Rgb12",
    BGR12 = 0x0230_001B => "Bgr12",
    RGB14 = 0x0230_005E => "Rgb14",
    BGR14 = 0x0230_004A => "Bgr14",
    RGB16 = 0x0230_0033 => "Rgb16",
    BGR16 = 0x0230_004B => "Bgr16",
    RGBA8 = 0x0220_0016 => "Rgba8",
    BGRA8 = 0x0220_0017 => "Bgra8",
    RGBA10 = 0x0240_005F => "Rgba10",
    BGRA10 = 0x0240_004C => "Bgra10",
    RGBA12 = 0x0240_0061 => "Rgba12",
    BGRA12 = 0x0240_004E => "Bgra12",
    RGBA14 = 0x0240_0063 => "Rgba14",
    BGRA14 = 0x0240_0050 => "Bgra14",
    RGBA16 = 0x0240_0064 => "Rgba16",
    BGRA16 = 0x0240_0051 => "Bgra16",
    YUV411 = 0x020C_001E => "Yuv411",
    YUV422 = 0x0210_001F => "Yuv422",
    YUV444 = 0x0218_0020 => "Yuv444",
    YUV422_8 = 0x0210_0032 => "Yuv422_8",
    YCBCR8_CBYCR = 0x0218_003A => "YCbCr8_CbYCr",
    YCBCR422_8 = 0x0210_003B => "YCbCr422_8",
    YCBCR411_8_CBYYCRYY = 0x020C_003C => "YCbCr411_8_CbYYCrYY",
    YCBCR601_8_CBYCR = 0x0218_003D => "YCbCr601_8_CbYCr",
    YCBCR601_422_8 = 0x0210_003E => "YCbCr601_422_8",
    YCBCR601_411_8_CBYYCRYY = 0x020C_003F => "YCbCr601_411_8_CbYYCrYY",
    YCBCR709_8_CBYCR = 0x0218_0040 => "YCbCr709_8_CbYCr",
    YCBCR709_422_8 = 0x0210_0041 => "YCbCr709_422_8",
    YCBCR709_411_8_CBYYCRYY = 0x020C_0042 => "YCbCr709_411_8_CbYYCrYY",
    YCBCR422_8_CBYCRY = 0x0210_0043 => "YCbCr422_8_CbYCrY",
    YCBCR601_422_8_CBYCRY = 0x0210_0044 => "YCbCr601_422_8_CbYCrY",
    YCBCR709_422_8_CBYCRY = 0x0210_0045 => "YCbCr709_422_8_CbYCrY",
    YCBCR411_8 = 0x020C_005A => "YCbCr411_8",
    YCBCR8 = 0x0218_005B => "YCbCr8",
}
