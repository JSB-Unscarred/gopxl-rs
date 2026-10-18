//! 作业文件的保存、改名、下载、上传与加载。
//!
//! 会修改设备上的作业：把当前作业保存为 `SDK-demo` 并改名为 `SDK-demo-job`，下载到当前目录的 `sample_job.gpjob`，
//! 再以 `SDK-local-job-demo` 上传并加载。后两个名字已存在时改名或上传失败，重复运行前先在设备上删除这两个作业。

mod common;

use std::time::Duration;

use gopxl::Value;
use gopxl::control::{Client, Method, Request};

const JOBS_PATH: &str = "/jobs";
const JOB_FILES_PATH: &str = "/jobs/files";
const SAVE_JOB_PATH: &str = "/jobs/commands/save";
const LOAD_JOB_PATH: &str = "/jobs/commands/load";
const RENAME_JOB_PATH: &str = "/jobs/commands/rename";
/// 改名后作业的文件内容。
const READ_DATA_PATH: &str = "/jobs/files/SDK-demo-job/data";
const JOB_FILE: &str = "sample_job.gpjob";
/// 上传作业的超时：文件传输连接上连续无数据的时长，与默认请求超时同为 3 s。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> common::Result {
    let system = common::connect().await?;
    common::verify_connection(&system).await?;
    let client = system.client();

    // 演示用途，列出作业失败只报告、继续后面的步骤。
    if let Err(e) = list_jobs(client).await {
        eprintln!("Error: {e}");
    }

    let save = r#"{"name":"SDK-demo"}"#.parse::<Value>()?;
    println!("Saving current job as {}...", save["name"]);
    client.call(SAVE_JOB_PATH, save).response().await?;

    let rename = r#"{"sourceName":"SDK-demo","destName":"SDK-demo-job"}"#.parse::<Value>()?;
    println!("Renaming job to {}...", rename["destName"]);
    // 改名失败（例如目标已存在）只报告，继续下载已有的同名作业。
    if let Err(e) = client.call(RENAME_JOB_PATH, rename).response().await {
        eprintln!("Error: {e}");
    }

    let job_file = std::path::absolute(JOB_FILE)?;
    let download = Request::new(Method::Read, READ_DATA_PATH);
    let job = client.transfer(download, common::RECEIVE_TIMEOUT).await?;
    let content = job.payload["content"]
        .as_bytes()
        .ok_or("job data has no binary \"content\"")?;
    std::fs::write(&job_file, content)?;
    println!("Job downloaded to {}", job_file.display());
    drop(job);

    let mut upload = Value::object();
    upload.set("fromLive", false)?;
    upload.set("name", "SDK-local-job-demo")?;
    upload.set("content", std::fs::read(&job_file)?)?;
    println!(
        "Uploading job as {} from {}...",
        upload["name"],
        job_file.display()
    );
    let create = Request::with_payload(Method::Create, JOB_FILES_PATH, upload);
    client.transfer(create, UPLOAD_TIMEOUT).await?;

    let load = r#"{"name":"SDK-local-job-demo"}"#.parse::<Value>()?;
    client.call(LOAD_JOB_PATH, load).response().await?;
    let jobs = read_expanded(client, JOBS_PATH).await?;
    println!("Loaded job: {}", common::text(&jobs["loadedJob"]));
    Ok(())
}

/// 打印设备上已保存的作业名。
async fn list_jobs(client: &Client) -> common::Result {
    let files = read_expanded(client, JOB_FILES_PATH).await?;
    if let Some(jobs) = files.pointer("/_embedded/item") {
        println!("List of saved jobs:");
        for job in jobs.as_array().unwrap_or_default() {
            println!("{}", job["jobName"]);
        }
    } else {
        println!("Currently GoPxL does not contain any saved job.");
    }
    Ok(())
}

/// `read path`，参数 `expandLevel: 1` 把内嵌资源展开一层（作业在列表中只部分表示；省略等同于 0）。
async fn read_expanded(client: &Client, path: &str) -> common::Result<Value> {
    let mut request = Request::new(Method::Read, path);
    request.args.set("expandLevel", 1)?;
    Ok(client.send(request).response().await?.payload)
}
