//! 添加并配置测量工具：加入 Profile Bounding Box、开启并修改区域、改 extId、连接数据源、开启测量输出，最后把测量值
//! 加入 GDP 输出，对应 C++ 示例 `ConfigureTool`。
//!
//! 设备上要有扫描数据（实时或回放）才能读到测量值；加入的输出由 `receive_measurement` 接收。
//!
//! 与 C++ 的差异：`autoshift` 改为手册的 `autoShift`；工具路径按 href 原样使用（C++ 去掉了开头的 `/`）。

mod common;

use std::thread;
use std::time::Duration;

use gopxl::Value;
use gopxl::resource::rel;

const TOOLS_PATH: &str = "/tools/";
/// 改名后的 extId；以 `extId=` 访问工具，不依赖服务端分配的 ID。
const TOOL_ID: &str = "ProfileBoundingBox-demo";
const TOOL_PATH: &str = "/tools/extId=ProfileBoundingBox-demo";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    // position 0 把新工具放在最前，下面读 /tools 时第一项就是它。
    let payload = r#"{"type":"ProfileBoundingBox","autoConnect":false,"position":0}"#;
    let payload = payload.parse::<Value>()?;
    println!("\nAdding new tool: {}...", payload["type"]);
    client.create(TOOLS_PATH, payload).response()?;

    let tools = system.resource(TOOLS_PATH).child_paths(rel::ITEM)?;
    let tool_path = tools.first().ok_or("/tools has no item")?;

    let use_region = common::read_at(client, tool_path, "/parameters/UseRegion")?
        .as_bool()
        .ok_or("UseRegion is not a boolean")?;
    if use_region {
        println!("\nTool region already enabled.");
    } else {
        println!("\nEnabling tool region...");
        let payload = r#"{"parameters":{"UseRegion":true}}"#.parse::<Value>()?;
        client.update(tool_path, payload).response()?;
        let id = common::read_at(client, tool_path, "/id")?;
        println!("Tool ID: {}", common::text(&id));
    }

    println!("\nUpdating tool ID...");
    let mut payload = Value::object();
    payload.set("extId", TOOL_ID)?;
    client.update(tool_path, payload).response()?;
    println!("Tool ID: {TOOL_ID}");

    println!("\nModifying tool region...");
    let payload = r#"{"parameters":{"Region":{"height":10.0,"width":10.0,"x":5.0,"z":5.0}}}"#
        .parse::<Value>()?;
    client.update(TOOL_PATH, payload).response()?;

    let data_sources = system.resource("/dataSources/").child_paths(rel::ITEM)?;
    common::print_paths("Data sources", &data_sources);
    let inputs = system
        .resource(format!("{TOOL_PATH}/inputs"))
        .child_paths(rel::ITEM)?;
    common::print_paths("Tool inputs", &inputs);

    println!("\nConnecting tool input...");
    let mut payload = Value::object();
    payload.set("dataSource", common::scan_source("topUniformProfile"))?;
    client
        .update(&format!("{TOOL_PATH}/inputs/ProfileInput"), payload)
        .response()?;

    println!("\nEnabling measurement output...");
    let payload = r#"{"enabled":true}"#.parse::<Value>()?;
    client
        .update(&format!("{TOOL_PATH}/outputs/X"), payload)
        .response()?;

    // 给工具 1 s 处理扫描数据、算出测量值。
    thread::sleep(Duration::from_secs(1));
    let metrics = client
        .read(&format!("{TOOL_PATH}/metrics"))
        .response()?
        .payload;
    let value = metrics
        .pointer("/outputsByExtId/X/value")
        .ok_or("tool metrics have no value for output X")?;
    println!("\nMeasurement value: {value}");

    common::enable_gdp(client)?;
    // 输出 X 的数据源 ID：新工具是第一个 Profile Bounding Box，ID 为 `-0`。
    println!("\nAdding measurement value to output...");
    common::add_output(client, "tools:ProfileBoundingBox-0:outputs:X", 1)?;
    Ok(())
}
