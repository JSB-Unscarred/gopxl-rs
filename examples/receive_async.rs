//! 在后台线程上持续接收 GDP 数据，对应 C++ 示例 `ReceiveAsync`（`ReceiveDataAsync` 加回调）。
//!
//! 示例把时间戳（stamp）加入 GDP 输出，后台线程拥有 `GdpClient`、循环 `receive` 并打印每个数据集；主线程等待第一个
//! 数据集（最多 3 s），之后按回车结束。方法对任何 GDP 数据类型都适用。
//!
//! 与 C++ 的差异：线程之间经 channel 通知第一个数据集与停止（C++ 跨线程读写普通的 `bool dataReceived`，是数据竞争）；
//! 后台线程的接收错误在退出时报告（C++ 只写日志，回调不再被调用）。

mod common;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gopxl::gdp::{Body, GdpClient, Message, RecvError};

/// 主线程等待第一个数据集的时长。
const FIRST_DATA_TIMEOUT: Duration = Duration::from_secs(3);

/// 后台线程检查停止信号的间隔，同 C++ 接收线程检查关闭的 `CANCEL_QUERY_INTERVAL`：`GdpClient` 归后台线程所有，
/// 主线程无法关闭它来打断 `receive`，只能由后台线程分段等待。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    common::replay_enabled(client)?;
    common::enable_gdp(client)?;
    let stamp = format!("scan:{}:{}:stamp", common::ENGINE_ID, common::SCANNER_ID);
    common::add_output(client, &stamp, 0)?;

    println!("Connecting to Gocator Protocol...");
    let gdp = common::connect_gdp(&system)?;

    println!("Running callback function to receive data...");
    println!("Press Enter to close the connection and exit.");
    common::start_if_ready(&system)?;

    // 容量 1、满了就丢：只用来通知第一个数据集，之后的通知不积压。
    let (data_tx, data_rx) = mpsc::sync_channel(1);
    // 主线程丢弃发送端即停止。
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let receiver = thread::spawn(move || receive(gdp, &data_tx, &stop_rx));

    match data_rx.recv_timeout(FIRST_DATA_TIMEOUT) {
        Ok(()) => {}
        // 后台线程出错退出时发送端随之释放，报告它的错误。
        Err(RecvTimeoutError::Disconnected) => return join(receiver),
        Err(RecvTimeoutError::Timeout) => {
            return Err("Timeout occurred while waiting for data".into());
        }
    }

    std::io::stdin().read_line(&mut String::new())?;
    drop(stop_tx);
    join(receiver)?;
    system.stop()?;
    Ok(())
}

/// 后台线程：循环接收并打印数据集，每个数据集之后通知 `data`，直到 `stop` 断开。数据集跨越多个 `POLL_INTERVAL`
/// 时，已收到的消息留在 `GdpClient` 里，下一次 `receive` 接着组装。
fn receive(
    mut gdp: GdpClient,
    data: &SyncSender<()>,
    stop: &Receiver<()>,
) -> Result<(), RecvError> {
    while let Err(TryRecvError::Empty) = stop.try_recv() {
        match gdp.receive(POLL_INTERVAL) {
            Ok(data_set) => {
                on_data(&data_set);
                let _ = data.try_send(());
            }
            Err(RecvError::Timeout | RecvError::Incomplete { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// 等待后台线程结束，返回它的接收错误。
fn join(receiver: JoinHandle<Result<(), RecvError>>) -> common::Result {
    receiver
        .join()
        .map_err(|_| "GDP receiver thread panicked")??;
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
