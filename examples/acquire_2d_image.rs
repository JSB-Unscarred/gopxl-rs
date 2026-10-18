//! 设置 2D 相机的曝光、添加 `ImageFilter` 工具，经 GDP 接收含图像的数据集并打印，对应 C++ 示例 `Acquire2dImage`。
//!
//! 适用于 2D 视觉设备（`2dscanner` 引擎）。
//!
//! 与 C++ 的差异：
//! - 去掉没有活动异常时的 `throw;`：连接检查失败时返回错误，`receive` 返回的数据集至少有一条消息；
//! - 去掉 `ENGINE_ID` 不是 `2dscanner` 时的警告：路径已固定为 `2dscanner` 引擎。

mod common;

use gopxl::Value;

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    // 修改配置之前先停止。
    system.stop().await?;
    let client = system.client();

    client
        .update(
            common::SENSOR_2D_PATH,
            r#"{"parameters":{"generalSettings":{"exposure":10.0}}}"#.parse::<Value>()?,
        )
        .response()
        .await?;

    client
        .create(
            "/tools/",
            r#"{"type":"ImageFilter","autoConnect":true,"position":0}"#.parse::<Value>()?,
        )
        .response()
        .await?;
    println!("New tool added: ImageFilter");

    common::enable_gdp(client).await?;
    common::add_output(client, "scan:2dscanner:scanner-0:image", 0).await?;
    println!("Added image to the Gocator Protocol output.");

    let mut gdp = common::connect_gdp(&system).await?;
    println!("Gocator Protocol connected.");
    common::start_if_ready(&system).await?;

    let data_set = common::wait_for_image(&mut gdp).await?;
    common::print_image_set(&data_set, None)?;

    drop(gdp);
    system.stop().await?;
    println!("System stopped.");
    Ok(())
}
