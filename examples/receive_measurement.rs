//! 经 GDP 同步接收工具的测量值，对应 C++ 示例 `ReceiveMeasurement`。
//!
//! 测量输出要先加入 GDP 输出（见 `configure_tool`）；实时数据与回放数据都可以。

mod common;

use gopxl::gdp::Body;

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    common::replay_enabled(client)?;
    common::enable_gdp(client)?;
    let mut gdp = common::connect_gdp(&system)?;
    common::start_if_ready(&system)?;

    let data_set = gdp.receive(common::RECEIVE_TIMEOUT)?;
    println!("Total number of messages: {}", data_set.len());
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        match &message.body {
            Body::Measurement(measurement) => {
                common::print_cas("Measurement", &message.cas);
                println!("\tValue: {}", measurement.value);
                println!("\tDecision: {}", measurement.decision);
            }
            Body::Null(_) => common::print_cas("Null", &message.cas),
            _ => println!("No measurement found in the message."),
        }
    }

    drop(gdp);
    system.stop()?;
    Ok(())
}
