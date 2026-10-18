//! 经 GDP 同步接收高度图图像并写成 BMP，对应 C++ 示例 `ReceiveImage`。
//!
//! 适用于 Gocator 线激光传感器（`common::ENGINE_ID`）；使用实时数据时先把扫描模式切换为图像。图像写到
//! `common::IMAGE_FILE`。
//!
//! 与 C++ 的差异：
//! - 先切换扫描模式再启动（C++ 启动之后才切换）；
//! - 按数据源 ID 精确判断图像是否已在 GDP 输出中（见 `common::ensure_output`）。

mod common;

use gopxl::gdp::{Body, Cas, Image};

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    if !common::replay_enabled(client)? {
        common::set_scan_mode(client, common::SCAN_MODE_IMAGE, "Image")?;
        common::start_if_ready(&system)?;
    }

    common::enable_gdp(client)?;
    // 传感器的图像数据源：传感器 ID 加数据源键（`LMILaserLineProfiler` 为 `Image`）与序号 0。
    let source = common::scan_source(&format!("{}Image0", common::SENSOR_ID));
    common::ensure_output(client, &source)?;

    let mut gdp = common::connect_gdp(&system)?;

    let data_set = gdp.receive(common::RECEIVE_TIMEOUT)?;
    println!(
        "Total number of messages in this data set: {}",
        data_set.len()
    );
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        if let Body::Image(image) = &message.body {
            save_image(&message.cas, image);
        } else {
            println!(
                "No image found in the message. \
                 Check that scan data is being collected and that scan target is in range."
            );
        }
    }

    drop(gdp);
    system.stop()?;
    Ok(())
}

/// 打印图像的属性并写成 BMP；`write_bmp` 不支持的像素格式只打印原因。
fn save_image(cas: &Cas, image: &Image) {
    common::print_cas("Image", cas);
    println!("FlippedX: {}", image.flipped_x);
    println!("FlippedY: {}", image.flipped_y);
    println!("Height: {}", image.height);
    println!("Width: {}", image.width);
    println!("PixelSize: {}", image.pixel_size);
    println!("PixelFormat: {:?}", image.pixel_format);
    println!("ColorFilter: {}", image.color_filter);

    match common::write_bmp(common::IMAGE_FILE, image) {
        Ok(()) => println!("Image saved as '{}'", common::IMAGE_FILE),
        Err(e) => println!("{e}"),
    }
}
