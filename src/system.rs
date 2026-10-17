//! 一台 GoPxL 设备的门面 [`System`]，对应 C++ 的 `GoSystem`：拥有主连接，把 `GoSystem` 的几个便捷操作包装成方法。
//!
//! 只包装 `GoSystem` 已有的操作；作业、回放、工具、GDP 输出等其余资源经 [`System::client`] 的原始动词访问（与
//! 官方示例相同），或经 [`System::resource`] 的缓存资源访问（与 `ResourceApi` 示例相同）。资源由服务端在运行时
//! 定义，各版本的字段也会变化，类型化模型只会落后于服务端。

use std::net::SocketAddr;
use std::time::Duration;

use crate::control::Client;
use crate::error::{Error, Result};
use crate::resource::Resource;
use crate::value::{Map, Value};

/// `GoSystem` 各操作的超时（C++ 的 `*_TIMEOUT_MSEC` 都是 15 s）。
const TIMEOUT: Duration = Duration::from_secs(15);

/// 一台 GoPxL 设备，对应 C++ 的 `GoSystem`。存在即已连接，没有“未连接”的状态。
///
/// 唯一拥有主连接（[`Client`]），Drop 即断开（C++ 的 `Disconnect`）。地址、连接状态、断开回调与原始动词经
/// [`client`](Self::client) 访问。
#[derive(Debug)]
pub struct System {
    client: Client,
}

/// 运行状态（`/system` 的 `runState`），对应 C++ 的 `GoSystem::State`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// 0：就绪，可以启动。
    Ready,
    /// 1：运行中。
    Running,
    /// 2：冲突，例如传感器组的配置与实际连接的传感器不一致。
    Conflict,
    /// 表外的值原样保留（C++ 直接强制转换为枚举）。
    Other(i64),
}

impl System {
    /// 连接 `addr`（超时 1 s，见 [`Client::connect`]），对应 C++ 的 `GoSystem(address, port)` 加 `Connect`。
    pub fn connect(addr: SocketAddr) -> Result<Self> {
        Ok(Self {
            client: Client::connect(addr)?,
        })
    }

    /// 主连接，对应 C++ 的 `Client`。大多数操作直接经它发出。
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// 路径 `path` 上带缓存的资源（见 [`Resource`]），开启写入前的校验，对应 C++ 的 `Resource`（`GoSystem` 的资源
    /// 管理器默认开启校验）。每次调用得到一个独立的 `Resource`，各有各的缓存。
    pub fn resource(&self, path: impl Into<String>) -> Resource {
        let mut resource = Resource::new(self.client.handle(), path);
        resource.set_validation(true);
        resource
    }

    /// 启动设备：`call /system/commands/start`。
    pub fn start(&self) -> Result<()> {
        self.call("/system/commands/start")
    }

    /// 停止设备：`call /system/commands/stop`。
    pub fn stop(&self) -> Result<()> {
        self.call("/system/commands/stop")
    }

    /// 运行状态：`read /system` 的 `runState`，对应 C++ 的 `RunningState`。
    pub fn running_state(&self) -> Result<RunState> {
        let state = self.read_field("/system", "runState", Value::as_i64)?;
        Ok(match state {
            0 => RunState::Ready,
            1 => RunState::Running,
            2 => RunState::Conflict,
            other => RunState::Other(other),
        })
    }

    /// 开启或关闭快速编辑：`update /system {"quickEditEnabled": enabled}`，对应 C++ 的 `EnableQuickEdit` /
    /// `DisableQuickEdit`。快速编辑下修改配置不会对已有的扫描数据重新计算。
    pub fn set_quick_edit(&self, enabled: bool) -> Result<()> {
        let patch = Map::from([("quickEditEnabled".to_owned(), Value::Bool(enabled))]);
        self.client
            .update("/system", patch)
            .timeout(TIMEOUT)
            .response()?;
        Ok(())
    }

    /// 快速编辑是否开启：`read /system` 的 `quickEditEnabled`，对应 C++ 的 `QuickEditEnabled`。
    pub fn quick_edit_enabled(&self) -> Result<bool> {
        self.read_field("/system", "quickEditEnabled", Value::as_bool)
    }

    /// GDP 端口：`read /controls/gocator` 的 `serverPort`，对应 C++ 的 `GdpPort`。不在 `u16` 内时报 `Payload`
    /// （C++ 截断为 `k16u`）。
    pub fn gdp_port(&self) -> Result<u16> {
        self.read_field("/controls/gocator", "serverPort", |port| {
            port.as_i64().and_then(|port| u16::try_from(port).ok())
        })
    }

    /// 序列号为 `serial` 的传感器的资源路径，找不到时为 `None`，对应 C++ 的 `SensorPath`（找不到时返回 `""`）。
    ///
    /// 与 C++ 一样逐层读取 `/scan/engines`、各引擎的 `scanners`、各扫描器的 `sensors` 与每个传感器，比较字符串
    /// `serialNumber`；共 O(传感器数) 次往返。
    pub fn sensor_path(&self, serial: &str) -> Result<Option<String>> {
        for engine in self.children("/scan/engines")? {
            for scanner in self.children(&format!("{engine}/scanners"))? {
                for sensor in self.children(&format!("{scanner}/sensors"))? {
                    let response = self.client.read(&sensor).timeout(TIMEOUT).response()?;
                    if response.payload["serialNumber"].as_str() == Some(serial) {
                        return Ok(Some(sensor));
                    }
                }
            }
        }
        Ok(None)
    }

    fn call(&self, path: &str) -> Result<()> {
        self.client
            .call(path, Value::object())
            .timeout(TIMEOUT)
            .response()?;
        Ok(())
    }

    /// 读 `path`，用 `get` 取载荷中的 `field`；缺失或类型不对时报 `Payload`（C++ 抛 `GoJsonError`）。
    fn read_field<T>(
        &self,
        path: &'static str,
        field: &'static str,
        get: impl FnOnce(&Value) -> Option<T>,
    ) -> Result<T> {
        let response = self.client.read(path).timeout(TIMEOUT).response()?;
        get(&response.payload[field]).ok_or_else(|| Error::Payload {
            path: path.to_owned(),
            field: field.to_owned(),
        })
    }

    /// 集合 `path` 的子资源路径：`_embedded.item` 各条目的 `_links.self.href`。服务端强制 `item` 为数组；不是数组
    /// 或条目缺少 href 时与 C++ 一样当作没有子项。
    fn children(&self, path: &str) -> Result<Vec<String>> {
        let response = self.client.read(path).timeout(TIMEOUT).response()?;
        let items = response.payload["_embedded"]["item"]
            .as_array()
            .unwrap_or_default();
        let hrefs = items
            .iter()
            .filter_map(|item| item.pointer("/_links/self/href")?.as_str())
            .map(str::to_owned)
            .collect();
        Ok(hrefs)
    }
}
