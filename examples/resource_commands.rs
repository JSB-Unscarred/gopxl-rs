//! 用资源层调用命令：列出扫描器的命令，保存并改名作业，浏览作业文件，清除并重新对准扫描器（轮询对准状态），最后删除
//! 演示作业，对应 C++ 示例 `ResourceCommands`。
//!
//! 会修改设备上的作业与对准结果；对准要求传感器上有扫描数据，否则只打印 `Alignment failed`、继续清理。
//!
//! 与 C++ 的差异：对准变换的浮点数按最短往返表示输出（C++ 的 `operator<<` 取 6 位有效数字）。

mod common;

use std::thread;
use std::time::Duration;

use common::{ALIGNED, ALIGNING, SCANNER_PATH, SENSOR_PATH, UNALIGNED};
use gopxl::resource::{Resource, rel};
use gopxl::{System, Value};

const JOBS_PATH: &str = "/jobs";
const JOB_FILES_PATH: &str = "/jobs/files";
const DEMO_JOB: &str = "SDK_resource_api_demo";
const RENAMED_JOB: &str = "SDK_resource_api_renamed";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;

    let mut scanner = system.resource(SCANNER_PATH);
    let jobs = system.resource(JOBS_PATH);
    let mut job_files = system.resource(JOB_FILES_PATH);

    println!("\n=== Scanner Commands ===");
    for name in scanner.command_names()? {
        println!("  {name}");
    }

    println!("\n=== Save Job ===");
    let mut save = Value::object();
    save.set("name", DEMO_JOB)?;
    println!("Saving job as \"{DEMO_JOB}\"...");
    jobs.command("save", save).response()?;
    println!("Job saved.");

    println!("\n=== Rename Job ===");
    let mut rename = Value::object();
    rename.set("sourceName", DEMO_JOB)?;
    rename.set("destName", RENAMED_JOB)?;
    println!("Renaming job...");
    match jobs.command("rename", rename).response() {
        Ok(_) => println!("Job renamed to \"{RENAMED_JOB}\"."),
        Err(e) => {
            eprintln!("Rename failed: {e}");
            eprintln!("Destination name may already exist.");
        }
    }

    println!("\n=== Job Files ===");
    // 展开一层：条目就是各作业文件的数据，children 直接放进子资源的缓存，读 jobName 不再发请求。
    job_files.set_expand_level(1);
    let files = job_files.children(rel::ITEM)?;
    println!("Saved jobs ({}):", files.len());
    for mut file in files {
        let name = file.get::<String>("/jobName")?;
        println!("  {name} ({})", file.path());
    }

    println!("\n=== Clear Alignment ===");
    println!("Clearing alignment...");
    scanner.command("clearAlign", Value::object()).response()?;
    println!("Alignment cleared.");

    println!("\n=== Run Alignment ===");
    println!("Starting alignment...");
    // 与 C++ 相同，对准、轮询与读变换中任何一步失败都只报告，继续清理。
    if let Err(e) = align(&system, &scanner) {
        eprintln!("Alignment failed: {e}");
        eprintln!("Ensure scan data is available on the sensor.");
    }

    println!("\n=== Cleanup ===");
    // 作业不存在（notFound）也算删除成功。
    match system
        .resource(format!("{JOB_FILES_PATH}/{RENAMED_JOB}"))
        .delete()
    {
        Ok(()) => println!("Demo job file deleted."),
        Err(e) => println!("Note: Could not delete demo job ({e})"),
    }
    Ok(())
}

/// 执行对准，每秒读一次对准状态直到不再是“对准中”，再打印传感器的对准变换。
fn align(system: &System, scanner: &Resource) -> common::Result {
    scanner
        .command("align", Value::object())
        .timeout(common::EXTENDED_TIMEOUT)
        .response()?;

    // align 的应答可能早于对准完成。轮询期间本连接不发变更请求、缓存不会自动过期，所以每次先丢弃缓存再读。
    let mut alignment = scanner.child("alignment");
    let state = loop {
        thread::sleep(Duration::from_secs(1));
        alignment.invalidate();
        let state = alignment.get::<i64>("/alignState")?;
        if state != ALIGNING {
            break state;
        }
    };
    // C++ 的 alignmentStates.at() 对未知状态抛异常，同样归入对准失败。
    let label = match state {
        UNALIGNED => "Not Aligned",
        ALIGNED => "Aligned",
        _ => return Err(format!("unknown alignment state {state}").into()),
    };
    println!("Alignment state: {label}");

    let mut calibration = system.resource(format!("{SENSOR_PATH}/transform"));
    println!("Alignment transform: {{");
    let mut number = |pointer: &str| calibration.get::<f64>(pointer);
    println!("  X angle:  {},", number("/xAngle")?);
    println!("  X offset: {},", number("/xOffset")?);
    println!("  Y angle:  {},", number("/yAngle")?);
    println!("  Y offset: {},", number("/yOffset")?);
    println!("  Z angle:  {},", number("/zAngle")?);
    println!("  Z offset: {}", number("/zOffset")?);
    println!("}}");
    Ok(())
}
