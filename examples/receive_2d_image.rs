//! 配置 2D 相机与 Script 工具，经 GDP 接收含图像的数据集并把图像写成 BMP。
//!
//! 适用于 2D 视觉设备，路径固定为 `2dscanner` 引擎，与 `common::ENGINE_ID` 无关；设备上要有作业 `job01`。示例先
//! 停止设备、加载 `job01` 再改配置，收到数据后停止设备。Script 工具把图像亮度乘 3 后输出，并输出最小、最大与平均
//! 亮度三个测量值。图像写到 `common::IMAGE_FILE`，像素格式不支持时返回错误。

mod common;

use gopxl::Value;
use gopxl::control::Client;

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

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    // 修改配置之前先停止。
    system.stop().await?;
    let client = system.client();

    client
        .call(
            "/jobs/commands/load",
            r#"{"name":"job01","enabled":true}"#.parse::<Value>()?,
        )
        .timeout(common::EXTENDED_TIMEOUT)
        .response()
        .await?;

    let mut trigger = Value::object();
    trigger.set(common::TRIGGER_SOURCE_PATH, common::TRIGGER_MODE_TIME)?;
    client
        .update(common::SENSOR_2D_PATH, trigger)
        .response()
        .await?;

    let mut exposure = Value::object();
    exposure.set("/parameters/generalSettings/exposure", 60.0)?;
    client
        .update(common::SENSOR_2D_PATH, exposure)
        .response()
        .await?;

    setup_script_tool(client).await?;

    common::enable_gdp(client).await?;
    let output = client.read(IMAGE_OUTPUT_PATH).response().await?.payload;
    let source = output["dataSourceId"]
        .as_str()
        .ok_or("the image output has no \"dataSourceId\"")?;
    println!("{source}");
    common::add_output(client, source, 0).await?;

    let mut gdp = common::connect_gdp(&system).await?;
    common::start_if_ready(&system).await?;

    let data_set = common::wait_for_image(&mut gdp).await?;
    common::print_image_set(&data_set, Some(common::IMAGE_FILE))?;

    drop(gdp);
    system.stop().await?;
    Ok(())
}

/// 在位置 0 创建 Script 工具，设置 4 个输出及其类型并写入 [`SCRIPT`]。
async fn setup_script_tool(client: &Client) -> common::Result {
    client
        .create(
            "/tools/",
            r#"{"type":"Script","autoConnect":true,"position":0}"#.parse::<Value>()?,
        )
        .response()
        .await?;
    client
        .update(
            SCRIPT_TOOL_PATH,
            r#"{"parameters":{"NumberOfOutputs":4}}"#.parse::<Value>()?,
        )
        .response()
        .await?;
    // 输出类型：18 为图像，5 为测量值。
    let types =
        r#"{"parameters":{"Output0Type":18,"Output1Type":5,"Output2Type":5,"Output3Type":5}}"#;
    client
        .update(SCRIPT_TOOL_PATH, types.parse::<Value>()?)
        .response()
        .await?;
    let mut code = Value::object();
    code.set("/parameters/Code/code", SCRIPT)?;
    client.update(SCRIPT_TOOL_PATH, code).response().await?;
    Ok(())
}
