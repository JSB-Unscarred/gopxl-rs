//! 经 GDP 同步接收轮廓，对应 C++ 示例 `ReceiveProfile`：统一轮廓与轮廓点云的原始值按偏移与分辨率换算为毫米坐标。
//!
//! 需要线扫传感器；实时数据时先切换到轮廓扫描模式，回放数据直接使用。与 C++ 的差异：按 source 精确匹配判断轮廓是否
//! 已在 GDP 输出中（见 `common::ensure_output`）。

mod common;

use common::NULL;
use gopxl::gdp::Body;
use gopxl::value::Value;

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    if !common::replay_enabled(client)? {
        common::set_scan_mode(client, common::SCAN_MODE_PROFILE, "Profile")?;
    }

    println!("Enabling intensity...");
    let intensity = r#"{"parameters":{"scanModeSettings":{"intensityEnabled":true}}}"#;
    client
        .update(common::SCANNER_PATH, intensity.parse::<Value>()?)
        .response()?;

    common::enable_gdp(client)?;
    // 共焦引擎（`LMIConfocalLineProfiler`）的统一轮廓带 `Layer0` 后缀。
    let source = format!(
        "scan:{}:{}:topUniformProfile",
        common::ENGINE_ID,
        common::SCANNER_ID
    );
    common::ensure_output(client, &source)?;

    println!("Connecting to Gocator Protocol...");
    let mut gdp = common::connect_gdp(&system)?;
    common::start_if_ready(&system)?;

    let data_set = gdp.receive(common::RECEIVE_TIMEOUT)?;
    println!("Total number of messages: {}", data_set.len());
    for (i, message) in data_set.iter().enumerate() {
        println!("{} GDP Output Source {}", "-".repeat(64), i + 1);
        match &message.body {
            Body::UniformProfile(profile) => {
                common::print_cas("Uniform Profile", &message.cas);
                // 统一轮廓只发送 z，x 由点的下标换算。
                let points: Vec<[f64; 2]> = (0u32..)
                    .zip(&profile.ranges)
                    .filter(|&(_, &z)| z != NULL)
                    .map(|(x, &z)| common::profile_mm(&profile.attrs, [x.into(), z.into()]))
                    .collect();
                println!("Profile points count: {}", profile.ranges.len());
                println!("Intensity count: {}", profile.intensity.len());
                println!("Valid points count: {}", points.len());
            }
            Body::ProfilePointCloud(profile) => {
                common::print_cas("Point Cloud Profile", &message.cas);
                let points: Vec<[f64; 2]> = profile
                    .points
                    .iter()
                    .filter(|&&[x, _]| x != NULL)
                    .map(|point| common::profile_mm(&profile.attrs, point.map(f64::from)))
                    .collect();
                println!("Profile points count: {}", profile.points.len());
                println!("Intensity width: {}", profile.intensity.len());
                println!("Valid points count: {}", points.len());
            }
            _ => println!("No profile found in the message."),
        }
    }

    drop(gdp);
    system.stop()?;
    Ok(())
}
