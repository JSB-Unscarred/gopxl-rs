//! 经 GDP 接收单层、多层数组与分层输出的统一轮廓，对应 C++ 示例 `MultilayerOutputs`。
//!
//! 需要共焦线扫传感器（`LMIConfocalLineProfiler`）：与 C++ 的 `SampleUtils` 一样，先把 `common` 的引擎 ID 与路径
//! 改成共焦引擎。三个阶段依次输出层数为 1 的数组、层数为 8 的数组（各层以 `cas.array_index` 区分），以及开启分层
//! 输出后逐层添加的 8 个输出；每个阶段接收并打印一个数据集。
//! 与 C++ 的差异：轮廓点云的消息类型打印为 `Point Cloud Profile`（C++ 误印为 `Uniform Profile`）；接收失败时
//! 按空数据集继续（C++ 还会打印已收到的部分消息）。

mod common;

use gopxl::gdp::{self, Body, Message};
use gopxl::{System, Value};

/// 清空 GDP 输出表。
const REMOVE_ALL_OUTPUTS_PATH: &str = "/controls/gocator/outputs/commands/removeAll";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    common::set_scan_mode(client, common::SCAN_MODE_PROFILE, "Profile")?;

    println!("Enabling uniform spacing...");
    let spacing = r#"{"parameters":{"scanModeSettings":{
        "uniformSpacingEnabled":true,"individualLayersEnabled":false}}}"#;
    client
        .update(common::SCANNER_PATH, spacing.parse::<Value>()?)
        .response()?;

    common::enable_gdp(client)?;
    let mut gdp = common::connect_gdp(&system)?;

    println!("Configuring sensor for single profile layer output...");
    let single = r#"{"parameters":{"layerSettings":{"layerCount":1}}}"#;
    client
        .update(common::SENSOR_PATH, single.parse::<Value>()?)
        .response()?;
    // individualLayersEnabled 为 false 时各层只合并为数组输出（见手册），没有 `...Layer0` 数据源；层数为 1 的
    // 数组即单个轮廓，所以与 C++ 一样添加数组。
    println!("Adding single profile to GDP output...");
    common::add_output(client, &common::scan_source("topUniformProfileArray"), 0)?;
    receive(&system, &mut gdp)?;
    system.stop()?;
    println!("System stopped.");

    client
        .call(REMOVE_ALL_OUTPUTS_PATH, Value::object())
        .response()?;
    println!("Configuring sensor for multiple layer profile array output...");
    let multiple = r#"{"parameters":{"layerSettings":{"layerCount":8}}}"#;
    client
        .update(common::SENSOR_PATH, multiple.parse::<Value>()?)
        .response()?;
    println!("Adding array of profiles to GDP output...");
    common::add_output(client, &common::scan_source("topUniformProfileArray"), 0)?;
    receive(&system, &mut gdp)?;
    system.stop()?;
    println!("System stopped.");

    println!("Configuring sensor for multiple layer separated profile output...");
    let separated = r#"{"parameters":{"scanModeSettings":{"individualLayersEnabled":true}}}"#;
    client
        .update(common::SCANNER_PATH, separated.parse::<Value>()?)
        .response()?;
    client
        .call(REMOVE_ALL_OUTPUTS_PATH, Value::object())
        .response()?;
    for layer in 0..8 {
        println!("Adding profile layer {layer} to GDP output...");
        common::add_output(
            client,
            &common::scan_source(&format!("topUniformProfileLayer{layer}")),
            0,
        )?;
    }
    receive(&system, &mut gdp)?;

    drop(gdp);
    system.stop()?;
    println!("System stopped.");
    Ok(())
}

/// 就绪时启动设备，接收并打印一个数据集，对应 C++ 每个阶段末尾的 `Start`、`ReceiveData` 与 `TransformDataSet`。
/// 与 C++ 一样，接收失败只打印错误，示例继续下一阶段；未结束的数据集留在 `gdp` 里，下一次接收接着组装。
fn receive(system: &System, gdp: &mut gdp::Client) -> common::Result {
    common::start_if_ready(system)?;
    let data_set = match gdp.receive(common::RECEIVE_TIMEOUT) {
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

/// 打印时间戳，或把轮廓换算为毫米坐标并打印点数，对应 C++ 的 `TransformDataSet`。
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
