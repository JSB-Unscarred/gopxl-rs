//! 系统的备份与恢复。
//!
//! 把全局设置、全部工作区与作业、回放数据和当前作业归档到当前目录的 `sample_backup.gpbak`，再用这个文件恢复系统；
//! 恢复覆盖设备上的这些内容。收发方式见 `common::download_archive` 与 `common::restore_archive`。

mod common;

use gopxl::Value;

const BACKUP_FILE: &str = "sample_backup.gpbak";

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    // 加上 "support" 并把扩展名改为 .gpsup 即得到支持文件。
    let contents = r#"{"contents":["global","allWorkspaces","allJobs","replay","liveJob"]}"#
        .parse::<Value>()?;

    println!("Receiving backup data...");
    let backup_file = common::download_archive(client, &contents, BACKUP_FILE).await?;

    println!("Restoring system...");
    common::restore_archive(client, contents, &backup_file).await
}
