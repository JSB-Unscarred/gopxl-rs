//! 经流接收系统、扫描器与传感器的 metrics，直到按下 Enter，对应 C++ 示例 `ReceiveMetrics`。
//!
//! 每个流是一条自己的连接（`Client::open_stream`），主线程轮流取出三个流的消息、逐条打印，不需要 C++ 的打印锁。
//! Drop 流句柄即关闭连接、结束流，不发 `cancelStream`。字段按 Gocator 传感器打印（C++ 的 `GOCATOR_SENSOR`）。
//!
//! 与 C++ 的差异：数值原样输出（C++ 的 `Get<int>` 截断 `currentSyncTime` 等超出 `int` 的值）；`Type`、`Status`、
//! `StreamStatus` 输出可读文本（C++ 输出整数与原始字符串）。

mod common;

use std::thread;
use std::time::Duration;

use gopxl::Value;
use gopxl::control::StreamMessage;

/// 每个流每轮等待第一条消息的时长：三个流轮流等待，它决定按下 Enter 之后多久退出。
const POLL: Duration = Duration::from_millis(100);

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();
    common::start_if_ready(&system)?;

    println!("Receiving metrics...");
    println!("Press Enter to close the connection and exit.");
    let streams = [
        "/system/metrics".to_owned(),
        format!("{}/metrics", common::SCANNER_PATH),
        format!("{}/metrics", common::SENSOR_PATH),
    ]
    .iter()
    .map(|path| client.open_stream(path, Value::object()))
    .collect::<Result<Vec<_>, _>>()?;
    let enter = thread::spawn(|| std::io::stdin().read_line(&mut String::new()));
    while !enter.is_finished() {
        for (source, stream) in streams.iter().enumerate() {
            // 等到一条之后取完已到达的消息，忙的流不会因为另外两个流的等待而积压。
            let mut wait = POLL;
            while let Some(message) = stream.recv_timeout(wait)? {
                print_metrics(source, &message);
                wait = Duration::ZERO;
            }
        }
    }

    drop(streams);
    system.stop()?;
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
