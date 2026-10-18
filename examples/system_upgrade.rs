//! 用升级包升级设备，对应 C++ 示例 `SystemUpgrade`。
//!
//! 用法：`cargo run --example system_upgrade -- <升级包>`。设备必须是传感器或 GoMax，不能是 PC；被远程控制的传感器要
//! 连接控制方（`GOPXL_IP=127.0.0.1`）。先订阅 `/system` 再发出升级命令，每条 `/system` 通知之后
//! 读取升级状态与进度，直到完成或失败。
//!
//! 与 C++ 的差异：升级包经文件传输连接发送（`Client::transfer`，协议假设 T1、T2），不占用主连接；包路径取自第一个
//! 命令行参数（C++ 写死 `./upgrade_archive.dat`），读不到时报错（C++ 发送空包）；默认地址同其他示例（C++ 此处为
//! 127.0.0.1）；升级失败时以错误退出（C++ 仍返回成功）；返回时订阅 Drop，先发出 `unSub` 再断开（C++ 直接断开）。

mod common;

use std::time::{Duration, Instant};

use gopxl::control::{Client, Method, Notice, Request};

const UPGRADE_COMMAND: &str = "/system/commands/upgrade";
/// 升级状态（`upgradeState`、`upgradePercentComplete`）所在的资源，也是订阅的表达式。
const UPGRADE_STATUS_PATH: &str = "/system";

/// 升级命令的超时：文件传输连接上连续没有收到数据的时长。
const UPGRADE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// 等待升级完成的时长。
const UPGRADE_COMPLETE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// `upgradeState`：失败。手册的说明文字与取值错位，取值以 C++ 的 `UpgradeState` 为准。
const UPGRADE_FAILED: i64 = -1;
/// `upgradeState`：完成，也是启动后的默认值。
const UPGRADE_COMPLETED: i64 = 0;

fn main() -> common::Result {
    let archive = std::env::args()
        .nth(1)
        .ok_or("usage: system_upgrade <upgrade archive>")?;
    println!("Press ENTER to start the upgrade process...");
    std::io::stdin().read_line(&mut String::new())?;

    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    println!("Reading upgrade archive...");
    let package = std::fs::read(&archive).map_err(|e| format!("{archive}: {e}"))?;
    let mut request = Request::new(Method::Call, UPGRADE_COMMAND);
    request.payload.set("package", package)?;

    println!("Upgrading system...");
    // 先订阅再发出升级命令：进度通知不会漏掉。
    let subscription = client.subscribe(UPGRADE_STATUS_PATH)?;
    // 升级包可能有几百 MB，经文件传输连接发送，主连接照常收通知。
    client.transfer(request, UPGRADE_COMMAND_TIMEOUT)?;
    println!(
        "Upgrading the system using {} archive.",
        std::path::absolute(&archive)?.display()
    );

    let deadline = Instant::now() + UPGRADE_COMPLETE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let notice = subscription
            .recv_timeout(remaining)?
            .ok_or("the upgrade did not complete within 5 minutes")?;
        // Lagged 时丢掉的可能正是 /system 的通知，同样重读状态。
        if matches!(&notice, Notice::Event(n) if n.path != UPGRADE_STATUS_PATH) {
            continue;
        }
        match read_progress(client) {
            Ok(UPGRADE_COMPLETED) => {
                println!("System upgraded successfully.");
                return Ok(());
            }
            Ok(UPGRADE_FAILED) => return Err("System upgrade failed.".into()),
            Ok(_) => {}
            // 与 C++ 的通知回调一样，一次读取出错只打印，继续等待下一条通知。
            Err(e) => eprintln!("{e} - Notification handler error."),
        }
    }
}

/// 读 `/system`，打印升级进度并返回 `upgradeState`。
fn read_progress(client: &Client) -> common::Result<i64> {
    let status = client.read(UPGRADE_STATUS_PATH).response()?.payload;
    let state = status["upgradeState"]
        .as_i64()
        .ok_or("/system has no \"upgradeState\"")?;
    let percent = status["upgradePercentComplete"]
        .as_u64()
        .ok_or("/system has no \"upgradePercentComplete\"")?;
    println!("Upgrade in progress: {percent}%.");
    Ok(state)
}
