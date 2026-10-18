//! 经 GDP 同步接收表面，对应 C++ 示例 `ReceiveSurface`：统一表面与表面点云的原始值按偏移与分辨率换算为毫米坐标。
//!
//! 实时数据时先切换到表面扫描模式，回放数据直接使用。与 C++ 的差异：按 source 精确匹配判断表面是否已在 GDP 输出中
//! （见 `common::ensure_output`）；去掉读取扫描模式之前无意义的 `update {}`。

mod common;

use common::NULL;
use gopxl::gdp::{Body, Message, SurfaceAttrs};

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    if !common::replay_enabled(client).await? {
        common::set_scan_mode(client, common::SCAN_MODE_SURFACE, "Surface").await?;
    }

    common::enable_intensity(client).await?;

    common::enable_gdp(client).await?;
    common::ensure_output(client, &common::scan_source("topUniformSurface")).await?;

    let mut gdp = common::connect_gdp(&system).await?;
    common::start_if_ready(&system).await?;

    let data_set = gdp.receive(common::RECEIVE_TIMEOUT).await?;
    println!("Total number of messages: {}", data_set.len());
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        print_surface(message);
    }

    drop(gdp);
    system.stop().await?;
    Ok(())
}

/// 把表面换算为毫米坐标并打印，对应 C++ 循环里的 `switch`。
fn print_surface(message: &Message) {
    let (kind, attrs, intensity, points) = match &message.body {
        Body::UniformSurface(surface) => {
            let a = &surface.attrs;
            // 统一表面只发送 z（行主序），x、y 由列、行下标换算。
            let points: Vec<[f64; 3]> = (0u32..)
                .zip(&surface.ranges)
                .filter(|&(_, &z)| z != NULL)
                .map(|(k, &z)| to_mm(a, [k % a.width, k / a.width].map(f64::from), z))
                .collect();
            ("Uniform Surface", a, &surface.intensity, points)
        }
        Body::SurfacePointCloud(surface) => {
            let a = &surface.attrs;
            let points = surface
                .points
                .iter()
                .filter(|&&[_, _, z]| z != NULL)
                .map(|&[x, y, z]| to_mm(a, [x, y].map(f64::from), z))
                .collect();
            ("Point Cloud Surface", a, &surface.intensity, points)
        }
        _ => {
            println!("No surface found in the message.");
            return;
        }
    };

    common::print_cas(kind, &message.cas);
    println!("Surface data length: {}", attrs.length);
    println!("Surface data width: {}", attrs.width);
    println!("Intensity length: {}", attrs.intensity_length);
    println!("Intensity width: {}", attrs.intensity_width);
    println!("Intensity pixel format: {:?}", attrs.intensity_format);
    println!("Valid points count: {}", points.len());
    if attrs.intensity_length > 0 && attrs.intensity_width > 0 {
        println!("Intensity buffer count: {}", intensity.len());
    }
}

/// 原始值换算为毫米：`偏移 + 分辨率 × 原始值`。
fn to_mm(attrs: &SurfaceAttrs, [x, y]: [f64; 2], z: i16) -> [f64; 3] {
    [
        attrs.x_offset + attrs.x_scale * x,
        attrs.y_offset + attrs.y_scale * y,
        attrs.z_offset + attrs.z_scale * f64::from(z),
    ]
}
