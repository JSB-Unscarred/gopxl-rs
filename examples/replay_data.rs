//! 录制扫描数据并下载为 `.gprec` 文件，再上传、回放并跳到最后一帧，对应 C++ 示例 `ReplayData`。
//!
//! 会修改设备：开启录制，把扫描器改为 10 Hz 时间触发，运行 0.5 s 后停止；用只含当前作业与回放数据的归档把录制下载到
//! `sample_recording.gprec`，再恢复它并开启回放。
//!
//! 与 C++ 的差异：录制的下载与上传（归档、恢复）走文件传输连接（`Handle::transfer`）；只移植 `GOCATOR_SENSOR`
//! 分支（触发参数在扫描器上；SmartCam 的在传感器上）。

mod common;

use std::time::Duration;

use gopxl::Value;

const REPLAY_PATH: &str = "/replay/playback";
const REPLAY_SEEK_PATH: &str = "/replay/commands/seek";
const RECORDING_PATH: &str = "/replay/recording";
const RECORDING_FILE: &str = "sample_recording.gprec";

const FRAMERATE_PATH: &str = "/parameters/triggerSettings/frameRate";
const MAX_FRAMERATE_PATH: &str = "/parameters/triggerSettings/maxFrameRateEnabled";
const FRAMERATE: i64 = 10;

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    println!("Enabling recording...");
    client
        .update(RECORDING_PATH, r#"{"enabled":true}"#.parse::<Value>()?)
        .response()?;

    println!("Setting frame rate to {FRAMERATE}...");
    let mut params = Value::object();
    params.set(common::TRIGGER_SOURCE_PATH, common::TRIGGER_MODE_TIME)?;
    params.set(MAX_FRAMERATE_PATH, false)?;
    // 先关闭 maxFrameRateEnabled 再设帧率：开启时有的扫描引擎把 frameRate 设为只读。
    client
        .update(common::SCANNER_PATH, params.clone())
        .response()?;
    params.set(FRAMERATE_PATH, FRAMERATE)?;
    client.update(common::SCANNER_PATH, params).response()?;

    common::start_if_ready(&system)?;
    std::thread::sleep(Duration::from_millis(500));
    system.stop()?;

    // 只含当前作业与回放数据的归档就是录制文件。
    let contents = r#"{"contents":["liveJob","replay"]}"#.parse::<Value>()?;
    let recording_file = common::download_archive(client, &contents, RECORDING_FILE)?;
    common::restore_archive(client, contents, &recording_file)?;

    println!("Enabling replay...");
    client
        .update(
            REPLAY_PATH,
            r#"{"enabled":true,"frameRate":5}"#.parse::<Value>()?,
        )
        .response()?;

    let frame_count = common::read_at(client, "/replay", "/frameCount")?
        .as_i64()
        .ok_or("/replay frameCount is not an integer")?;
    println!("Number of frames in recording: {frame_count}");

    // 开始并立即停止回放。
    system.start()?;
    system.stop()?;

    if frame_count <= 0 {
        println!("Number of frames is zero - Cannot jump to specific frame");
    } else {
        let mut seek = Value::object();
        seek.set("positionDomain", 0)?;
        // position 从 0 开始。
        seek.set("position", frame_count - 1)?;
        client.call(REPLAY_SEEK_PATH, seek).response()?;
        println!("Frame changed to {frame_count}");
    }
    Ok(())
}
