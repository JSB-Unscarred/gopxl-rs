//! 设置 2D 相机的曝光、添加 `ImageFilter` 工具，经 GDP 接收含图像的数据集并打印。
//!
//! 适用于 2D 视觉设备，路径固定为 `2dscanner` 引擎，与 `common::ENGINE_ID` 无关。示例先停止设备再改配置，
//! 收到数据后停止设备；每次运行新建一个 `ImageFilter` 工具。

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
