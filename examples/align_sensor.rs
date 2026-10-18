//! 清除并重新对准扫描器，打印对准状态与变换，对应 C++ 示例 `AlignSensor`。
//!
//! 传感器上要有扫描数据才能对准成功；对准可能耗时数秒（G3 传感器更久）。示例先把当前作业另存为
//! `SDK_alignment_sample`，然后覆盖设备上的对准结果。

mod common;

use std::thread;
use std::time::Duration;

use common::{ALIGNED, ALIGNING, SCANNER_PATH, SENSOR_PATH, UNALIGNED};
use gopxl::Value;

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();
    let alignment_path = format!("{SCANNER_PATH}/alignment");

    let payload = r#"{"name":"SDK_alignment_sample"}"#.parse::<Value>()?;
    println!("Saving job as {}...", payload["name"]);
    client.call("/jobs/commands/save", payload).response()?;

    println!("Clearing alignment...");
    client
        .call(
            &format!("{SCANNER_PATH}/commands/clearAlign"),
            Value::object(),
        )
        .response()?;

    println!("Executing alignment...");
    client
        .call(&format!("{SCANNER_PATH}/commands/align"), Value::object())
        .timeout(common::EXTENDED_TIMEOUT)
        .response()?;

    // align 的应答可能在对准完成之前返回（协议手册），所以每秒读一次 alignState，直到不再是“对准中”。
    let state = loop {
        thread::sleep(Duration::from_secs(1));
        let state = common::read_at(client, &alignment_path, "/alignState")?
            .as_i64()
            .ok_or("alignState is not an integer")?;
        if state != ALIGNING {
            break state;
        }
    };
    let label = match state {
        UNALIGNED => "Not aligned",
        ALIGNED => "Aligned",
        _ => return Err(format!("unknown alignment state {state}").into()),
    };
    println!("Alignment state: {label}");

    if state == UNALIGNED {
        println!("Alignment failed. Check alignment configuration on sensor.");
    }
    // 传感器的对准变换。
    let calibration = client
        .read(&format!("{SENSOR_PATH}/transform"))
        .response()?
        .payload;
    println!("Alignment transform: {{");
    println!("X angle:  {},", calibration["xAngle"]);
    println!("X offset: {},", calibration["xOffset"]);
    println!("Y angle:  {},", calibration["yAngle"]);
    println!("Y offset: {},", calibration["yOffset"]);
    println!("Z angle:  {},", calibration["zAngle"]);
    println!("Z offset: {}", calibration["zOffset"]);
    println!("}}");

    // 扫描器的变换：编码器分辨率与速度。
    let transform = client
        .read(&format!("{SCANNER_PATH}/transform"))
        .response()?
        .payload;
    println!("Encoder resolution: {}", transform["encoderResolution"]);
    println!("Speed: {}", transform["speed"]);
    Ok(())
}
