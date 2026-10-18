//! 持续接收 GDP 数据直到按下回车。
//!
//! 示例把时间戳（stamp）加入 GDP 输出，等到第一个数据集（最多 3 s）后循环 `receive` 并打印每个数据集；
//! `select!` 同时等数据集与回车（stdin 由一个阻塞线程读），回车即停止设备并退出。方法对任何 GDP 数据类型都适用。

mod common;

use std::time::Duration;

use gopxl::gdp::{Body, Message};

/// 等待第一个数据集的时长。
const FIRST_DATA_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    common::replay_enabled(client).await?;
    common::enable_gdp(client).await?;
    common::add_output(client, &common::scan_source("stamp"), 0).await?;

    let mut gdp = common::connect_gdp(&system).await?;

    println!("Receiving data...");
    println!("Press Enter to close the connection and exit.");
    common::start_if_ready(&system).await?;

    let mut enter = tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new()));
    on_data(&gdp.receive(FIRST_DATA_TIMEOUT).await?);
    loop {
        tokio::select! {
            data_set = gdp.receive(Duration::MAX) => on_data(&data_set?),
            _ = &mut enter => break,
        }
    }

    drop(gdp);
    system.stop().await?;
    Ok(())
}

/// 打印一个数据集。
fn on_data(data_set: &[Message]) {
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        if let Body::Stamp(stamp) = &message.body {
            common::print_cas("Stamp", &message.cas);
            println!("Frame index: {}", stamp.frame_index);
            println!("\tTimestamp: {}", stamp.timestamp);
            println!("\tEncoder: {}", stamp.encoder);
            println!("System Time Sec: {}", stamp.system_time_sec);
            println!("System Time Nanosec: {}", stamp.system_time_nsec);
        } else {
            println!("No stamp found in the message.");
        }
    }
}
