//! 用升级包升级设备。
//!
//! 用法：`cargo run --example system_upgrade -- <升级包>`，按回车后开始。设备必须是传感器或 GoMax，不能是 PC；
//! Accelerated 传感器要连接控制方（见 `common` 的模块文档）。升级包经文件传输连接发送（见 `src/docs/assumptions.rs`
//! 的 T1、T2）。先订阅 `/system` 再发出升级命令，每条 `/system` 通知之后读取升级状态与进度，直到完成或失败；失败时
//! 以错误退出，结束后先退订再断开。

mod common;

use std::time::{Duration, Instant};

use gopxl::control::{Client, Method, Notice, Request};
use tokio::io::{AsyncBufReadExt, BufReader};

const UPGRADE_COMMAND: &str = "/system/commands/upgrade";
/// 升级状态（`upgradeState`、`upgradePercentComplete`）所在的资源，也是订阅的表达式。
const UPGRADE_STATUS_PATH: &str = "/system";

/// 升级命令的超时：文件传输连接上连续无数据的时长。
const UPGRADE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// 等待升级完成的时长。
const UPGRADE_COMPLETE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// `upgradeState`：失败。
const UPGRADE_FAILED: i64 = -1;
/// `upgradeState`：完成，也是启动后的默认值。
const UPGRADE_COMPLETED: i64 = 0;

#[tokio::main]
async fn main() -> common::Result {
    let archive = std::env::args()
        .nth(1)
        .ok_or("usage: system_upgrade <upgrade archive>")?;
    println!("Press ENTER to start the upgrade process...");
    BufReader::new(tokio::io::stdin())
        .read_line(&mut String::new())
        .await?;

    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    println!("Reading upgrade archive...");
    let package = std::fs::read(&archive).map_err(|e| format!("{archive}: {e}"))?;
    let mut request = Request::new(Method::Call, UPGRADE_COMMAND);
    request.payload.set("package", package)?;

    println!("Upgrading system...");
    // 先订阅再发出升级命令：进度通知不会漏掉。
    let subscription = client.subscribe(UPGRADE_STATUS_PATH).await?;
    // 升级包可能有几百 MB，经文件传输连接发送，主连接照常收通知。
    client.transfer(request, UPGRADE_COMMAND_TIMEOUT).await?;
    println!(
        "Upgrading the system using {} archive.",
        std::path::absolute(&archive)?.display()
    );

    let deadline = Instant::now() + UPGRADE_COMPLETE_TIMEOUT;
    let outcome = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let notice = subscription
            .recv_timeout(remaining)
            .await?
            .ok_or("the upgrade did not complete within 5 minutes")?;
        // `Notice::Lagged` 表示通知队列满时丢掉了若干条，丢掉的可能正是 /system 的通知，同样重读状态。
        if matches!(&notice, Notice::Event(n) if n.path != UPGRADE_STATUS_PATH) {
            continue;
        }
        match read_progress(client).await {
            Ok(UPGRADE_COMPLETED) => {
                println!("System upgraded successfully.");
                break Ok(());
            }
            Ok(UPGRADE_FAILED) => break Err("System upgrade failed.".into()),
            Ok(_) => {}
            // 演示用途，一次读取出错只打印，继续等待下一条通知。
            Err(e) => eprintln!("{e} - Notification handler error."),
        }
    };
    subscription.unsubscribe().await?;
    outcome
}

/// 读 `/system`，打印升级进度并返回 `upgradeState`。
async fn read_progress(client: &Client) -> common::Result<i64> {
    let status = client.read(UPGRADE_STATUS_PATH).response().await?.payload;
    let state = status["upgradeState"]
        .as_i64()
        .ok_or("/system has no \"upgradeState\"")?;
    let percent = status["upgradePercentComplete"]
        .as_u64()
        .ok_or("/system has no \"upgradePercentComplete\"")?;
    println!("Upgrade in progress: {percent}%.");
    Ok(state)
}
