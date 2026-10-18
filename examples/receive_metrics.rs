//! 经流接收系统、扫描器与传感器的 metrics，直到按下 Enter，对应 C++ 示例 `ReceiveMetrics`。
//!
//! 每个流是一条自己的连接（`Client::open_stream`），`select!` 同时等三个流与回车，逐条打印，不需要 C++ 的打印锁。
//! Drop 流句柄即关闭连接、结束流，不发 `cancelStream`。字段按 Gocator 传感器打印（C++ 的 `GOCATOR_SENSOR`）。
//!
//! 与 C++ 的差异：数值原样输出（C++ 的 `Get<int>` 截断 `currentSyncTime` 等超出 `int` 的值）；`Type`、`Status`、
//! `StreamStatus` 输出可读文本（C++ 输出整数与原始字符串）。

mod common;

use gopxl::Value;
use gopxl::control::StreamMessage;

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();
    common::start_if_ready(&system).await?;

    println!("Receiving metrics...");
    println!("Press Enter to close the connection and exit.");
    let mut system_metrics = client
        .open_stream("/system/metrics", Value::object())
        .await?;
    let mut scanner_metrics = client
        .open_stream(
            &format!("{}/metrics", common::SCANNER_PATH),
            Value::object(),
        )
        .await?;
    let mut sensor_metrics = client
        .open_stream(&format!("{}/metrics", common::SENSOR_PATH), Value::object())
        .await?;
    let mut enter = tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new()));
    loop {
        tokio::select! {
            message = system_metrics.recv() => print_metrics(0, &message?),
            message = scanner_metrics.recv() => print_metrics(1, &message?),
            message = sensor_metrics.recv() => print_metrics(2, &message?),
            _ = &mut enter => break,
        }
    }

    drop((system_metrics, scanner_metrics, sensor_metrics));
    system.stop().await?;
    Ok(())
}

/// 打印一条 metrics 消息，`source` 是流的序号（系统、扫描器、传感器），对应 C++ 的 `onMetrics`。
fn print_metrics(source: usize, message: &StreamMessage) {
    let content = &message.payload;
    match source {
        0 => {
            println!("\nApplication Uptime: {}", content["appUpTime"]);
            println!("CPU Cores Usage Average: {}", content["cpuCoresUsedAvg"]);
            println!("Memory Capacity: {}", content["memCapacity"]);
            println!("Memory Used: {}", content["memUsed"]);
        }
        1 => {
            println!("\nCurrent Sync Time: {}", content["currentSyncTime"]);
            println!("Frame Count: {}", content["frameCount"]);
            println!(
                "Processing Latency Average: {}",
                content["processingLatencyAvg"]
            );
            println!(
                "Processing Latency Maximum: {}",
                content["processingLatencyMax"]
            );
        }
        _ => {
            println!("\nCamera Temperature 0: {}", content["cameraTemp0"]);
            println!("CPU Temperature: {}", content["cpuTemp"]);
            println!("Laser Driver Temperature: {}", content["laserDriverTemp"]);
        }
    }
    println!("Type: stream");
    println!("Status: {}", message.status);
    println!("StreamId: {}", message.stream_id);
    println!("StreamStatus: {:?}", message.state);
    println!("Path: {}", message.path);
}
