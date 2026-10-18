//! 添加并配置测量工具：加入 Profile Bounding Box、开启区域、改 extId、修改区域、连接数据源、开启测量输出，最后把
//! 测量值加入 GDP 输出 1 号。
//!
//! 设备上要有扫描数据（实时或回放）才能读到测量值；加入的输出由 `receive_measurement` 接收。每次运行新建一个工具并
//! 把 extId 设为 `ProfileBoundingBox-demo`，重复运行前先删除上次创建的工具。

mod common;

use std::time::Duration;

use gopxl::Value;
use gopxl::resource::rel;

const TOOLS_PATH: &str = "/tools/";
/// 改名后的 extId；以 `extId=` 访问工具，不依赖服务端分配的 ID。
const TOOL_ID: &str = "ProfileBoundingBox-demo";
const TOOL_PATH: &str = "/tools/extId=ProfileBoundingBox-demo";

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    // position 0 把新工具放在最前，下面读 /tools 时第一项就是它。
    let payload = r#"{"type":"ProfileBoundingBox","autoConnect":false,"position":0}"#;
    let payload = payload.parse::<Value>()?;
    println!("\nAdding new tool: {}...", payload["type"]);
    client.create(TOOLS_PATH, payload).response().await?;

    let tools = system.resource(TOOLS_PATH).child_paths(rel::ITEM).await?;
    let tool_path = tools.first().ok_or("/tools has no item")?;

    let use_region = common::read_at(client, tool_path, "/parameters/UseRegion")
        .await?
        .as_bool()
        .ok_or("UseRegion is not a boolean")?;
    if use_region {
        println!("\nTool region already enabled.");
    } else {
        println!("\nEnabling tool region...");
        let payload = r#"{"parameters":{"UseRegion":true}}"#.parse::<Value>()?;
        client.update(tool_path, payload).response().await?;
        let id = common::read_at(client, tool_path, "/id").await?;
        println!("Tool ID: {}", common::text(&id));
    }

    println!("\nUpdating tool ID...");
    let mut payload = Value::object();
    payload.set("extId", TOOL_ID)?;
    client.update(tool_path, payload).response().await?;
    println!("Tool ID: {TOOL_ID}");

    println!("\nModifying tool region...");
    let payload = r#"{"parameters":{"Region":{"height":10.0,"width":10.0,"x":5.0,"z":5.0}}}"#
        .parse::<Value>()?;
    client.update(TOOL_PATH, payload).response().await?;

    let data_sources = system
        .resource("/dataSources/")
        .child_paths(rel::ITEM)
        .await?;
    common::print_paths("Data sources", &data_sources);
    let inputs = system
        .resource(format!("{TOOL_PATH}/inputs"))
        .child_paths(rel::ITEM)
        .await?;
    common::print_paths("Tool inputs", &inputs);

    println!("\nConnecting tool input...");
    let mut payload = Value::object();
    payload.set("dataSource", common::scan_source("topUniformProfile"))?;
    client
        .update(&format!("{TOOL_PATH}/inputs/ProfileInput"), payload)
        .response()
        .await?;

    println!("\nEnabling measurement output...");
    let payload = r#"{"enabled":true}"#.parse::<Value>()?;
    client
        .update(&format!("{TOOL_PATH}/outputs/X"), payload)
        .response()
        .await?;

    // 给工具 1 s 处理扫描数据、算出测量值。
    tokio::time::sleep(Duration::from_secs(1)).await;
    let metrics = client
        .read(&format!("{TOOL_PATH}/metrics"))
        .response()
        .await?
        .payload;
    let value = metrics
        .pointer("/outputsByExtId/X/value")
        .ok_or("tool metrics have no value for output X")?;
    println!("\nMeasurement value: {value}");

    common::enable_gdp(client).await?;
    // 输出 X 的数据源 ID 是 `tools:<工具 ID>:outputs:X`，工具 ID 即新工具路径（href）的最后一段（见 `src/docs/assumptions.rs` 的 R2）。
    let tool_id = tool_path.rsplit('/').next().unwrap_or_default();
    println!("\nAdding measurement value to output...");
    common::add_output(client, &format!("tools:{tool_id}:outputs:X"), 1).await?;
    Ok(())
}
