//! 用资源层订阅与缓存：订阅 `/system` 并观察启停带来的远端修改，创建传感器资源后随即订阅，按展开层级枚举引擎与
//! 扫描器，演示读缓存与失效，对应 C++ 示例 `ResourceSubscriptions`。
//!
//! 会启动再停止设备。
//!
//! 与 C++ 的差异：订阅表达式是 `<路径>/*`，子资源的通知也会到达（C++ 订阅精确路径），`unSub` 写出后不等应答；
//! `has_remote_changes` 在从未读取时、以及同一连接上发出任何变更请求之后也为 `true`，所以启动后总是输出 `true`
//! 并重读 `/system`（C++ 对已订阅的资源只看通知，通知 0.5 s 内未到达时输出 `false`、沿用缓存）；自动订阅由调用方
//! 完成：创建传感器资源后显式 `subscribe`，这一节的标题与输出相应改动；每个 `Resource` 各有缓存，扫描器一节新建
//! 引擎资源、读一次引擎（C++ 的管理器返回上一节 `Children` 填好缓存的同一对象，不发请求）；rel 只有一个条目时
//! 服务端写出单个对象（例如只有一个扫描器），C++ 当作空表，这里照常列出。

mod common;

use std::thread;
use std::time::Duration;

use common::{ENGINE_ID, ENGINE_PATH, SENSOR_PATH};
use gopxl::resource::rel;

/// 启停之后等待通知到达的时间（C++ 的 `kThread_Sleep(500'000)`）。
const NOTIFY_WAIT: Duration = Duration::from_millis(500);

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;

    println!("\n=== Manual Subscription ===");
    let mut system_res = system.resource("/system");
    println!("Subscribing to /system...");
    system_res.subscribe()?;
    println!("IsSubscribed: {}", system_res.is_subscribed());

    let run_state = system_res.get::<i64>("/runState")?;
    println!("Initial runState: {run_state}");
    println!("HasRemoteChanges: {}", system_res.has_remote_changes());

    // 启停使服务端发出 /system 的 updated 通知。
    println!("\nStarting sensor to trigger notification...");
    system.start()?;
    thread::sleep(NOTIFY_WAIT);
    println!(
        "HasRemoteChanges after start: {}",
        system_res.has_remote_changes()
    );

    // 缓存已过期，这次读服务端。
    let run_state = system_res.get::<i64>("/runState")?;
    println!("Updated runState: {run_state}");
    println!(
        "HasRemoteChanges after re-read: {}",
        system_res.has_remote_changes()
    );

    system.stop()?;
    thread::sleep(NOTIFY_WAIT);

    println!("\nUnsubscribing...");
    system_res.unsubscribe();
    println!("IsSubscribed: {}", system_res.is_subscribed());

    println!("\n=== Subscribe on Creation ===");
    let mut sensor = system.resource(SENSOR_PATH);
    sensor.subscribe()?;
    sensor.data()?;
    println!("Sensor resource created, subscribed and cached.");
    println!("IsSubscribed: {}", sensor.is_subscribed());

    println!("\n=== Expand Level & Child Enumeration ===");
    let mut engines = system.resource("/scan/engines");
    // 展开层级 0：条目只有链接。
    engines.set_expand_level(0);
    let engine_paths = engines.child_paths(rel::ITEM)?;
    println!("Engines (expandLevel=0), {} found:", engine_paths.len());
    for path in &engine_paths {
        println!("  {path}");
    }

    // 展开层级 1：条目带数据，直接放进子资源的缓存。改层级即丢弃缓存，children 按新层级重读。
    engines.set_expand_level(1);
    let engine_children = engines.children(rel::ITEM)?;
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
    let scanner_paths = laser_engine.child_paths(rel::SCANNER)?;
    println!("Scanners under {ENGINE_ID}:");
    for path in &scanner_paths {
        println!("  {path}");
    }

    println!("\n=== Caching Behavior ===");
    let mut sys_info = system.resource("/system/info");
    println!("First read (server round-trip)...");
    println!("  appId: {}", sys_info.get::<&str>("/appId")?);
    println!("Second read (from cache)...");
    println!("  appId: {}", sys_info.get::<&str>("/appId")?);
    println!("After InvalidateCache (server round-trip)...");
    sys_info.invalidate();
    println!("  appId: {}", sys_info.get::<&str>("/appId")?);

    // 断开（C++ 的 `Disconnect`）之前取消订阅；未订阅时什么也不做。
    sensor.unsubscribe();
    drop(system);
    Ok(())
}
