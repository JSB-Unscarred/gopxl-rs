//! 系统的备份与恢复，对应 C++ 示例 `BackupRestore`。
//!
//! 把全局设置、全部工作区与作业、回放数据和当前作业归档到 `sample_backup.gpbak`，再用这个文件恢复系统；恢复覆盖
//! 设备上的这些内容。
//!
//! 与 C++ 的差异：归档与恢复走文件传输连接（`Handle::transfer`）；恢复的超时从 3 s 改为 30 s
//! （`common::EXTENDED_TIMEOUT`，C++ 注明它用于 backup / restore）。

mod common;

use gopxl::Value;
use gopxl::control::{Method, Request};

const BACKUP_PATH: &str = "/system/commands/archive";
const RESTORE_PATH: &str = "/system/commands/restore";
const BACKUP_FILE: &str = "sample_backup.gpbak";

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    // 加上 "support" 并把扩展名改为 .gpsup 即得到支持文件。
    let contents = r#"{"contents":["global","allWorkspaces","allJobs","replay","liveJob"]}"#
        .parse::<Value>()?;

    println!("Receiving backup data...");
    let archive = Request {
        payload: contents.clone(),
        ..Request::new(Method::Call, BACKUP_PATH)
    };
    let response = client.transfer(archive, common::RECEIVE_TIMEOUT)?;
    let data = response.payload["data"]
        .as_bytes()
        .ok_or("archive response has no binary \"data\"")?;

    println!("Downloading backup file...");
    let backup_file = std::path::absolute(BACKUP_FILE)?;
    std::fs::write(&backup_file, data)?;
    println!("File location: {}", backup_file.display());
    // 归档可能超过 1 GiB，恢复期间不再占着下载的那一份。
    drop(response);

    println!("Restoring system...");
    let mut restore = Request {
        payload: contents,
        ..Request::new(Method::Call, RESTORE_PATH)
    };
    restore.payload.set("data", std::fs::read(&backup_file)?)?;
    println!("Restoring system from {}...", backup_file.display());
    client.transfer(restore, common::EXTENDED_TIMEOUT)?;
    Ok(())
}
