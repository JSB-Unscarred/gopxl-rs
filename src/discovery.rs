//! UDP 发现协议的报文编解码：GoPxL 发现（本模块，UDP 3320）与 Gocator 6.x 经典发现（[`classic`]，UDP 3220）。
//!
//! 客户端向 `255.255.255.255:3320` 广播 Discover（[`encode_discover`]），每个服务端回复一条 Announce。Announce
//! 也是受限广播，所以接收 socket 同样会收到其他客户端（以及自己）发出的 Discover。整数都是小端：
//!
//! | 偏移 | Discover（24 字节） | Announce |
//! |---|---|---|
//! | 0 | `u64` Length = 24 | `u64` Length = 数据报长度（含 JSON 末尾的 NUL） |
//! | 8 | `u64` `MsgId` = 1 | `u64` `MsgId` = `0x1001` |
//! | 16 | `u64` 签名 `"LMIGOPXL"` | `u64` 签名 |
//! | 24 | — | `i64` 状态：1，或 −992 表示服务端截断了某个字段 |
//! | 32 | — | 紧凑 JSON 对象，以 NUL 结尾，最长 1504 字节 |
//!
//! 数据报来自网络，[`parse_announce`] 遵守以下约定：
//! - 只解析收到的字节：JSON 取到第一个 NUL，缺少结尾 NUL 时取到数据报末尾；数据报边界由 UDP
//!   给出，Length 字段不校验；
//! - 读取都经过 `bytes::Buf` 的 `try_get_*`，下面的 lint 禁止下标与 unwrap，坏报文只得到 `Err`；
//! - 单个字段缺失或类型不对时取默认值，整条 Announce 仍然可用。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

pub mod classic;
mod socket;

pub use socket::{discover, discover_with_classic};

use std::net::{Ipv4Addr, SocketAddrV4};

use bytes::{Buf, TryGetError};

use crate::value::Value;

/// GoPxL 发现的 UDP 端口：Discover 与 Announce 都广播到这个端口。
pub const PORT: u16 = 3320;

/// 签名 `"LMIGOPXL"`。
const SIGNATURE: u64 = 0x4C58_504F_4749_4D4C;
/// 客户端广播的 Discover 命令的 `MsgId`。
const DISCOVER: u64 = 0x0001;
/// 客户端设置服务端地址的 `SetIp` 命令的 `MsgId`；本 crate 不发送，收到时识别为命令。
const SET_IP: u64 = 0x0003;
/// 服务端回复的 Announce 的 `MsgId`。
const ANNOUNCE: u64 = 0x1001;

/// 发现到的一个实例。
///
/// GoPxL 实例来自 Announce 的 JSON（[`parse_announce`]），经典实例来自 `GetIP` / `GetInfo` 回复
/// （[`classic::handle_reply`]）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "五个互相独立的标志：dhcp、is_remote、address_conflict 来自 Announce，classic、truncated 标记来源与截断"
)]
pub struct Instance {
    /// 经 Gocator 6.x 经典协议发现的设备，不是 GoPxL 服务端。
    pub classic: bool,
    /// IPv4 地址；`""` 或无法解析时为 `None`。
    pub address: Option<Ipv4Addr>,
    /// 子网掩码（PC 上的服务端发送 `""`）。
    pub mask: Option<Ipv4Addr>,
    /// 网关（PC 上的服务端发送 `""`）。
    pub gateway: Option<Ipv4Addr>,
    /// 应用 ID；被远程控制时是控制方的 ID。经典实例为 `"classic-<序列号>"`。
    pub app_id: String,
    /// 应用名；被远程控制时为空。经典实例为 `"Gocator Classic"`。
    pub app_name: String,
    /// 应用版本，例如 `"1.5.0.0"`。
    pub app_version: String,
    /// 设备型号。
    pub device_model: String,
    /// 序列号。JSON 里是十进制字符串，不是合法的 `u32` 时为 `None`。
    pub serial_number: Option<u32>,
    /// 控制通道端口；被远程控制时为 0。
    pub control_port: u16,
    /// GDP 端口；被远程控制或 `AutoServicePorts` 模式下为 0。经典实例是它的 `DataPort`。
    pub gdp_port: u16,
    /// Web 端口（传感器 80，PC 8100）。
    pub web_port: u16,
    /// 是否使用 DHCP。
    pub dhcp: bool,
    /// 是否正被远程控制；此时端口都为 0。
    pub is_remote: bool,
    /// 地址冲突（设备的地址是 link-local）。
    pub address_conflict: bool,
    /// HMI 状态：0 RUNNING、1 STOPPED、2 STARTING、3 STOPPING、4 `FAILED_TO_START`、5 `FAILED_TO_STOP`
    /// （取值表未经手册确认）。
    pub hmi_status: Option<i32>,
    /// Announce 的状态 ≠ 1（−992）：服务端按字节截断了某个超长字段。
    pub truncated: bool,
}

impl Instance {
    /// 控制通道地址；能作为 GoPxL 服务端连接（GoPxL 实例、未被远程控制、地址与控制端口有效）时为 `Some`。
    pub fn control_addr(&self) -> Option<SocketAddrV4> {
        if self.classic || self.is_remote || self.control_port == 0 {
            return None;
        }
        Some(SocketAddrV4::new(self.address?, self.control_port))
    }
}

/// 发现报文的解析错误。每种错误都只意味着丢弃这条数据报、继续接收。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// 数据报短于固定布局，签名、`MsgId` 或状态不符，或 JSON 不是 UTF-8、顶层不是对象。
    #[error("malformed discovery datagram")]
    Malformed,
    /// 客户端发出的命令（Discover / `SetIp`，经典协议的 `GetIP` / `GetInfo`），包括自己广播的回环。
    #[error("discovery command sent by a client")]
    Command,
    /// Announce 的 JSON 不合法；状态为 −992 时是服务端把 JSON 截断在 1504 字节。
    #[error("invalid announce JSON")]
    Json(#[from] serde_json::Error),
}

/// 越界读取意味着数据报短于它的固定布局。
impl From<TryGetError> for DiscoveryError {
    fn from(_: TryGetError) -> Self {
        Self::Malformed
    }
}

/// Discover 报文。
pub fn encode_discover() -> [u8; 24] {
    let mut out = [0; 24];
    put_u64s(&mut out, &[24, DISCOVER, SIGNATURE]);
    out
}

/// 依次写入小端 `u64`：两种协议的命令都只由 8 字节字段组成。
fn put_u64s(out: &mut [u8], fields: &[u64]) {
    for (chunk, field) in out.chunks_exact_mut(8).zip(fields) {
        chunk.copy_from_slice(&field.to_le_bytes());
    }
}

/// 解析 Announce，约定见模块文档。状态 ≠ 1 时置 `truncated`；字段缺失
/// 或类型不对时取默认值（`None` / 0 / false / `""`）；Discover 与 `SetIp`（包括自己发出的）返回
/// [`DiscoveryError::Command`]。
pub fn parse_announce(datagram: &[u8]) -> Result<Instance, DiscoveryError> {
    let mut buf = datagram;
    let _length = buf.try_get_u64_le()?;
    let msg_id = buf.try_get_u64_le()?;
    if buf.try_get_u64_le()? != SIGNATURE {
        return Err(DiscoveryError::Malformed);
    }
    match msg_id {
        ANNOUNCE => {}
        DISCOVER | SET_IP => return Err(DiscoveryError::Command),
        _ => return Err(DiscoveryError::Malformed),
    }
    let status = buf.try_get_i64_le()?;
    let json = buf.split(|&b| b == 0).next().unwrap_or_default();
    let json: Value = std::str::from_utf8(json)
        .map_err(|_| DiscoveryError::Malformed)?
        .parse()?;
    if json.as_map().is_none() {
        return Err(DiscoveryError::Malformed);
    }
    Ok(Instance {
        classic: false,
        address: ip(&json["Address"]),
        mask: ip(&json["Mask"]),
        gateway: ip(&json["Gateway"]),
        app_id: string(&json["AppId"]),
        app_name: string(&json["AppName"]),
        app_version: string(&json["AppVersion"]),
        device_model: string(&json["DeviceModel"]),
        serial_number: json["SerialNumber"].as_str().and_then(|s| s.parse().ok()),
        control_port: port(&json["ControlPort"]),
        gdp_port: port(&json["GdpPort"]),
        web_port: port(&json["WebPort"]),
        dhcp: json["Dhcp"].as_bool().unwrap_or_default(),
        is_remote: json["IsRemote"].as_bool().unwrap_or_default(),
        address_conflict: json["AddressConflict"].as_bool().unwrap_or_default(),
        hmi_status: json["HMIStatus"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok()),
        truncated: status != 1,
    })
}

/// 点分十进制的 IPv4 地址；`""`、非字符串或无法解析时为 `None`。
fn ip(value: &Value) -> Option<Ipv4Addr> {
    value.as_str()?.parse().ok()
}

/// 字符串字段，非字符串为 `""`。
fn string(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// 端口：`u16` 范围内的非负整数，否则为 0。
fn port(value: &Value) -> u16 {
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or_default()
}
