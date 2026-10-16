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
//! 数据报来自网络，[`parse_announce`] 用以下约定修掉 C++ SDK 的缺陷（重写方案 §5.4 第 28 条）：
//! - 只解析收到的字节：先校验长度与 Length 字段，JSON 取到第一个 NUL 或数据报末尾，1537 字节的 Announce 在
//!   1536 字节的缓冲区里丢掉 NUL 后，C++ 会越界读取；
//! - 读取都经过 `bytes::Buf` 的 `try_get_*`，下面的 lint 禁止下标与 unwrap，坏报文只得到 `Err`；
//! - 字段缺失或类型不对时取默认值：C++ 在类型不对时抛异常，整条 Announce 被丢弃。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

pub mod classic;

use std::net::{Ipv4Addr, SocketAddrV4};

use bytes::{Buf, TryGetError};

use crate::value::Value;

/// GoPxL 发现的 UDP 端口：Discover 与 Announce 都广播到这个端口。
pub const PORT: u16 = 3320;

/// 签名 `"LMIGOPXL"`。
const SIGNATURE: u64 = 0x4C58_504F_4749_4D4C;
const DISCOVER: u64 = 0x0001;
const SET_IP: u64 = 0x0003;
const ANNOUNCE: u64 = 0x1001;

/// 发现到的一个实例，对应 C++ 的 `GoInstance`。
///
/// GoPxL 实例来自 Announce 的 JSON（[`parse_announce`]），经典实例来自 `GetIP` / `GetInfo` 回复
/// （[`classic::handle_reply`]）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "五个互相独立的标志：dhcp、is_remote、address_conflict 对应 GoInstance 的 getter，classic、truncated 由设计规定"
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
    /// （这张表只出现在 SDK 示例中）。
    pub hmi_status: Option<i32>,
    /// Announce 的状态 ≠ 1（−992）：服务端按字节截断了某个超长字段。
    pub truncated: bool,
}

impl Instance {
    /// 能否作为 GoPxL 服务端连接：GoPxL 实例、未被远程控制、地址与控制端口有效（SDK 示例 `Discover.cpp` 的判断）。
    pub fn is_connectable(&self) -> bool {
        !self.classic && !self.is_remote && self.address.is_some() && self.control_port != 0
    }

    /// 控制通道地址，[`is_connectable`](Self::is_connectable) 时为 `Some`。
    pub fn control_addr(&self) -> Option<SocketAddrV4> {
        self.socket_addr(self.control_port)
    }

    /// GDP 通道地址，可连接且 `gdp_port != 0` 时为 `Some`。
    pub fn gdp_addr(&self) -> Option<SocketAddrV4> {
        self.socket_addr(self.gdp_port)
    }

    fn socket_addr(&self, port: u16) -> Option<SocketAddrV4> {
        if port == 0 || !self.is_connectable() {
            return None;
        }
        Some(SocketAddrV4::new(self.address?, port))
    }
}

/// 发现报文的解析错误。每种错误都只意味着丢弃这条数据报、继续接收。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// 长度、Length 字段、签名、`MsgId` 或状态不符，或 JSON 不是 UTF-8、顶层不是对象。
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

/// Discover 报文，对应 `GoPxLDiscoveryProto::Broadcast` 发出的 `DiscoveryBroadcastHeader`。
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

/// 解析 Announce，对应 `GoPxLDiscoveryProto::ParseReply`，约定见模块文档。状态 ≠ 1 时置 `truncated`；字段缺失
/// 或类型不对时取默认值（`None` / 0 / false / `""`）；Discover 与 `SetIp`（包括自己发出的）返回
/// [`DiscoveryError::Command`]。
pub fn parse_announce(datagram: &[u8]) -> Result<Instance, DiscoveryError> {
    let mut buf = datagram;
    let length = buf.try_get_u64_le()?;
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
    if usize::try_from(length) != Ok(datagram.len()) {
        return Err(DiscoveryError::Malformed);
    }
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

/// 端口：`u16` 范围内的非负整数，否则为 0（C++ 截断为 `k16u`）。
fn port(value: &Value) -> u16 {
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::{DiscoveryError, Instance, encode_discover, parse_announce};
    use crate::hex;

    /// G14 的 JSON（derived）：1.5 服务端总是发送的 15 个键，按字节序排列，290 字节。
    const G14_JSON: &str = r#"{"Address":"192.168.1.10","AddressConflict":false,"AppId":"A1B2C3","AppName":"GoPxL","AppVersion":"1.5.0.0","ControlPort":3600,"DeviceModel":"2520","Dhcp":false,"Gateway":"192.168.1.1","GdpPort":3601,"HMIStatus":0,"IsRemote":false,"Mask":"255.255.255.0","SerialNumber":"12345","WebPort":80}"#;

    fn g14() -> Instance {
        Instance {
            classic: false,
            address: Some(Ipv4Addr::new(192, 168, 1, 10)),
            mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            app_id: "A1B2C3".into(),
            app_name: "GoPxL".into(),
            app_version: "1.5.0.0".into(),
            device_model: "2520".into(),
            serial_number: Some(12_345),
            control_port: 3600,
            gdp_port: 3601,
            web_port: 80,
            dhcp: false,
            is_remote: false,
            address_conflict: false,
            hmi_status: Some(0),
            truncated: false,
        }
    }

    /// 按协议拼出一条 Announce：头部 + JSON + NUL，Length 等于总长。
    fn announce(status: i64, json: &[u8]) -> Vec<u8> {
        let mut out = vec![0; 24];
        let len = 32 + json.len() as u64 + 1;
        super::put_u64s(&mut out, &[len, super::ANNOUNCE, super::SIGNATURE]);
        out.extend_from_slice(&status.to_le_bytes());
        out.extend_from_slice(json);
        out.push(0);
        out
    }

    // G7（derived）：Discover 与 C++ 逐字节一致；收到 Discover 或 SetIp（G18 的头部）报 Command，自己的回环同样如此。
    #[test]
    fn discover_matches_g7_and_commands_are_ignored() {
        let g7 = hex("18 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 4C 4D 49 47 4F 50 58 4C");
        assert_eq!(encode_discover().as_slice(), g7);

        let set_ip = hex("7F 00 00 00 00 00 00 00 03 00 00 00 00 00 00 00 4C 4D 49 47 4F 50 58 4C");
        for datagram in [g7, set_ip] {
            assert!(matches!(
                parse_announce(&datagram),
                Err(DiscoveryError::Command)
            ));
        }
    }

    // G14a/b/d（derived）：JSON 末尾的 NUL 可有可无；状态 −992 而 JSON 合法时照常解码并置 truncated。
    #[test]
    fn announce_decodes_g14_vectors() {
        let long_name = G14_JSON.replace("\"GoPxL\"", &format!("\"{}\"", "N".repeat(64)));
        let cases = [
            (
                "43 01 00 00 00 00 00 00 01 10 00 00 00 00 00 00 4C 4D 49 47 4F 50 58 4C 01 00 00 00 00 00 00 00",
                G14_JSON,
                "00",
                g14(),
            ),
            (
                "42 01 00 00 00 00 00 00 01 10 00 00 00 00 00 00 4C 4D 49 47 4F 50 58 4C 01 00 00 00 00 00 00 00",
                G14_JSON,
                "",
                g14(),
            ),
            (
                "7E 01 00 00 00 00 00 00 01 10 00 00 00 00 00 00 4C 4D 49 47 4F 50 58 4C 20 FC FF FF FF FF FF FF",
                &long_name,
                "00",
                Instance {
                    app_name: "N".repeat(64),
                    truncated: true,
                    ..g14()
                },
            ),
        ];
        for (header, json, tail, expected) in cases {
            let mut datagram = hex(header);
            datagram.extend_from_slice(json.as_bytes());
            datagram.extend(hex(tail));
            assert_eq!(parse_announce(&datagram).unwrap(), expected, "{header}");
        }
    }

    // G14c（derived）：1537 字节、JSON 被截断在 1504 字节的 Announce 报 Json 错误，JSON 只读到数据报内的 NUL。
    #[test]
    fn truncated_json_is_an_error() {
        let mut datagram = hex(
            "01 06 00 00 00 00 00 00 01 10 00 00 00 00 00 00 4C 4D 49 47 4F 50 58 4C 20 FC FF FF FF FF FF FF",
        );
        datagram
            .extend_from_slice(br#"{"Address":"192.168.1.10","AddressConflict":false,"AppId":""#);
        datagram.resize(32 + 1504, b'x');
        datagram.push(0);
        assert_eq!(datagram.len(), 1537);
        assert!(matches!(
            parse_announce(&datagram),
            Err(DiscoveryError::Json(_))
        ));
    }

    // 字段约定：缺失或类型不对的字段取默认值，其余字段照常解码（C++ 抛异常并丢弃整条 Announce）。
    #[test]
    fn bad_fields_fall_back_to_defaults() {
        let json = br#"{"Address":"","Mask":"255.255.255","Gateway":7,"AppId":5,"AppName":"ok","SerialNumber":"abc","ControlPort":70000,"GdpPort":-1,"WebPort":80.0,"Dhcp":1,"IsRemote":"true","HMIStatus":"0"}"#;
        let expected = Instance {
            app_name: "ok".into(),
            ..Instance::default()
        };
        assert_eq!(parse_announce(&announce(1, json)).unwrap(), expected);
    }

    // Length 字段与数据报长度不符、签名或 MsgId 不对、JSON 不是 UTF-8 或不是对象 → Malformed；
    // 每个真前缀（含短于 32 字节的）都是 Malformed，不越界读取。
    #[test]
    fn malformed_announces_are_rejected() {
        let base = announce(1, b"{}");
        let mutations: [fn(&mut Vec<u8>); 4] =
            [|d| d[0] += 1, |d| d.push(0), |d| d[16] ^= 1, |d| d[8] = 2];
        let mut cases: Vec<Vec<u8>> = mutations
            .iter()
            .map(|mutate| {
                let mut d = base.clone();
                mutate(&mut d);
                d
            })
            .collect();
        cases.push(announce(1, b"[]"));
        cases.push(announce(1, b"{\"AppId\":\"\xFF\"}"));
        let whole = announce(1, G14_JSON.as_bytes());
        cases.extend((0..whole.len()).map(|end| whole[..end].to_vec()));

        for datagram in cases {
            assert!(
                matches!(parse_announce(&datagram), Err(DiscoveryError::Malformed)),
                "{datagram:02X?}"
            );
        }
    }
}
