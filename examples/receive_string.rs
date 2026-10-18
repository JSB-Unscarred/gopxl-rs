//! 经 GDP 同步接收工具的字符串输出。
//!
//! 字符串输出要先加入 GDP 输出（见 `configure_tool`）；实时数据与回放数据都可以。消息带标签位置时才打印位置。
//! 结束时停止设备。

mod common;

use gopxl::gdp::Body;

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    common::replay_enabled(client).await?;
    common::enable_gdp(client).await?;
    let mut gdp = common::connect_gdp(&system).await?;
    common::start_if_ready(&system).await?;

    let data_set = gdp.receive(common::RECEIVE_TIMEOUT).await?;
    println!("Total number of messages: {}", data_set.len());
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        match &message.body {
            Body::String(string) => {
                common::print_cas("String", &message.cas);
                println!("\tValue: {}", string.text);
                println!("Decision value: {}", string.decision);
                if let Some([x, y, z]) = string.position {
                    println!("String position (XYZ): {x:.6}, {y:.6}, {z:.6}");
                }
            }
            Body::Null(_) => common::print_cas("Null", &message.cas),
            _ => println!("No string found in the message."),
        }
    }

    drop(gdp);
    system.stop().await?;
    Ok(())
}
