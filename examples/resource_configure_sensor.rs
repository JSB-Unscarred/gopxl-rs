//! 用资源层配置扫描器与传感器：软件触发、单次与多次曝光、显示名、有效区域、数字输出，并读出网络配置，对应 C++ 示例
//! `ResourceConfigureSensor`（`configure_sensor` 的资源层版本）。
//!
//! 会修改设备上的触发源、曝光、显示名、有效区域与数字输出。资源由 `System::resource` 得到：写入前按 schema 校验
//! （每个资源第一次写入时读一次 schema），写入之后的读取重新读服务端。
//!
//! 与 C++ 的差异：`ScopedUpdate` 块改为用 `Value::set` 构建一个补丁、调用一次 `update`，发送失败时退出（C++ 的
//! `GoUpdateScope` 析构吞掉 `Flush` 的异常，示例静默继续）；曝光值按原值输出（C++ 的 `GetInt` 截断小数）；可用设备
//! 或添加后的设备列表为空时报错退出（C++ 取空数组的首元素与 `deviceUris[0]`，行为未定义）；第一次触发返回
//! `notFound` 时 C++ 把端口标记为已删除，随后写端口抛异常退出，这里照常继续。

mod common;

use common::{
    ACTIVE_AREA_PATH, DIGITAL_OUTPUT_PATH, DIO_TRIGGER_EVENT_PATH, DIO_TRIGGER_SOFTWARE,
    EXPOSURE_MODE_MULTI, EXPOSURE_MODE_PATH, EXPOSURE_MODE_SINGLE, MULTI_EXPOSURE_PATH,
    SCANNER_PATH, SENSOR_PATH, SINGLE_EXPOSURE_PATH, TRIGGER_MODE_SOFTWARE, TRIGGER_SOURCE_PATH,
    section, trigger_event_name,
};
use gopxl::resource::rel;
use gopxl::{System, Value};

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let mut scanner = system.resource(SCANNER_PATH);
    let mut sensor = system.resource(SENSOR_PATH);

    section("Configuration 1: Update trigger source.");
    println!("Setting scanner trigger source to \"Software\"...");
    scanner
        .set(TRIGGER_SOURCE_PATH, TRIGGER_MODE_SOFTWARE)
        .await?;
    let source = scanner.get::<i64>(TRIGGER_SOURCE_PATH).await?;
    println!("Trigger source: {source}");

    system.start().await?;
    println!("Triggering scan using software...");
    scanner
        .action("trigger", Value::object())
        .response()
        .await?;
    system.stop().await?;

    section("Configuration 2: Update single exposure.");
    println!("Setting single exposure to 1200...");
    // 两个相关的修改放进同一个补丁，一次 update 发出（C++ 的 ScopedUpdate）。
    let mut patch = Value::object();
    patch.set(EXPOSURE_MODE_PATH, EXPOSURE_MODE_SINGLE)?;
    patch.set(SINGLE_EXPOSURE_PATH, 1200)?;
    sensor.update(patch).await?;
    let exposure = sensor.get::<Value>(SINGLE_EXPOSURE_PATH).await?;
    println!("Exposure value: {exposure}");

    section("Configuration 3: Update multiple exposures.");
    println!("Setting multiple exposures...");
    let mut patch = Value::object();
    patch.set(EXPOSURE_MODE_PATH, EXPOSURE_MODE_MULTI)?;
    patch.set(
        MULTI_EXPOSURE_PATH,
        [1080, 2010, 5040].map(Value::from).to_vec(),
    )?;
    sensor.update(patch).await?;
    let exposures = sensor.get::<Value>(MULTI_EXPOSURE_PATH).await?;
    println!("Multi-exposure values: {exposures}");

    section("Configuration 4: Update sensor name.");
    println!("Setting display name...");
    sensor.set("/displayName", "Main-Sensor-Change").await?;
    let name = sensor.get::<&str>("/displayName").await?;
    println!("Display name: {name}");

    section("Configuration 5: Change the active area.");
    println!("Updating active area...");
    sensor
        .set(&format!("{ACTIVE_AREA_PATH}/width"), 3.5)
        .await?;
    let area = sensor.get::<Value>(ACTIVE_AREA_PATH).await?;
    println!("Active area: {area}");

    section("Configuration 6: Enable, trigger, and configure Digital Output.");
    configure_digital_output(&system).await?;

    section("Configuration 7: Read network configuration.");
    let mut ip_config = system.resource(common::IP_CONFIG_PATH);
    let interfaces = ip_config.get::<Value>("/interfaces").await?;
    println!("Network configurations: {interfaces}.");
    Ok(())
}

/// 开启数字输出，设备列表为空时加入第一个可用设备，把第一个设备 port-0 的触发事件设为软件，再触发脉冲。
async fn configure_digital_output(system: &System) -> common::Result {
    let mut digital_output = system.resource(DIGITAL_OUTPUT_PATH);
    println!("Enabling Digital Output...");
    digital_output.set("/enabled", true).await?;

    // 设备列表为空时加入第一个可用设备（C++ 的 `ChildCount` 与 `CreateChild`）。
    let mut devices = digital_output.child("devices");
    if devices.child_paths(rel::ITEM).await?.is_empty() {
        println!("\nAdding device to Digital Output...");
        let mut available = digital_output.child("availableDevices");
        let device = available.get::<Value>("/devices/0").await?;
        devices.create_child(device).await?;
    }

    // 添加过设备时缓存已过期，这里重新读设备列表。
    let paths = devices.child_paths(rel::ITEM).await?;
    let device = paths.first().ok_or("Digital Output has no device")?;
    let mut port = system.resource(format!("{device}/ports/port-0"));

    println!("Setting digital output trigger event to Software...");
    port.set(DIO_TRIGGER_EVENT_PATH, DIO_TRIGGER_SOFTWARE)
        .await?;
    let event = port.get::<i64>(DIO_TRIGGER_EVENT_PATH).await?;
    let name = trigger_event_name(event)
        .ok_or_else(|| format!("unknown digital output trigger event {event}"))?;
    println!("Digital output trigger event: {name}");

    // 两次触发与 C++ 一样失败时只打印错误、继续执行：例如脉冲要求设备正在扫描，而这时设备已停止。
    // target：定时脉冲的时间（µs）或编码器位置（mm）；value：true 为高电平。
    println!("\nTriggering single non-scheduled pulse...");
    let args = r#"{"parameter":{"target":0,"value":true}}"#.parse::<Value>()?;
    if let Err(e) = port.command("trigger", args).response().await {
        eprintln!("Error: {e}");
        eprintln!("Failed to trigger digital output pulse.");
    }

    // signalType：0 为脉冲，1 为连续。
    println!("\nChanging configurations for a continuous scheduled pulse...");
    port.update(r#"{"parameters":{"signalType":1,"scheduled":true}}"#.parse::<Value>()?)
        .await?;

    println!("Triggering continuous scheduled pulse...");
    let args = r#"{"parameter":{"target":0}}"#.parse::<Value>()?;
    if let Err(e) = port.command("trigger", args).response().await {
        eprintln!("Error: {e}");
        eprintln!("Failed to trigger continuous scheduled pulse.");
    }
    Ok(())
}
