//! 作业文件的保存、改名、下载、上传与加载，对应 C++ 示例 `SaveJob`。
//!
//! 会修改设备上的作业：把当前作业保存为 `SDK-demo` 并改名为 `SDK-demo-job`，下载到 `sample_job.gpjob`，再以
//! `SDK-local-job-demo` 上传并加载。后两个名字已存在时改名或上传失败，重复运行前先在设备上删除这两个作业。
//!
//! 与 C++ 的差异：作业文件的下载与上传走文件传输连接（`Handle::transfer`）；下载的读请求不再带上一步陈旧的
//! `{"name": ...}` 载荷。

mod common;

use std::time::Duration;

use gopxl::Value;
use gopxl::control::{Handle, Method, Request};

const JOBS_PATH: &str = "/jobs";
const JOB_FILES_PATH: &str = "/jobs/files";
const SAVE_JOB_PATH: &str = "/jobs/commands/save";
const LOAD_JOB_PATH: &str = "/jobs/commands/load";
const RENAME_JOB_PATH: &str = "/jobs/commands/rename";
/// 改名后作业的文件内容。
const READ_DATA_PATH: &str = "/jobs/files/SDK-demo-job/data";
const JOB_FILE: &str = "sample_job.gpjob";
/// 上传作业的读超时，即 C++ 的 `REST_COMMAND_TIMEOUT_MSEC`。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(3);

fn main() -> common::Result {
    let system = common::connect()?;
    common::verify_connection(&system)?;
    let client = system.client();

    // 与 C++ 相同，列表失败只报告、不中止。
    if let Err(e) = list_jobs(client) {
        eprintln!("Error: {e}");
    }

    let save = r#"{"name":"SDK-demo"}"#.parse::<Value>()?;
    println!("Saving current job as {}...", save["name"]);
    client.call(SAVE_JOB_PATH, save).response()?;

    let rename = r#"{"sourceName":"SDK-demo","destName":"SDK-demo-job"}"#.parse::<Value>()?;
    println!("Renaming job to {}...", rename["destName"]);
    // 与 C++ 相同，改名失败（例如目标已存在）不中止，继续下载已有的同名作业。
    if let Err(e) = client.call(RENAME_JOB_PATH, rename).response() {
        eprintln!("Error: {e}");
    }

    let job_file = std::path::absolute(JOB_FILE)?;
    let download = Request::new(Method::Read, READ_DATA_PATH);
    let job = client.transfer(download, common::RECEIVE_TIMEOUT)?;
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
    client.transfer(create, UPLOAD_TIMEOUT)?;

    let load = r#"{"name":"SDK-local-job-demo"}"#.parse::<Value>()?;
    client.call(LOAD_JOB_PATH, load).response()?;
    let jobs = read_expanded(client, JOBS_PATH)?;
    println!("Loaded job: {}", common::text(&jobs["loadedJob"]));
    Ok(())
}

/// 打印设备上已保存的作业名。
fn list_jobs(client: &Handle) -> common::Result {
    let files = read_expanded(client, JOB_FILES_PATH)?;
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
fn read_expanded(client: &Handle, path: &str) -> common::Result<Value> {
    let mut request = Request::new(Method::Read, path);
    request.args.set("expandLevel", 1)?;
    Ok(client.send(request).response()?.payload)
}
