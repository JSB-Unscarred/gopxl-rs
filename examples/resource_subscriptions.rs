//! 用资源层订阅与缓存：订阅 `/system` 并观察启停带来的远端修改，创建传感器资源后随即订阅，按展开层级枚举引擎与
//! 扫描器，演示读缓存与失效。
//!
//! 会启动再停止设备。订阅表达式是 `<路径>/*`，子资源的通知也会到达。`has_remote_changes` 在从未读取时、以及同一
//! 连接上发出任何变更请求之后也为 `true`，所以启动后总是输出 `true` 并重读 `/system`。每个 `Resource` 各有缓存，
//! 扫描器一节新建引擎资源、读一次引擎。

mod common;

use std::time::Duration;

use common::{ENGINE_ID, ENGINE_PATH, SENSOR_PATH};
use gopxl::resource::rel;

/// 启停之后等待通知到达的时间。
const NOTIFY_WAIT: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;

    println!("\n=== Manual Subscription ===");
    let mut system_res = system.resource("/system");
    println!("Subscribing to /system...");
    system_res.subscribe().await?;
    println!("is_subscribed: {}", system_res.is_subscribed());

    let run_state = system_res.get::<i64>("/runState").await?;
    println!("Initial runState: {run_state}");
    println!("has_remote_changes: {}", system_res.has_remote_changes());

    // 启停使服务端发出 /system 的 updated 通知。
    println!("\nStarting sensor to trigger notification...");
    system.start().await?;
    tokio::time::sleep(NOTIFY_WAIT).await;
    println!(
        "has_remote_changes after start: {}",
        system_res.has_remote_changes()
    );

    // 缓存已过期，这次读服务端。
    let run_state = system_res.get::<i64>("/runState").await?;
    println!("Updated runState: {run_state}");
    println!(
        "has_remote_changes after re-read: {}",
        system_res.has_remote_changes()
    );

    system.stop().await?;
    tokio::time::sleep(NOTIFY_WAIT).await;

    println!("\nUnsubscribing...");
    system_res.unsubscribe().await?;
    println!("is_subscribed: {}", system_res.is_subscribed());

    println!("\n=== Subscribe after Creation ===");
    let mut sensor = system.resource(SENSOR_PATH);
    sensor.subscribe().await?;
    sensor.data().await?;
    println!("Sensor resource created, subscribed and cached.");
    println!("is_subscribed: {}", sensor.is_subscribed());

    println!("\n=== Expand Level & Child Enumeration ===");
    let mut engines = system.resource("/scan/engines");
    // 展开层级 0：条目只有链接。
    engines.set_expand_level(0);
    let engine_paths = engines.child_paths(rel::ITEM).await?;
    println!("Engines (expandLevel=0), {} found:", engine_paths.len());
    for path in &engine_paths {
        println!("  {path}");
    }

    // 展开层级 1：条目带数据，直接放进子资源的缓存。改层级即丢弃缓存，children 按新层级重读。
    engines.set_expand_level(1);
    let engine_children = engines.children(rel::ITEM).await?;
    println!(
        "\nEngines (expandLevel=1), {} found:",
        engine_children.len()
    );
    for engine in &engine_children {
        println!("  {}", engine.path());
    }

    println!("\n=== Scanner Enumeration ===");
    let mut laser_engine = system.resource(ENGINE_PATH);
    laser_engine.set_expand_level(1);
    let scanner_paths = laser_engine.child_paths(rel::SCANNER).await?;
    println!("Scanners under {ENGINE_ID}:");
    for path in &scanner_paths {
        println!("  {path}");
    }

    println!("\n=== Caching Behavior ===");
    let mut sys_info = system.resource("/system/info");
    println!("First read (server round-trip)...");
    println!("  appId: {}", sys_info.get::<&str>("/appId").await?);
    println!("Second read (from cache)...");
    println!("  appId: {}", sys_info.get::<&str>("/appId").await?);
    println!("After invalidate (server round-trip)...");
    sys_info.invalidate();
    println!("  appId: {}", sys_info.get::<&str>("/appId").await?);

    // 断开之前取消订阅；未订阅时什么也不做。
    sensor.unsubscribe().await?;
    drop(system);
    Ok(())
}
