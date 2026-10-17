//! 配置 2D 相机与 Script 工具，经 GDP 接收含图像的数据集并把图像写成 BMP，对应 C++ 示例 `Receive2dImage`。
//!
//! 适用于 2D 视觉设备（`2dscanner` 引擎），设备上要有作业 `job01`。Script 工具把图像亮度乘 3 后输出，并输出最小、
//! 最大与平均亮度三个测量值。图像写到 `common::IMAGE_FILE`。
//!
//! 与 C++ 的差异：
//! - 去掉没有活动异常时的 `throw;`：连接检查失败时返回错误，`receive` 返回的数据集至少有一条消息；
//! - 路径固定为 `2dscanner` 引擎，去掉 `ENGINE_ID` 不是 `2dscanner` 时的警告（C++ 取 `SampleUtils` 的线激光
//!   引擎路径，需手动切换）；
//! - 不支持的像素格式报错（C++ 的 `ImageUtils` 写出只有文件头的 BMP）。

mod common;

use gopxl::control::Client;
use gopxl::value::Value;

/// 扫描器的图像输出，`dataSourceId` 即要加入 GDP 输出的数据源。
const IMAGE_OUTPUT_PATH: &str = "/scan/engines/2dscanner/scanners/scanner-0/outputs/image";
/// [`setup_script_tool`] 在位置 0 创建的工具。
const SCRIPT_TOOL_PATH: &str = "/tools/Script-0";

/// Script 工具的代码：亮度乘 3 后作为 RGB 图像从输出 0 发出，最小、最大、平均亮度从输出 1–3 发出。
const SCRIPT: &str = "\
im = get_image(0)

new_pixels = im.pixels.copy()

factor = 3
new_pixels = numpy.clip(new_pixels * float(factor), 0, 255).astype(numpy.uint8)

min_intensity = numpy.min(new_pixels)
max_intensity = numpy.max(new_pixels)
mean_intensity = numpy.mean(new_pixels)

send_image(0, new_pixels, PixelFormat.RGB_8)

send_measurement(1, min_intensity)
send_measurement(2, max_intensity)
send_measurement(3, mean_intensity)
";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    // 修改配置之前先停止。
    system.stop()?;
    let client = system.client();

    client
        .call(
            "/jobs/commands/load",
            r#"{"name":"job01","enabled":true}"#.parse::<Value>()?,
        )
        .timeout(common::EXTENDED_TIMEOUT)
        .response()?;

    let mut trigger = Value::object();
    trigger.set(
        "/parameters/triggerSettings/source",
        common::TRIGGER_MODE_TIME,
    )?;
    client.update(common::SENSOR_2D_PATH, trigger).response()?;

    let mut exposure = Value::object();
    exposure.set("/parameters/generalSettings/exposure", 60.0)?;
    client.update(common::SENSOR_2D_PATH, exposure).response()?;

    setup_script_tool(client)?;

    common::enable_gdp(client)?;
    let output = client.read(IMAGE_OUTPUT_PATH).response()?.payload;
    let source = output["dataSourceId"]
        .as_str()
        .ok_or("the image output has no \"dataSourceId\"")?;
    println!("{source}");
    common::add_output(client, source, 0)?;

    println!("Connecting to Gocator Protocol...");
    let mut gdp = common::connect_gdp(&system)?;
    common::start_if_ready(&system)?;

    let data_set = common::wait_for_image(&mut gdp)?;
    common::print_image_set(&data_set, Some(common::IMAGE_FILE))?;

    drop(gdp);
    system.stop()?;
    Ok(())
}

/// 在位置 0 创建 Script 工具，设置 4 个输出及其类型并写入 [`SCRIPT`]。
fn setup_script_tool(client: &Client) -> common::Result {
    client
        .create(
            "/tools/",
            r#"{"type":"Script","autoConnect":true,"position":0}"#.parse::<Value>()?,
        )
        .response()?;
    client
        .update(
            SCRIPT_TOOL_PATH,
            r#"{"parameters":{"NumberOfOutputs":4}}"#.parse::<Value>()?,
        )
        .response()?;
    // 输出类型：18 为图像，5 为测量值。
    let types =
        r#"{"parameters":{"Output0Type":18,"Output1Type":5,"Output2Type":5,"Output3Type":5}}"#;
    client
        .update(SCRIPT_TOOL_PATH, types.parse::<Value>()?)
        .response()?;
    let mut code = Value::object();
    code.set("/parameters/Code/code", SCRIPT)?;
    client.update(SCRIPT_TOOL_PATH, code).response()?;
    Ok(())
}
