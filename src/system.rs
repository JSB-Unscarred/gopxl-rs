//! 一台 GoPxL 设备的门面 [`System`]，对应 C++ 的 `GoSystem`：拥有主连接，把 `GoSystem` 的几个便捷操作包装成方法。
//!
//! 只包装 `GoSystem` 已有的操作；作业、回放、工具、GDP 输出等其余资源经 [`System::client`] 的原始动词访问（与
//! 官方示例相同），或经 [`System::resource`] 的缓存资源访问（与 `ResourceApi` 示例相同）。资源由服务端在运行时
//! 定义，各版本的字段也会变化，类型化模型只会落后于服务端。

use std::net::SocketAddr;
use std::time::Duration;

use crate::control::{Client, Connection};
use crate::error::Result;
use crate::gdp;
use crate::resource::{Resource, rel};
use crate::value::Value;

/// `GoSystem` 各操作的超时（C++ 的 `*_TIMEOUT_MSEC` 都是 15 s）。
const TIMEOUT: Duration = Duration::from_secs(15);

/// 一台 GoPxL 设备，对应 `GoSystem`。构造即连接；连接断开后 `System` 仍在，`client().is_connected()` 为假，
/// 之后的请求以 `Disconnected` 失败。
///
/// 唯一拥有主连接（[`Connection`]），Drop 即断开（C++ 的 `Disconnect`）。地址、连接状态与原始动词经
/// [`client`](Self::client) 访问。
#[derive(Debug)]
pub struct System {
    connection: Connection,
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
    /// 连接 `addr`（超时 1 s，见 [`Connection::connect`]），对应 C++ 的 `GoSystem(address, port)` 加 `Connect`。
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        Ok(Self {
            connection: Connection::connect(addr).await?,
        })
    }

    /// 主连接的句柄，对应 C++ 的 `Client`。大多数操作直接经它发出。
    pub fn client(&self) -> &Client {
        self.connection.client()
    }

    /// 路径 `path` 上带缓存的资源（见 [`Resource`]），开启写入前的校验，对应 C++ 的 `Resource`（`GoSystem` 的资源
    /// 管理器默认开启校验）。每次调用得到一个独立的 `Resource`，各有各的缓存。
    pub fn resource(&self, path: impl Into<String>) -> Resource {
        let mut resource = Resource::new(self.client().clone(), path);
        resource.set_validation(true);
        resource
    }

    /// 连接设备的 GDP 服务端（地址同主连接，端口见 [`gdp_port`](Self::gdp_port)，超时 5 s），对应官方示例
    /// `SampleUtils` 里连接 `GoGdpClient` 的写法。
    pub async fn gdp(&self) -> Result<gdp::Client> {
        let addr = SocketAddr::new(self.client().addr().ip(), self.gdp_port().await?);
        gdp::Client::connect(addr).await
    }

    /// 启动设备：`call /system/commands/start`。
    pub async fn start(&self) -> Result<()> {
        self.command("start").await
    }

    /// 停止设备：`call /system/commands/stop`。
    pub async fn stop(&self) -> Result<()> {
        self.command("stop").await
    }

    /// 运行状态：`read /system` 的 `runState`，对应 C++ 的 `RunningState`。
    pub async fn running_state(&self) -> Result<RunState> {
        Ok(match self.res("/system").get::<i64>("/runState").await? {
            0 => RunState::Ready,
            1 => RunState::Running,
            2 => RunState::Conflict,
            other => RunState::Other(other),
        })
    }

    /// 开启或关闭快速编辑：`update /system {"quickEditEnabled": enabled}`，对应 C++ 的 `EnableQuickEdit` /
    /// `DisableQuickEdit`。快速编辑下修改配置不会对已有的扫描数据重新计算。
    pub async fn set_quick_edit(&self, enabled: bool) -> Result<()> {
        self.res("/system").set("/quickEditEnabled", enabled).await
    }

    /// 快速编辑是否开启：`read /system` 的 `quickEditEnabled`，对应 C++ 的 `QuickEditEnabled`。
    pub async fn quick_edit_enabled(&self) -> Result<bool> {
        self.res("/system").get("/quickEditEnabled").await
    }

    /// GDP 端口：`read /controls/gocator` 的 `serverPort`，对应 C++ 的 `GdpPort`。不在 `u16` 内时报 `Payload`
    /// （C++ 截断为 `k16u`）。
    pub async fn gdp_port(&self) -> Result<u16> {
        self.res("/controls/gocator").get("/serverPort").await
    }

    /// 序列号为 `serial` 的传感器的资源路径，找不到时为 `None`，对应 C++ 的 `SensorPath`（找不到时返回 `""`）。
    ///
    /// 与 C++ 一样逐层读取 `/scan/engines`、各引擎的 `scanners`、各扫描器的 `sensors` 与每个传感器，比较字符串
    /// `serialNumber`；共 O(传感器数) 次往返。
    pub async fn sensor_path(&self, serial: &str) -> Result<Option<String>> {
        for engine in self.res("/scan/engines").child_paths(rel::ITEM).await? {
            for scanner in self
                .res(&format!("{engine}/scanners"))
                .child_paths(rel::ITEM)
                .await?
            {
                for sensor in self
                    .res(&format!("{scanner}/sensors"))
                    .child_paths(rel::ITEM)
                    .await?
                {
                    if self.res(&sensor).get::<&str>("/serialNumber").await? == serial {
                        return Ok(Some(sensor));
                    }
                }
            }
        }
        Ok(None)
    }

    /// `call /system/commands/<name>`。
    async fn command(&self, name: &str) -> Result<()> {
        self.res("/system")
            .command(name, Value::object())
            .response()
            .await?;
        Ok(())
    }

    /// `path` 上的资源：不校验、超时 15 s，`GoSystem` 的操作都经它发请求，读不到的字段报 `Payload`。
    fn res(&self, path: &str) -> Resource {
        let mut resource = Resource::new(self.client().clone(), path);
        resource.set_timeout(TIMEOUT);
        resource
    }
}
