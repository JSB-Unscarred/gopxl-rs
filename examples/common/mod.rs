//! 示例共用的部分，对应 C++ 示例的 `SampleUtils`：设备地址、路径与超时常量、连接检查、GDP 输出的设置、数据集的
//! 打印与 BMP 写出。
//!
//! 每个示例以 `mod common;` 引入；设备地址取自环境变量 `GOPXL_IP`（`IP` 或 `IP:端口`），未设置时先发现
//! （C++ 示例写死 `SYSTEM_IP` 为 `192.168.1.10`）。被远程控制（加速）的传感器要连接控制方，通常是 `127.0.0.1`。
//! 换引擎（例如共焦线的 `LMIConfocalLineProfiler`）时改这里的 ID 与路径，示例的其他路径与数据源都由它们拼出。

#![allow(dead_code, reason = "每个示例只用到其中一部分")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gopxl::control::{Client, Method, Request};
use gopxl::discovery::{self, Instance};
use gopxl::gdp::{self, Body, Cas, Image, Message, PixelFormat, ProfileAttrs};
use gopxl::{RunState, System, Value, control};

/// 示例的结果类型：`main` 返回 `Err` 时打印错误并以失败退出，取代 C++ 示例逐段的 `try` / `catch`。
pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// 引擎 ID（C++ 示例以 `GOCATOR_SENSOR` 选择的激光线轮廓引擎）。
pub const ENGINE_ID: &str = "LMILaserLineProfiler";
/// 扫描器（传感器组）ID。
pub const SCANNER_ID: &str = "scanner-0";
/// 传感器 ID。
pub const SENSOR_ID: &str = "sensor-0";

/// 引擎的资源路径。
pub const ENGINE_PATH: &str = "/scan/engines/LMILaserLineProfiler";
/// 扫描器集合。
pub const SCANNERS_PATH: &str = "/scan/engines/LMILaserLineProfiler/scanners";
/// 示例使用的扫描器。
pub const SCANNER_PATH: &str = "/scan/engines/LMILaserLineProfiler/scanners/scanner-0";
/// 扫描器下的传感器集合。
pub const SENSORS_PATH: &str = "/scan/engines/LMILaserLineProfiler/scanners/scanner-0/sensors";
/// 示例使用的传感器。
pub const SENSOR_PATH: &str =
    "/scan/engines/LMILaserLineProfiler/scanners/scanner-0/sensors/sensor-0";
/// 2D 相机（`2dscanner` 引擎）的传感器；它的设置都在传感器一级。
pub const SENSOR_2D_PATH: &str = "/scan/engines/2dscanner/scanners/scanner-0/sensors/sensor-0";
/// 网卡配置。
pub const IP_CONFIG_PATH: &str = "/environ/ipConfig";

/// 扫描器参数中扫描模式的 JSON Pointer（只有 Gocator 传感器有扫描模式，SmartCam 只有图像模式）。
pub const SCAN_MODE_PATH: &str = "/parameters/scanModeSettings/scanMode";
/// 扫描模式：图像。
pub const SCAN_MODE_IMAGE: i64 = 0;
/// 扫描模式：轮廓。
pub const SCAN_MODE_PROFILE: i64 = 2;
/// 扫描模式：表面。
pub const SCAN_MODE_SURFACE: i64 = 3;
/// 扫描器参数中的触发源。
pub const TRIGGER_SOURCE_PATH: &str = "/parameters/triggerSettings/source";
/// 触发模式：时间。
pub const TRIGGER_MODE_TIME: i64 = 0;
/// 触发模式：软件。
pub const TRIGGER_MODE_SOFTWARE: i64 = 3;

/// 对准状态（扫描器 `alignment` 的 `alignState`）：未对准。
pub const UNALIGNED: i64 = 0;
/// 对准状态：已对准。
pub const ALIGNED: i64 = 1;
/// 对准状态：对准中。
pub const ALIGNING: i64 = 2;

// 传感器参数中的曝光模式、单次曝光、多次曝光与有效区域。
pub const EXPOSURE_MODE_PATH: &str = "/parameters/exposureSettings/exposureMode";
pub const SINGLE_EXPOSURE_PATH: &str = "/parameters/exposureSettings/singleExposure";
pub const MULTI_EXPOSURE_PATH: &str = "/parameters/exposureSettings/multipleExposures";
pub const ACTIVE_AREA_PATH: &str = "/parameters/activeAreaSettings/activeArea";
/// 曝光模式：单次。
pub const EXPOSURE_MODE_SINGLE: i64 = 0;
/// 曝光模式：多次。
pub const EXPOSURE_MODE_MULTI: i64 = 1;

/// 数字输出。
pub const DIGITAL_OUTPUT_PATH: &str = "/controls/digitalOutput";
/// 数字输出端口参数中的触发事件。
pub const DIO_TRIGGER_EVENT_PATH: &str = "/parameters/triggerEvent";
/// 触发事件：软件。
pub const DIO_TRIGGER_SOFTWARE: i64 = 2;

/// 对准、作业加载、恢复等耗时命令的超时；其余请求用默认的 3 s。
pub const EXTENDED_TIMEOUT: Duration = Duration::from_secs(30);
/// 接收数据集与下载文件的超时。
pub const RECEIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// 图像的默认输出位置。
pub const IMAGE_FILE: &str = "ImageOutput.bmp";

/// 无效的轮廓与表面点（`k16S_NULL`）。
pub const NULL: i16 = i16::MIN;

/// 发现的等待时长。
pub const DISCOVER_TIMEOUT: Duration = Duration::from_secs(3);

/// 连接 `GOPXL_IP`，对应 C++ 示例开头的 `SetAddress`、`SetControlPort` 与 `Connect`；未设置时用
/// [`discover_one`] 找到的实例。
pub fn connect() -> Result<System> {
    let addr = match std::env::var("GOPXL_IP") {
        Ok(text) => match text.parse() {
            Ok(addr) => addr,
            Err(_) => SocketAddr::new(text.parse()?, control::DEFAULT_PORT),
        },
        Err(_) => discover_one()?,
    };
    println!("Connecting to {addr}...");
    Ok(System::connect(addr)?)
}

/// 发现唯一一台可连接（未被远程控制）的 GoPxL 实例，返回它的控制通道地址。没有或有多台时返回错误，由
/// `GOPXL_IP` 指定，避免连到别的设备上改配置。
fn discover_one() -> Result<SocketAddr> {
    println!("GOPXL_IP is not set, discovering...");
    let found: Vec<_> = discovery::discover(DISCOVER_TIMEOUT)?
        .iter()
        .filter_map(Instance::control_addr)
        .collect();
    match found.as_slice() {
        [addr] => Ok((*addr).into()),
        [] => Err("no connectable GoPxL instance found; set GOPXL_IP".into()),
        _ => Err(format!("found {found:?}; set GOPXL_IP to one of them").into()),
    }
}

/// 打印 API 版本与设备信息，确认设备没有被远程控制；对应 C++ 的 `VerifyConnection`。被远程控制时返回错误：
/// 这时要连接控制方的地址。
pub fn verify_connection(system: &System) -> Result {
    let client = system.client();
    let version = client.read("/version").response()?.payload;
    println!("API version is {}.", text(&version["apiVersion"]));

    let info = client.read("/environ/info").response()?.payload;
    let (model, serial) = (text(&info["model"]), text(&info["serialNumber"]));
    let application = info["applicationType"].as_i64();
    let device = match application {
        Some(0) => format!("a Gocator Sensor model {model} with serial number {serial}"),
        Some(1) => "a GoPxL on PC".to_owned(),
        Some(2) => format!("a GoMax model {model} with serial number {serial}"),
        Some(3) => "a GoPxL Daemon".to_owned(),
        _ => format!("of application type {}", info["applicationType"]),
    };
    println!("This device is {device}.");
    // 传感器本身之外的应用，序列号属于它连接的传感器。
    if application != Some(0) {
        let sensor = client.read(SENSOR_PATH).response()?.payload;
        println!(
            "The serial number of {SCANNER_ID} {SENSOR_ID} is {}.",
            text(&sensor["serialNumber"])
        );
    }

    let remote = client.read("/environ/remoteController").response()?.payload;
    if remote["remoteConnected"].as_bool() == Some(true) {
        return Err(format!(
            "this device is controlled by a remote controller at IP {} with control port {}; \
             use the IP address of the remote controller",
            text(&remote["ipAddress"]),
            remote["controlPort"]
        )
        .into());
    }
    Ok(())
}

/// 字符串原样输出（C++ 的 `Get<string>()`），其他值输出 JSON 文本。
pub fn text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// 打印小节标题，对应 C++ 的 `PrintSectionHeader`。
pub fn section(title: &str) {
    let rule = "=".repeat(81);
    println!("\n{rule}\n{title}\n{rule}\n");
}

/// 数字输出触发事件的名称。
pub fn trigger_event_name(event: i64) -> Option<&'static str> {
    Some(match event {
        1 => "Measurement",
        2 => "Software",
        3 => "Alignment",
        4 => "Exposure Begin",
        5 => "Exposure End",
        6 => "Part Detection",
        7 => "System State",
        _ => return None,
    })
}

/// 扫描器的数据源 `scan:<引擎>:<扫描器>:<key>`，例如 `topUniformProfile`、`stamp`；共焦引擎的轮廓带 `Layer0`
/// 后缀，`LMIFringeSnapshot` 以传感器 ID 代替 `top`。
pub fn scan_source(key: &str) -> String {
    format!("scan:{ENGINE_ID}:{SCANNER_ID}:{key}")
}

/// 读 `path`，取载荷中 `pointer` 处的值；缺失时报错（C++ 的 `At` 抛异常）。
pub fn read_at(client: &Client, path: &str, pointer: &str) -> Result<Value> {
    let payload = client.read(path).response()?.payload;
    let value = payload
        .pointer(pointer)
        .ok_or_else(|| format!("{path} has no {pointer}"))?;
    Ok(value.clone())
}

/// 读 `/replay/playback`，打印并返回用的是否是回放数据。
pub fn replay_enabled(client: &Client) -> Result<bool> {
    let enabled = read_at(client, "/replay/playback", "/enabled")?
        .as_bool()
        .ok_or("/replay/playback enabled is not a boolean")?;
    println!("Using {} data", if enabled { "replay" } else { "live" });
    Ok(enabled)
}

/// 扫描模式不是 `mode` 时改为 `mode`（`name` 用于打印）。
pub fn set_scan_mode(client: &Client, mode: i64, name: &str) -> Result {
    let current = read_at(client, SCANNER_PATH, SCAN_MODE_PATH)?
        .as_i64()
        .ok_or("the scan mode is not an integer")?;
    if current != mode {
        println!("Setting scan mode to {name}...");
        let mut patch = Value::object();
        patch.set(SCAN_MODE_PATH, mode)?;
        client.update(SCANNER_PATH, patch).response()?;
    }
    Ok(())
}

/// 开启扫描器的强度输出。
pub fn enable_intensity(client: &Client) -> Result {
    println!("Enabling intensity...");
    let intensity = r#"{"parameters":{"scanModeSettings":{"intensityEnabled":true}}}"#;
    client
        .update(SCANNER_PATH, intensity.parse::<Value>()?)
        .response()?;
    Ok(())
}

/// 就绪时启动设备（C++ 示例的 `if (RunningState() == Ready) Start()`）。
pub fn start_if_ready(system: &System) -> Result {
    if system.running_state()? == RunState::Ready {
        println!("Starting system...");
        system.start()?;
    }
    Ok(())
}

/// 开启 GDP 输出（Gocator Protocol）。
pub fn enable_gdp(client: &Client) -> Result {
    println!("Enabling Gocator Protocol...");
    client
        .update("/controls/gocator", r#"{"enabled":true}"#.parse::<Value>()?)
        .response()?;
    Ok(())
}

/// 把数据源 `source` 加到 GDP 输出的 `output_id` 号，与它重叠的输出项后移（`autoShift`），否则重叠时被拒绝。
pub fn add_output(client: &Client, source: &str, output_id: i64) -> Result {
    let mut payload = Value::object();
    payload.set("source", source)?;
    payload.set("outputId", output_id)?;
    payload.set("autoShift", true)?;
    client
        .call("/controls/gocator/outputs/commands/add", payload)
        .response()?;
    Ok(())
}

/// `source` 不在 GDP 输出表中时把它加到 0 号。按 `source` 精确匹配：C++ 把 `source.find(key)` 的返回值当作真值，
/// 输出表非空即跳过添加。
pub fn ensure_output(client: &Client, source: &str) -> Result {
    let outputs = client.read("/controls/gocator/outputs").response()?.payload;
    let added = outputs["map"]
        .as_array()
        .ok_or("/controls/gocator/outputs has no \"map\"")?
        .iter()
        .any(|output| output["source"].as_str() == Some(source));
    if !added {
        add_output(client, source, 0)?;
    }
    Ok(())
}

/// 连接 GDP 服务端（[`System::gdp`]），打印进度；对应 C++ 示例的
/// `gdpClient->Connect(system.Address(), system.GdpPort())`。
pub fn connect_gdp(system: &System) -> Result<gdp::Client> {
    println!("Connecting to Gocator Protocol...");
    Ok(system.gdp()?)
}

/// 用 `call <command>` 下载归档（`contents` 选内容），写到当前目录的 `file`，返回其绝对路径；对应 C++ 示例的
/// archive 一段。响应可能超过 1 GiB，返回前释放。
pub fn download_archive(client: &Client, contents: &Value, file: &str) -> Result<PathBuf> {
    let archive = Request::with_payload(Method::Call, "/system/commands/archive", contents.clone());
    let response = client.transfer(archive, RECEIVE_TIMEOUT)?;
    let data = response.payload["data"]
        .as_bytes()
        .ok_or("archive response has no binary \"data\"")?;
    println!("Downloading {file}...");
    let path = std::path::absolute(file)?;
    std::fs::write(&path, data)?;
    println!("File location: {}", path.display());
    Ok(path)
}

/// 把归档文件 `path` 作为 `data` 连同 `contents` 上传并恢复（`call /system/commands/restore`）；对应 C++ 示例的
/// restore 一段。
pub fn restore_archive(client: &Client, contents: Value, path: &Path) -> Result {
    let mut restore = Request::with_payload(Method::Call, "/system/commands/restore", contents);
    restore.payload.set("data", std::fs::read(path)?)?;
    println!("Restoring from {}...", path.display());
    client.transfer(restore, EXTENDED_TIMEOUT)?;
    Ok(())
}

/// 打印标题与各资源路径（C++ 遍历 `ChildUris` 的循环）。
pub fn print_paths(title: &str, paths: &[String]) {
    println!("\n{title}:");
    for path in paths {
        println!("  {path}");
    }
}

/// 打印消息类型与公共属性（C++ 各消息分支开头的几行）；数组输出的每一层是一条消息，以 `array_index` 区分。
pub fn print_cas(kind: &str, cas: &Cas) {
    println!("Message type: {kind}");
    if let Some(id) = cas.gdp_id {
        println!("GDP ID: {id}");
    }
    println!("Data source ID: {}", cas.data_source_id);
    if cas.array_count == 0 {
        println!("Not arrayed");
    } else {
        println!("Arrayed");
        println!("\tCount: {}", cas.array_count);
        println!("\tIndex: {}", cas.array_index);
    }
}

/// 轮廓的原始值换算为毫米：`偏移 + 分辨率 × 原始值`（SDK 不提供换算，示例手算）。
pub fn profile_mm(attrs: &ProfileAttrs, [x, z]: [f64; 2]) -> [f64; 2] {
    [
        attrs.x_offset + attrs.x_scale * x,
        attrs.z_offset + attrs.z_scale * z,
    ]
}

/// 消息是轮廓时把它换算为毫米坐标并打印点数，返回是否为轮廓；对应 C++ 循环里轮廓的两个分支。
pub fn print_profile(message: &Message) -> bool {
    let (kind, total, intensity, points): (_, _, _, Vec<[f64; 2]>) = match &message.body {
        Body::UniformProfile(profile) => {
            // 统一轮廓只发送 z，x 由点的下标换算。
            let points = (0u32..)
                .zip(&profile.ranges)
                .filter(|&(_, &z)| z != NULL)
                .map(|(x, &z)| profile_mm(&profile.attrs, [x.into(), z.into()]))
                .collect();
            (
                "Uniform Profile",
                profile.ranges.len(),
                &profile.intensity,
                points,
            )
        }
        Body::ProfilePointCloud(profile) => {
            let points = profile
                .points
                .iter()
                .filter(|&&[x, _]| x != NULL)
                .map(|point| profile_mm(&profile.attrs, point.map(f64::from)))
                .collect();
            (
                "Point Cloud Profile",
                profile.points.len(),
                &profile.intensity,
                points,
            )
        }
        _ => return false,
    };
    print_cas(kind, &message.cas);
    println!("Profile points count: {total}");
    println!("Intensity count: {}", intensity.len());
    println!("Valid points count: {}", points.len());
    true
}

/// 反复接收，直到数据集里有图像或超过 10 s；返回最后一个数据集（C++ 的 `DataSet()`）。
pub fn wait_for_image(gdp: &mut gdp::Client) -> Result<Vec<Message>> {
    println!("Waiting until a dataset with Image data is received within 10 seconds.");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let data_set = gdp.receive(RECEIVE_TIMEOUT)?;
        print!(".");
        if data_set
            .iter()
            .any(|message| matches!(message.body, Body::Image(_)))
        {
            println!("\nA dataset with an Image message has been received.");
            return Ok(data_set);
        }
        if Instant::now() >= deadline {
            return Ok(data_set);
        }
    }
}

/// 打印数据集中的时间戳与图像；给出 `bmp` 时把图像写成 BMP。
pub fn print_image_set(data_set: &[Message], bmp: Option<&str>) -> Result {
    // 超时退出时前面是没有换行的进度点。
    println!(
        "\nTotal number of messages in this data set: {}",
        data_set.len()
    );
    for (i, message) in data_set.iter().enumerate() {
        let cas = &message.cas;
        println!("Message {} of {}", i + 1, data_set.len());
        println!("Message Type {}: {}", message.ty, cas.data_source_id);
        match &message.body {
            Body::Stamp(stamp) => {
                println!("Stamp");
                println!("Frame index: {}", stamp.frame_index);
                println!("Timestamp: {}", stamp.timestamp);
                println!("Encoder: {}", stamp.encoder);
                println!("Encoder at Z: {}", stamp.encoder_at_z);
                println!("Status: {}", stamp.status);
                println!("System Time Sec: {}", stamp.system_time_sec);
                println!("System Time Nanosec: {}", stamp.system_time_nsec);
            }
            Body::Image(image) => {
                println!("Image");
                println!("Data Source: {}", cas.data_source_id);
                println!("Array count: {}", cas.array_count);
                println!("Array index: {}", cas.array_index);
                if let Some(path) = bmp {
                    write_bmp(path, image)?;
                }
            }
            _ => println!("Unsupported message received."),
        }
    }
    Ok(())
}

/// 把图像写成 24 位 BMP，对应 C++ 示例的 `WriteOutputToBmpFile`。支持 8 位灰度、Mono8、Mono12、Mono12p、Mono16、
/// Rgb8、Bgr8；12 / 16 位缩放到 8 位，灰度写成三个相同的通道。
///
/// 按 [`Image::row_size`] 逐行读取、每行补齐到 4 字节写出（C++ 把像素当作连续数组、不补齐，宽度不是 4 的倍数时
/// 图像错位）。BMP 自下而上存储，行按相反顺序写出，图像不会上下颠倒。
pub fn write_bmp(path: &str, image: &Image) -> Result {
    type Pixel = fn(&[u8], usize) -> Option<[u8; 3]>;
    let pixel: Pixel = match image.pixel_format {
        PixelFormat::GREYSCALE_8BPP | PixelFormat::MONO8 => |row, x| Some([*row.get(x)?; 3]),
        PixelFormat::MONO12 => |row, x| Some(grey(le16(row, x)?, 4095)),
        PixelFormat::MONO16 => |row, x| Some(grey(le16(row, x)?, 65535)),
        // 每两个像素 3 字节：像素 0 = B 的低 4 位 << 8 | A，像素 1 = C << 4 | B 的高 4 位。
        PixelFormat::MONO12P => |row, x| {
            let [a, b, c] = [0, 1, 2].map(|i| row.get(x / 2 * 3 + i).copied());
            let value = if x % 2 == 0 {
                u16::from(b? & 0xF) << 8 | u16::from(a?)
            } else {
                u16::from(c?) << 4 | u16::from(b? >> 4)
            };
            Some(grey(value, 4095))
        },
        PixelFormat::RGB8 => |row, x| {
            let [r, g, b] = row.get(x * 3..x * 3 + 3)?.try_into().ok()?;
            Some([b, g, r])
        },
        PixelFormat::BGR8 => |row, x| row.get(x * 3..x * 3 + 3)?.try_into().ok(),
        other => return Err(format!("pixel format {other:?} cannot be written as BMP").into()),
    };
    let (width, height) = (image.width as usize, image.height as usize);
    let stride = (width * 3).next_multiple_of(4);
    let len = 54 + stride * height;
    let mut bmp = Vec::with_capacity(len);
    // 文件头（14 字节）与 BITMAPINFOHEADER（40 字节），分辨率 512 像素 / 米与 C++ 相同。
    bmp.extend(b"BM");
    for field in [u32::try_from(len)?, 0, 54, 40, image.width, image.height] {
        bmp.extend(field.to_le_bytes());
    }
    bmp.extend(1u16.to_le_bytes());
    bmp.extend(24u16.to_le_bytes());
    for field in [0u32, 0, 512, 512, 0, 0] {
        bmp.extend(field.to_le_bytes());
    }
    for row in image
        .pixels
        .chunks(image.row_size().max(1))
        .take(height)
        .rev()
    {
        let start = bmp.len();
        for x in 0..width {
            bmp.extend(pixel(row, x).ok_or("image row is shorter than its width")?);
        }
        bmp.resize(start + stride, 0);
    }
    std::fs::write(path, bmp)?;
    Ok(())
}

/// 第 `x` 个小端 16 位像素。
fn le16(row: &[u8], x: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        row.get(x * 2..x * 2 + 2)?.try_into().ok()?,
    ))
}

/// 把 `0..=max` 的值缩放到 8 位灰度（C++ 的 `value * 255 / max`）。
fn grey(value: u16, max: u32) -> [u8; 3] {
    [u8::try_from(u32::from(value) * 255 / max).unwrap_or(u8::MAX); 3]
}
