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
use gopxl::control::{Method, Request};

const BACKUP_PATH: &str = "/system/commands/archive";
const RESTORE_PATH: &str = "/system/commands/restore";
const REPLAY_PATH: &str = "/replay/playback";
const REPLAY_SEEK_PATH: &str = "/replay/commands/seek";
const RECORDING_PATH: &str = "/replay/recording";
const RECORDING_FILE: &str = "sample_recording.gprec";

const TRIGGER_SOURCE_PATH: &str = "/parameters/triggerSettings/source";
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
    params.set(TRIGGER_SOURCE_PATH, common::TRIGGER_MODE_TIME)?;
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

    let contents = r#"{"contents":["liveJob","replay"]}"#.parse::<Value>()?;
    let archive = Request {
        payload: contents.clone(),
        ..Request::new(Method::Call, BACKUP_PATH)
    };
    let response = client.transfer(archive, common::RECEIVE_TIMEOUT)?;
    let data = response.payload["data"]
        .as_bytes()
        .ok_or("archive response has no binary \"data\"")?;

    println!("Downloading .gprec file...");
    let recording_file = std::path::absolute(RECORDING_FILE)?;
    std::fs::write(&recording_file, data)?;
    println!("Recording file location: {}", recording_file.display());
    drop(response);

    let mut restore = Request {
        payload: contents,
        ..Request::new(Method::Call, RESTORE_PATH)
    };
    restore
        .payload
        .set("data", std::fs::read(&recording_file)?)?;
    println!("Uploading .gprec file...");
    client.transfer(restore, common::EXTENDED_TIMEOUT)?;

    println!("Enabling replay...");
    client
        .update(
            REPLAY_PATH,
            r#"{"enabled":true,"frameRate":5}"#.parse::<Value>()?,
        )
        .response()?;

    let replay = client.read("/replay").response()?.payload;
    let frame_count = replay["frameCount"]
        .as_i64()
        .ok_or("/replay has no \"frameCount\"")?;
    println!("Number of frames in recording: {frame_count}");

    // 开始并立即停止回放；与 C++ 相同直接发命令，用默认的 3 s 超时。
    client
        .call("/system/commands/start", Value::object())
        .response()?;
    client
        .call("/system/commands/stop", Value::object())
        .response()?;

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
