//! 经 GDP 同步接收轮廓，对应 C++ 示例 `ReceiveProfile`：统一轮廓与轮廓点云的原始值按偏移与分辨率换算为毫米坐标。
//!
//! 需要线扫传感器；实时数据时先切换到轮廓扫描模式，回放数据直接使用。与 C++ 的差异：按 source 精确匹配判断轮廓是否
//! 已在 GDP 输出中（见 `common::ensure_output`）。

mod common;

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    if !common::replay_enabled(client).await? {
        common::set_scan_mode(client, common::SCAN_MODE_PROFILE, "Profile").await?;
    }

    common::enable_intensity(client).await?;

    common::enable_gdp(client).await?;
    common::ensure_output(client, &common::scan_source("topUniformProfile")).await?;

    let mut gdp = common::connect_gdp(&system).await?;
    common::start_if_ready(&system).await?;

    let data_set = gdp.receive(common::RECEIVE_TIMEOUT).await?;
    println!("Total number of messages: {}", data_set.len());
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        if !common::print_profile(message) {
            println!("No profile found in the message.");
        }
    }

    drop(gdp);
    system.stop().await?;
    Ok(())
}
