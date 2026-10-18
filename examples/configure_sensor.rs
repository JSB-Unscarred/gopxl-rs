//! 配置扫描器与传感器：软件触发、单次与多次曝光、显示名、有效区域、数字输出，并读出网络配置，对应 C++ 示例
//! `ConfigureSensor`。
//!
//! 会修改设备上的触发源、曝光、显示名、有效区域与数字输出。只移植 Gocator 传感器分支：SmartCam 没有扫描模式，
//! 参数都在传感器上（软件触发源为 2，曝光在 `/parameters/generalSettings/exposure`，没有多次曝光，有效区域以像素
//! 为单位，数字输出用 port-1）。
//!
//! 与 C++ 的差异：删去重复发送的 triggerEvent update；曝光值按原值输出（C++ 的 `Get<int>` 截断小数）。

mod common;

use common::{
    ACTIVE_AREA_PATH, DIGITAL_OUTPUT_PATH, DIO_TRIGGER_EVENT_PATH, DIO_TRIGGER_SOFTWARE,
    EXPOSURE_MODE_MULTI, EXPOSURE_MODE_PATH, EXPOSURE_MODE_SINGLE, MULTI_EXPOSURE_PATH,
    SCANNER_PATH, SENSOR_PATH, SINGLE_EXPOSURE_PATH, TRIGGER_MODE_SOFTWARE, TRIGGER_SOURCE_PATH,
    section, trigger_event_name,
};
use gopxl::Value;
use gopxl::control::Handle;

const DIGITAL_OUTPUT_PORT_PATH: &str = "/controls/digitalOutput/devices/device-0/ports/port-0";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    section("Configuration 1: Update trigger source.");
    // 扫描器的软件触发源与数字输出的软件触发事件互不相关。
    let mut payload = Value::object();
    payload.set(TRIGGER_SOURCE_PATH, TRIGGER_MODE_SOFTWARE)?;
    println!("Setting scanner trigger source to \"Software\"...");
    client.update(SCANNER_PATH, payload).response()?;

    system.start()?;
    println!("Triggering scan using software...");
    client
        .call(&format!("{SCANNER_PATH}/actions/trigger"), Value::object())
        .response()?;
    system.stop()?;

    section("Configuration 2: Update single exposure.");
    let mut payload = Value::object();
    payload.set(EXPOSURE_MODE_PATH, EXPOSURE_MODE_SINGLE)?;
    payload.set(SINGLE_EXPOSURE_PATH, 1200)?;
    println!("Setting single exposure to 1200...");
    client.update(SENSOR_PATH, payload).response()?;
    let exposure = common::read_at(client, SENSOR_PATH, SINGLE_EXPOSURE_PATH)?;
    println!("Exposure value: {exposure}");

    section("Configuration 3: Update multiple exposures.");
    let mut payload = Value::object();
    payload.set(EXPOSURE_MODE_PATH, EXPOSURE_MODE_MULTI)?;
    payload.set(
        MULTI_EXPOSURE_PATH,
        [1080, 2010, 5040].map(Value::from).to_vec(),
    )?;
    println!("Setting multiple exposures...");
    client.update(SENSOR_PATH, payload).response()?;
    let exposures = common::read_at(client, SENSOR_PATH, MULTI_EXPOSURE_PATH)?;
    println!("Multi-exposure values: {exposures}");

    section("Configuration 4: Update sensor name.");
    println!("Setting display name...");
    let payload = r#"{"displayName":"Main-Sensor-Change"}"#.parse::<Value>()?;
    client.update(SENSOR_PATH, payload).response()?;
    let name = common::read_at(client, SENSOR_PATH, "/displayName")?;
    println!("Display name: {name}");

    section("Configuration 5: Change the active area.");
    // 有效区域是中心位置（x、y、z）与尺寸（width、length、height），Gocator 上以 mm 为单位。
    let mut payload = Value::object();
    payload.set(&format!("{ACTIVE_AREA_PATH}/width"), 3.5)?;
    println!("Updating active area...");
    client.update(SENSOR_PATH, payload).response()?;
    let area = common::read_at(client, SENSOR_PATH, ACTIVE_AREA_PATH)?;
    println!("Active area: {area}");

    section("Configuration 6: Enable, trigger, and configure Digital Output.");
    configure_digital_output(client)?;

    section("Configuration 7: Update network configuration.");
    let interfaces = common::read_at(client, common::IP_CONFIG_PATH, "/interfaces")?;
    println!("Network configurations: {interfaces}.");
    // 改 IP：把读到的载荷中 /interfaces/0/ipAddress 改为新地址后 update IP_CONFIG_PATH，之后用新地址连接。
    Ok(())
}

/// 开启数字输出，没有输出设备时加入第一个可用设备，把 port-0 的触发事件设为软件，再触发脉冲。
fn configure_digital_output(client: &Handle) -> common::Result {
    println!("Enabling Digital Output...");
    let payload = r#"{"enabled":true}"#.parse::<Value>()?;
    client.update(DIGITAL_OUTPUT_PATH, payload).response()?;

    let devices_path = format!("{DIGITAL_OUTPUT_PATH}/devices");
    let devices = client.read(&devices_path).response()?.payload;
    if devices.pointer("/_embedded").is_none() {
        println!("\nAdding device to Digital Output...");
        let available = client
            .read(&format!("{DIGITAL_OUTPUT_PATH}/availableDevices"))
            .response()?
            .payload;
        let device = available
            .pointer("/devices/0")
            .ok_or("no available digital output device")?;
        client.create(&devices_path, device.clone()).response()?;
    }

    let mut payload = Value::object();
    payload.set(DIO_TRIGGER_EVENT_PATH, DIO_TRIGGER_SOFTWARE)?;
    println!("Setting digital output trigger event to Software...");
    client
        .update(DIGITAL_OUTPUT_PORT_PATH, payload)
        .response()?;
    let event = common::read_at(client, DIGITAL_OUTPUT_PORT_PATH, DIO_TRIGGER_EVENT_PATH)?;
    let name = event
        .as_i64()
        .and_then(trigger_event_name)
        .ok_or_else(|| format!("unknown digital output trigger event {event}"))?;
    println!("Digital output trigger event: {name}");

    // 以下三步与 C++ 一样失败时只打印错误、继续执行：例如脉冲要求设备正在扫描，而这时设备已停止。
    let trigger_path = format!("{DIGITAL_OUTPUT_PORT_PATH}/commands/trigger");
    // target：定时脉冲的时间（µs）或编码器位置（mm）；value：true 为高电平。
    println!("\nTriggering single non-scheduled pulse...");
    let payload = r#"{"parameter":{"target":0,"value":true}}"#.parse::<Value>()?;
    report(client.call(&trigger_path, payload).response());

    // signalType：0 为脉冲，1 为连续；scheduled：true 时推迟到 target 指定的时间或位置。
    println!("\nChanging configurations for a continuous scheduled pulse...");
    let payload = r#"{"parameters":{"signalType":1,"scheduled":true}}"#.parse::<Value>()?;
    report(client.update(DIGITAL_OUTPUT_PORT_PATH, payload).response());

    println!("Triggering continuous scheduled pulse...");
    let payload = r#"{"parameter":{"target":0}}"#.parse::<Value>()?;
    report(client.call(&trigger_path, payload).response());
    Ok(())
}

/// 打印失败步骤的错误，不中止示例。
fn report<T>(result: gopxl::Result<T>) {
    if let Err(error) = result {
        eprintln!("Error: {error}");
    }
}
