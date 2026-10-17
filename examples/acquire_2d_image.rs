//! 设置 2D 相机的曝光、添加 `ImageFilter` 工具，经 GDP 接收含图像的数据集并打印，对应 C++ 示例 `Acquire2dImage`。
//!
//! 适用于 2D 视觉设备（`2dscanner` 引擎）。
//!
//! 与 C++ 的差异：
//! - 去掉没有活动异常时的 `throw;`：连接检查失败时返回错误，`receive` 返回的数据集至少有一条消息；
//! - 去掉 `ENGINE_ID` 不是 `2dscanner` 时的警告：路径已固定为 `2dscanner` 引擎。

mod common;

use gopxl::value::Value;

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    // 修改配置之前先停止。
    system.stop()?;
    let client = system.client();

    client
        .update(
            common::SENSOR_2D_PATH,
            r#"{"parameters":{"generalSettings":{"exposure":10.0}}}"#.parse::<Value>()?,
        )
        .response()?;

    client
        .create(
            "/tools/",
            r#"{"type":"ImageFilter","autoConnect":true,"position":0}"#.parse::<Value>()?,
        )
        .response()?;
    println!("New tool added: ImageFilter");

    common::enable_gdp(client)?;
    common::add_output(client, "scan:2dscanner:scanner-0:image", 0)?;
    println!("Added image to the Gocator Protocol output.");

    let mut gdp = common::connect_gdp(&system)?;
    println!("Gocator Protocol connected.");
    common::start_if_ready(&system)?;

    let data_set = common::wait_for_image(&mut gdp)?;
    common::print_image_set(&data_set, None)?;

    drop(gdp);
    system.stop()?;
    println!("System stopped.");
    Ok(())
}
