//! 用资源层读取 schema：打印 `/system` 的顶层属性、`runState` 与传感器、扫描器几个参数的 schema 节点，在客户端按
//! schema 校验曝光值，再开启写入前的校验修改曝光模式。
//!
//! 会把传感器的曝光模式设为单次（0）。路径不在 schema 中、或 schema 节点缺少 `type` 时报错退出；顶层属性的 `type`
//! 不是字符串（可空类型写作数组）时打印其 JSON 文本。

mod common;

use common::{
    EXPOSURE_MODE_PATH, SCANNER_PATH, SENSOR_PATH, SINGLE_EXPOSURE_PATH, TRIGGER_SOURCE_PATH,
};
use gopxl::resource::Resource;
use gopxl::{Error, Value, schema};

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;

    println!("\n=== Full Schema for /system ===");
    let mut system_res = system.resource("/system");
    let full_schema = system_res.schema().await?;
    let ty = full_schema
        .pointer("/type")
        .ok_or("the /system schema has no type")?;
    println!("Schema type: {ty}");
    if let Some(properties) = full_schema.pointer("/properties").and_then(Value::as_map) {
        println!("Top-level properties:");
        for (name, property) in properties {
            let ty = property
                .pointer("/type")
                .map_or_else(|| "unknown".to_owned(), common::text);
            println!("  {name} ({ty})");
        }
    }

    println!("\n=== Sub-Schema for /runState ===");
    print_schema("runState", schema_for(&mut system_res, "/runState").await?)?;

    println!("\n=== Sensor Property Schemas ===");
    let mut sensor = system.resource(SENSOR_PATH);
    print_schema(
        "singleExposure",
        schema_for(&mut sensor, SINGLE_EXPOSURE_PATH).await?,
    )?;
    print_schema(
        "exposureMode",
        schema_for(&mut sensor, EXPOSURE_MODE_PATH).await?,
    )?;

    println!("\n=== Pre-flight Validation ===");
    // 依次是范围内的值、可能超出范围的值与类型不符的值。
    for value in [
        Value::from(1000),
        Value::from(999_999),
        Value::from("not_a_number"),
    ] {
        let issues = sensor.validate(SINGLE_EXPOSURE_PATH, &value).await?;
        let verdict = if issues.is_empty() { "PASS" } else { "FAIL" };
        println!("Validate exposure={value}: {verdict}");
        for issue in &issues {
            println!("  Error: {issue}");
        }
    }

    println!("\n=== Automatic Setter Validation ===");
    sensor.set_validation(true);
    println!("Validation enabled: {}", sensor.validation_enabled());

    // 只捕获校验错误；服务端拒绝等其他错误中止示例。
    match sensor.set(EXPOSURE_MODE_PATH, 0).await {
        Ok(()) => println!("set(exposureMode, 0): OK"),
        Err(e @ Error::Validation { .. }) => eprintln!("Unexpected validation error: {e}"),
        Err(e) => return Err(e.into()),
    }

    // 99 不在曝光模式的 enum 中，校验失败，请求不发出。
    match sensor.set(EXPOSURE_MODE_PATH, 99).await {
        Ok(()) => println!("set(exposureMode, 99): OK (unexpected)"),
        Err(ref e @ Error::Validation { ref issues, .. }) => {
            println!("set(exposureMode, 99): Caught validation error");
            println!("  Message: {e}");
            for issue in issues {
                println!("  Detail: {issue}");
            }
        }
        Err(e) => return Err(e.into()),
    }
    sensor.set_validation(false);

    println!("\n=== System-Level Auto-Validation ===");
    let mut scanner = system.resource(SCANNER_PATH);
    println!(
        "scanner validation enabled: {}",
        scanner.validation_enabled()
    );

    println!("\n=== Scanner Schema ===");
    print_schema(
        "triggerSettings/source",
        schema_for(&mut scanner, TRIGGER_SOURCE_PATH).await?,
    )?;
    Ok(())
}

/// `resource` 的 schema 中 `pointer` 处的节点；不在 schema 中时报错。
async fn schema_for<'a>(resource: &'a mut Resource, pointer: &str) -> common::Result<&'a Value> {
    let path = resource.path().to_owned();
    let node = schema::schema_for_path(resource.schema().await?, pointer);
    node.ok_or_else(|| format!("{pointer} is not in the schema of {path}").into())
}

/// 打印 schema 节点的类型与存在的约束、注解；缺少 `type` 时报错。
fn print_schema(label: &str, schema: &Value) -> common::Result {
    println!("{label}:");
    let ty = schema
        .pointer("/type")
        .ok_or_else(|| format!("the schema of {label} has no type"))?;
    println!("  type:     {ty}");
    let keys = [
        "readOnly", "minimum", "maximum", "enum", "enumText", "units", "title",
    ];
    for key in keys {
        if let Some(value) = schema.pointer(key) {
            let name = format!("{key}:");
            println!("  {name:<10}{value}");
        }
    }
    Ok(())
}
