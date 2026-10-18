//! 发现网络上的 GoPxL 实例（传感器、PC 或 GoMax），打印各实例的信息与它可见的传感器。
//!
//! 只用 GoPxL 发现协议，Gocator 6.x 经典设备不在结果中。Accelerated 传感器由另一台 PC 上的 GoPxL 控制，上报的控制
//! 端口为 0，不能直接连接，只打印发现信息。连接某个实例失败时返回错误；读取它的传感器出错（包括可见传感器为空）时
//! 打印错误并继续下一个实例。

mod common;

use std::net::Ipv4Addr;

use gopxl::System;
use gopxl::discovery::{self, Instance};

/// 实例上可见的传感器。
const VISIBLE_SENSORS_PATH: &str = "/scan/visibleSensors/";

#[tokio::main]
async fn main() -> common::Result {
    let instances = discovery::discover(common::DISCOVER_TIMEOUT).await?;
    println!("Number of sensors on the network: {}", instances.len());
    if instances.is_empty() {
        return Err(
            "No sensors found, make sure sensors or GoPxl on PC/GoMax are available and connected."
                .into(),
        );
    }

    for (i, instance) in instances.iter().enumerate() {
        print_instance(i + 1, instance);
        // 由另一台 PC 控制的实例端口为 0，取不到控制通道地址。
        let Some(addr) = instance.control_addr() else {
            continue;
        };
        let system = System::connect(addr.into()).await?;
        if let Err(e) = print_sensors(&system).await {
            println!("Error: {e}");
        }
    }
    Ok(())
}

/// 打印发现到的一个实例；地址缺失时输出 `0.0.0.0`。
fn print_instance(number: usize, instance: &Instance) {
    let ip = |addr: Option<Ipv4Addr>| addr.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let name = if instance.app_name.is_empty() {
        &instance.app_id
    } else {
        &instance.app_name
    };
    let id_label = if instance.is_remote {
        "Remote Controller Application ID"
    } else {
        "Application ID"
    };
    // HMI 状态的取值表只出现在这个示例中。
    let hmi_status = match instance.hmi_status {
        Some(0) => "RUNNING",
        Some(1) => "STOPPED",
        Some(2) => "STARTING",
        Some(3) => "STOPPING",
        Some(4) => "FAILED_TO_START",
        Some(5) => "FAILED_TO_STOP",
        _ => "UNKNOWN",
    };
    println!();
    println!("{} GoPxL instance {number}: '{name}'", "*".repeat(52));
    println!("Is remote: {}", yes_no(instance.is_remote));
    println!("IP Address: {}", ip(instance.address));
    println!("Gateway: {}", ip(instance.gateway));
    println!("Mask: {}", ip(instance.mask));
    println!("DHCP: {}", if instance.dhcp { "True" } else { "False" });
    println!("{id_label}: {}", instance.app_id);
    println!("Application Name: {}", instance.app_name);
    println!("Application Version: {}", instance.app_version);
    println!("Control Port: {}", instance.control_port);
    println!("GDP Port: {}", instance.gdp_port);
    println!("Web Port: {}", instance.web_port);
    println!("IP Conflict: {}", yes_no(instance.address_conflict));
    println!("HMI Status: {hmi_status}");
}

/// 打印实例可见、且在它的扫描引擎下有资源路径的传感器。网关、掩码与 DHCP 取自实例自身的第一块网卡。
async fn print_sensors(system: &System) -> common::Result {
    let client = system.client();
    let visible = client.read(VISIBLE_SENSORS_PATH).response().await?.payload;
    let sensors = visible["sensors"].as_array().unwrap_or_default();
    if sensors.is_empty() {
        return Err(
            format!("Unable to read any visible sensors at path: {VISIBLE_SENSORS_PATH}").into(),
        );
    }

    // 网卡配置属于本实例，与传感器无关，读一次。
    let ip_config = client
        .read(common::IP_CONFIG_PATH)
        .response()
        .await?
        .payload;
    let interface = &ip_config["interfaces"][0];
    let mut count = 0;
    for sensor in sensors {
        let serial = sensor["serialNumber"]
            .as_str()
            .ok_or("visible sensor has no \"serialNumber\"")?;
        let Some(sensor_path) = system.sensor_path(serial).await? else {
            continue;
        };
        count += 1;
        println!("\n{} Sensor {count}", "*".repeat(22));
        println!(
            "Local sensor: {}",
            yes_no(sensor["isLocal"].as_bool() == Some(true))
        );
        println!(
            "IP Address: {}",
            common::text(&sensor["parameters"]["ipAddress"])
        );
        println!(
            "Gateway Address: {}",
            common::text(&interface["gatewayAddress"])
        );
        println!("Subnet Mask: {}", common::text(&interface["subnetMask"]));
        println!("DHCP: {}", interface["dhcp"]);
        println!("Serial Number: {serial}");
        println!("Model: {}", common::text(&sensor["model"]));
        println!("Engine ID: {}", common::text(&sensor["engineId"]));
        println!("Sensor Path: {sensor_path}");
    }
    Ok(())
}

/// 布尔值输出为 `Yes` / `No`。
fn yes_no(flag: bool) -> &'static str {
    if flag { "Yes" } else { "No" }
}
