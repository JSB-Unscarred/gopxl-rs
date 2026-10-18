//! 持续接收 GDP 数据直到按下回车，对应 C++ 示例 `ReceiveAsync`（`ReceiveDataAsync` 加回调）。
//!
//! 示例把时间戳（stamp）加入 GDP 输出，等到第一个数据集（最多 3 s）后循环 `receive` 并打印每个数据集。
//! 方法对任何 GDP 数据类型都适用。
//!
//! 与 C++ 的差异：C++ 在接收线程的回调里打印、主线程跨线程读写普通的 `bool dataReceived`（数据竞争）；这里主线程
//! 分段 `receive` 并打印，回车由另一个线程读取（同 `receive_metrics`）。`gdp::Client` 是 `Send`，要在自己的线程上
//! 接收时把它移过去即可。

mod common;

use std::thread;
use std::time::Duration;

use gopxl::gdp::{Body, Message, RecvError};

/// 等待第一个数据集的时长。
const FIRST_DATA_TIMEOUT: Duration = Duration::from_secs(3);

/// 每次 `receive` 的等待时长，决定按下回车之后多久退出（C++ 接收线程检查关闭的 `CANCEL_QUERY_INTERVAL`）。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    common::replay_enabled(client)?;
    common::enable_gdp(client)?;
    common::add_output(client, &common::scan_source("stamp"), 0)?;

    let mut gdp = common::connect_gdp(&system)?;

    println!("Running callback function to receive data...");
    println!("Press Enter to close the connection and exit.");
    common::start_if_ready(&system)?;

    let enter = thread::spawn(|| std::io::stdin().read_line(&mut String::new()));
    on_data(&gdp.receive(FIRST_DATA_TIMEOUT)?);
    // 数据集跨越多个 POLL_INTERVAL 时，已收到的消息留在 gdp::Client 里，下一次 receive 接着组装。
    while !enter.is_finished() {
        match gdp.receive(POLL_INTERVAL) {
            Ok(data_set) => on_data(&data_set),
            Err(RecvError::Timeout | RecvError::Incomplete { .. }) => {}
            Err(error) => return Err(error.into()),
        }
    }

    drop(gdp);
    system.stop()?;
    Ok(())
}

/// 打印一个数据集，对应 C++ 的回调 `onData`。
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
