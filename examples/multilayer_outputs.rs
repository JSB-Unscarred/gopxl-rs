//! 经 GDP 接收单层、多层数组与分层输出的统一轮廓。
//!
//! 需要共焦线扫传感器（`LMIConfocalLineProfiler`）：运行前把 `common` 的引擎 ID 与路径常量改成共焦引擎。三个阶段
//! 依次输出层数为 1 的数组、层数为 8 的数组（各层以 `cas.array_index` 区分），以及开启分层输出后逐层添加的 8 个
//! 输出；每个阶段启动设备、接收并打印一个数据集后停止设备，接收失败时按空数据集继续下一阶段。会修改扫描模式、层数
//! 并清空 GDP 输出表。

mod common;

use gopxl::gdp::{self, Body, Message};
use gopxl::{System, Value};

/// 清空 GDP 输出表。
const REMOVE_ALL_OUTPUTS_PATH: &str = "/controls/gocator/outputs/commands/removeAll";

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    common::set_scan_mode(client, common::SCAN_MODE_PROFILE, "Profile").await?;

    println!("Enabling uniform spacing...");
    let spacing = r#"{"parameters":{"scanModeSettings":{
        "uniformSpacingEnabled":true,"individualLayersEnabled":false}}}"#;
    client
        .update(common::SCANNER_PATH, spacing.parse::<Value>()?)
        .response()
        .await?;

    common::enable_gdp(client).await?;
    let mut gdp = common::connect_gdp(&system).await?;

    println!("Configuring sensor for single profile layer output...");
    let single = r#"{"parameters":{"layerSettings":{"layerCount":1}}}"#;
    client
        .update(common::SENSOR_PATH, single.parse::<Value>()?)
        .response()
        .await?;
    // individualLayersEnabled 为 false 时各层只合并为数组输出（见手册），`...Layer0` 数据源不存在；层数为 1 的
    // 数组即单个轮廓，所以添加数组。
    println!("Adding single profile to GDP output...");
    common::add_output(client, &common::scan_source("topUniformProfileArray"), 0).await?;
    receive(&system, &mut gdp).await?;
    system.stop().await?;
    println!("System stopped.");

    client
        .call(REMOVE_ALL_OUTPUTS_PATH, Value::object())
        .response()
        .await?;
    println!("Configuring sensor for multiple layer profile array output...");
    let multiple = r#"{"parameters":{"layerSettings":{"layerCount":8}}}"#;
    client
        .update(common::SENSOR_PATH, multiple.parse::<Value>()?)
        .response()
        .await?;
    println!("Adding array of profiles to GDP output...");
    common::add_output(client, &common::scan_source("topUniformProfileArray"), 0).await?;
    receive(&system, &mut gdp).await?;
    system.stop().await?;
    println!("System stopped.");

    println!("Configuring sensor for multiple layer separated profile output...");
    let separated = r#"{"parameters":{"scanModeSettings":{"individualLayersEnabled":true}}}"#;
    client
        .update(common::SCANNER_PATH, separated.parse::<Value>()?)
        .response()
        .await?;
    client
        .call(REMOVE_ALL_OUTPUTS_PATH, Value::object())
        .response()
        .await?;
    for layer in 0..8 {
        println!("Adding profile layer {layer} to GDP output...");
        common::add_output(
            client,
            &common::scan_source(&format!("topUniformProfileLayer{layer}")),
            0,
        )
        .await?;
    }
    receive(&system, &mut gdp).await?;

    drop(gdp);
    system.stop().await?;
    println!("System stopped.");
    Ok(())
}

/// 就绪时启动设备，接收并打印一个数据集。演示用途，接收失败只打印错误，示例继续下一阶段；未结束的数据集留在
/// `gdp` 里，下一次接收接着组装。
async fn receive(system: &System, gdp: &mut gdp::Client) -> common::Result {
    common::start_if_ready(system).await?;
    let data_set = match gdp.receive(common::RECEIVE_TIMEOUT).await {
        Ok(data_set) => {
            println!("Synchronous data transfer established.");
            data_set
        }
        Err(e) => {
            eprintln!("Failed to receive data via synchronous connection: {e}");
            Vec::new()
        }
    };
    println!("Total number of messages received: {}", data_set.len());
    if data_set.is_empty() {
        println!("No data available.");
    }
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        print_message(message);
    }
    Ok(())
}

/// 打印时间戳，或把轮廓换算为毫米坐标并打印点数。
fn print_message(message: &Message) {
    if let Body::Stamp(stamp) = &message.body {
        common::print_cas("Stamp", &message.cas);
        println!("Frame index: {}", stamp.frame_index);
        println!("\tTimestamp: {}", stamp.timestamp);
        println!("\tEncoder: {}", stamp.encoder);
    } else if !common::print_profile(message) {
        println!("No profile or stamp found in the message.");
    }
}
