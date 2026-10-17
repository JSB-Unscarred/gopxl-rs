//! 经 GDP 接收单层、多层数组与分层输出的统一轮廓，对应 C++ 示例 `MultilayerOutputs`。
//!
//! 需要共焦线扫传感器（`LMIConfocalLineProfiler`）：与 C++ 的 `SampleUtils` 一样，先把 `common` 的引擎 ID 与路径
//! 改成共焦引擎。三个阶段依次输出层数为 1 的数组、层数为 8 的数组（各层以 `cas.array_index` 区分），以及开启分层
//! 输出后逐层添加的 8 个输出；每个阶段接收并打印一个数据集。
//! 与 C++ 的差异：轮廓点云的消息类型打印为 `Point Cloud Profile`（C++ 误印为 `Uniform Profile`）；接收失败时
//! 按空数据集继续（C++ 还会打印已收到的部分消息）。

mod common;

use common::NULL;
use gopxl::System;
use gopxl::control::Handle;
use gopxl::gdp::{Body, GdpClient, Message};
use gopxl::value::Value;

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
    update(client, common::SCANNER_PATH, spacing)?;

    common::enable_gdp(client)?;
    println!("Connecting to Gocator Protocol...");
    let mut gdp = common::connect_gdp(&system)?;

    println!("Configuring sensor for single profile layer output...");
    let single = r#"{"parameters":{"layerSettings":{"layerCount":1}}}"#;
    update(client, common::SENSOR_PATH, single)?;
    // individualLayersEnabled 为 false 时各层只合并为数组输出（见手册），没有 `...Layer0` 数据源；层数为 1 的
    // 数组即单个轮廓，所以与 C++ 一样添加数组。
    println!("Adding single profile to GDP output...");
    common::add_output(client, &source("UniformProfileArray"), 0)?;
    receive(&system, &mut gdp)?;
    system.stop()?;
    println!("System stopped.");

    client
        .call(REMOVE_ALL_OUTPUTS_PATH, Value::object())
        .response()?;
    println!("Configuring sensor for multiple layer profile array output...");
    let multiple = r#"{"parameters":{"layerSettings":{"layerCount":8}}}"#;
    update(client, common::SENSOR_PATH, multiple)?;
    println!("Adding array of profiles to GDP output...");
    common::add_output(client, &source("UniformProfileArray"), 0)?;
    receive(&system, &mut gdp)?;
    system.stop()?;
    println!("System stopped.");

    println!("Configuring sensor for multiple layer separated profile output...");
    let separated = r#"{"parameters":{"scanModeSettings":{"individualLayersEnabled":true}}}"#;
    update(client, common::SCANNER_PATH, separated)?;
    client
        .call(REMOVE_ALL_OUTPUTS_PATH, Value::object())
        .response()?;
    for layer in 0..8 {
        println!("Adding profile layer {layer} to GDP output...");
        common::add_output(client, &source(&format!("UniformProfileLayer{layer}")), 0)?;
    }
    receive(&system, &mut gdp)?;

    drop(gdp);
    system.stop()?;
    println!("System stopped.");
    Ok(())
}

/// 以 JSON 文本 `json` 更新 `path`。
fn update(client: &Handle, path: &str, json: &str) -> common::Result {
    client.update(path, json.parse::<Value>()?).response()?;
    Ok(())
}

/// 扫描器顶部传感器的数据源 `top<name>`。
fn source(name: &str) -> String {
    format!(
        "scan:{}:{}:top{name}",
        common::ENGINE_ID,
        common::SCANNER_ID
    )
}

/// 就绪时启动设备，接收并打印一个数据集，对应 C++ 每个阶段末尾的 `Start`、`ReceiveData` 与 `TransformDataSet`。
/// 与 C++ 一样，接收失败只打印错误，示例继续下一阶段；未结束的数据集留在 `gdp` 里，下一次接收接着组装。
fn receive(system: &System, gdp: &mut GdpClient) -> common::Result {
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
    let cas = &message.cas;
    match &message.body {
        Body::Stamp(stamp) => {
            common::print_cas("Stamp", cas);
            println!("Frame index: {}", stamp.frame_index);
            println!("\tTimestamp: {}", stamp.timestamp);
            println!("\tEncoder: {}", stamp.encoder);
        }
        Body::UniformProfile(profile) => {
            common::print_cas("Uniform Profile", cas);
            // 统一轮廓只发送 z，x 由点的下标换算。
            let points: Vec<[f64; 2]> = (0u32..)
                .zip(&profile.ranges)
                .filter(|&(_, &z)| z != NULL)
                .map(|(x, &z)| common::profile_mm(&profile.attrs, [x.into(), z.into()]))
                .collect();
            println!("Profile points count: {}", profile.ranges.len());
            println!("Valid points count: {}", points.len());
        }
        Body::ProfilePointCloud(profile) => {
            common::print_cas("Point Cloud Profile", cas);
            let points: Vec<[f64; 2]> = profile
                .points
                .iter()
                .filter(|&&[x, _]| x != NULL)
                .map(|point| common::profile_mm(&profile.attrs, point.map(f64::from)))
                .collect();
            println!("Received range points count: {}", profile.points.len());
            println!("Valid points count: {}", points.len());
        }
        _ => println!("No profile or stamp found in the message."),
    }
}
