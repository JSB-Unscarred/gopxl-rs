//! 把第二台传感器加入传感器组，修改它在布局中的位置，再把它移除。
//!
//! 需要网络上有多台 LMI Laser Line Profiler 传感器，固件兼容且不属于其他传感器组。第二台按序列号加入，序列号取自
//! 环境变量 `GOPXL_SERIAL2`，默认 `62984`。引擎下扫描器为空时先创建一个。

mod common;

use gopxl::Value;

const SENSOR_2_SERIAL_NO: &str = "62984";

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    let engine = client.read(common::ENGINE_PATH).response().await?.payload;
    // `_embedded` 中只有一项时，它可能是对象而不是数组（例如 /controls/gocator 的 go:datasources）；所以缺失、
    // 空数组与空对象都按扫描器为空处理。
    let empty = match &engine["_embedded"]["go:scanner"] {
        Value::Nil => true,
        Value::Array(items) => items.is_empty(),
        Value::Map(map) => map.is_empty(),
        _ => false,
    };
    if empty {
        println!("Scanner not present, creating scanner");
        client
            .create(common::SCANNERS_PATH, Value::object())
            .response()
            .await?;
    } else {
        println!("A Sensor Group exists.");
    }

    let serial = std::env::var("GOPXL_SERIAL2").unwrap_or_else(|_| SENSOR_2_SERIAL_NO.to_owned());
    println!("Adding sensor {serial} to the sensor group...");
    let mut payload = Value::object();
    payload.set("/serialNumber", serial)?;
    client
        .create(common::SENSORS_PATH, payload)
        .response()
        .await?;

    let layout_path = format!("{}/layout", common::SCANNER_PATH);
    let mut layout = client.read(&layout_path).response().await?.payload;
    print_layout(&layout)?;

    // orientation：0 正向、1 反向；row：0 上排（A）、1 下排（B）；column：列序号。
    layout.set("/grid/sensors/1/row", 0)?;
    layout.set("/grid/sensors/1/column", 1)?;
    layout.set("/grid/sensors/1/orientation", 0)?;
    println!("Updating sensor layout...");
    client.update(&layout_path, layout).response().await?;

    print_layout(&client.read(&layout_path).response().await?.payload)?;

    // 第二台传感器加入后的资源。
    let sensor_1 = format!("{}/sensor-1", common::SENSORS_PATH);
    println!("Removing {sensor_1} from sensor group...");
    client.delete(&sensor_1).response().await?;
    Ok(())
}

/// 打印布局中每台传感器的位置。
fn print_layout(layout: &Value) -> common::Result {
    let sensors = layout["grid"]["sensors"]
        .as_array()
        .ok_or("layout has no /grid/sensors")?;
    println!("{}", "*".repeat(32));
    for sensor in sensors {
        println!("Sensor ID: {}", sensor["sensorId"]);
        println!("Row: {}, Column: {}", sensor["row"], sensor["column"]);
        println!("Orientation: {}", sensor["orientation"]);
        println!("Multiplexing Bank: {}\n", sensor["multiplexingBank"]);
    }
    Ok(())
}
