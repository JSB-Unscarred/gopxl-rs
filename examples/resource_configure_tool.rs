//! 用资源 API 添加并配置测量工具：创建 Profile Bounding Box、改 extId、开启并修改区域、列出数据源与工具输入、连接
//! 输入、开启测量输出并读取测量值，最后开启 GDP 输出并加入测量值，对应 C++ 示例 `ResourceConfigureTool`。
//!
//! 设备上要有扫描数据（实时或回放）才能读到测量值；加入的输出由 `receive_measurement` 接收。每次运行都新建一个
//! 工具，extId 要求唯一，重复运行前先在设备上删除上次的工具。
//!
//! 与 C++ 的差异：区域的四个参数写进一个补丁直接 `update`（C++ 经 `ScopedUpdate` 合并，线上同为一个 update）；
//! `autoshift` 改为手册的 `autoShift`；输出的数据源 ID 用新工具路径的最后一段拼出（C++ 写死
//! `ProfileBoundingBox-0`，设备上已有同类工具时加入的是旧工具的输出）。其余请求的顺序、参数与超时与 C++ 相同。

mod common;

use std::thread;
use std::time::Duration;

use gopxl::Value;
use gopxl::resource::rel;

const TOOLS_PATH: &str = "/tools";
const DATA_SOURCES_PATH: &str = "/dataSources";
const TOOL_TYPE: &str = "ProfileBoundingBox";
/// 改名后的 extId。
const TOOL_ID: &str = "ProfileBoundingBox-demo";
const GOCATOR_CONTROL_PATH: &str = "/controls/gocator";
const GOCATOR_ADD_OUTPUT_PATH: &str = "/controls/gocator/outputs/commands/add";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;

    let mut args = Value::object();
    args.set("type", TOOL_TYPE)?;
    println!("\nAdding new tool: {TOOL_TYPE}...");
    let mut tool = system.resource(TOOLS_PATH).create_child(args)?;
    println!("Tool created at: {}", tool.path());

    println!("\nUpdating tool ID to \"{TOOL_ID}\"...");
    tool.set("/extId", TOOL_ID)?;

    // 上面的 update 使缓存过期，这里重新读取工具。
    let use_region = tool.get("/parameters/UseRegion", Value::as_bool)?;
    if use_region {
        println!("Tool region already enabled.");
    } else {
        println!("Enabling tool region...");
        tool.set("/parameters/UseRegion", true)?;
    }

    println!("\nModifying tool region...");
    let patch = r#"{"parameters":{"Region":{"height":10.0,"width":10.0,"x":5.0,"z":5.0}}}"#;
    tool.update(patch.parse::<Value>()?)?;

    let data_sources = system.resource(DATA_SOURCES_PATH).child_paths(rel::ITEM)?;
    print_paths("Data sources", &data_sources);

    let mut profile_input = tool.child("inputs/ProfileInput");
    println!("\nConnecting tool input at {}...", profile_input.path());
    let profile = format!(
        "scan:{}:{}:topUniformProfile",
        common::ENGINE_ID,
        common::SCANNER_ID
    );
    profile_input.set("/dataSource", profile)?;

    let inputs = tool.child("inputs").child_paths(rel::ITEM)?;
    print_paths("Tool inputs", &inputs);

    let mut output = tool.child("outputs/X");
    println!("\nEnabling measurement output at {}...", output.path());
    output.set("/enabled", true)?;

    // 给工具 1 s 处理扫描数据、算出测量值；与 C++ 相同，读不到时只提示。
    thread::sleep(Duration::from_secs(1));
    let mut metrics = tool.child("metrics");
    match metrics.get("/outputsByExtId/X/value", Some) {
        Ok(value) => println!("Measurement value: {value}"),
        Err(e) => println!("Note: Could not read measurement value ({e})"),
    }

    println!("\nEnabling Gocator Protocol...");
    system
        .resource(GOCATOR_CONTROL_PATH)
        .set("/enabled", true)?;

    // 输出 X 的数据源 ID 是 `tools:<工具 ID>:outputs:X`，工具 ID 即新工具路径的最后一段（协议假设 R2）。
    let tool_id = tool.path().rsplit('/').next().unwrap_or_default();
    let mut args = Value::object();
    args.set("source", format!("tools:{tool_id}:outputs:X"))?;
    args.set("outputId", 1)?;
    args.set("autoShift", true)?;
    println!("Adding measurement value to output...");
    system
        .resource(GOCATOR_ADD_OUTPUT_PATH)
        .call(args)
        .response()?;
    Ok(())
}

/// 打印标题与各子资源路径（C++ 遍历 `ChildUris` 的循环）。
fn print_paths(title: &str, paths: &[String]) {
    println!("\n{title}:");
    for path in paths {
        println!("  {path}");
    }
}
