//! Gocator 6.x 经典发现协议（UDP 3220），用来找到 GoPxL 之前的 Gocator 设备。
//!
//! 流程：广播 `GetIP`（[`encode_get_ip`]），每台设备回复地址；对每个新序列号广播 `GetInfo`（[`encode_get_info`]），
//! 设备回复版本与端口。整数都是小端，命令与回复的头部顺序不同：
//!
//! | 偏移 | 命令（32 字节） | 回复 |
//! |---|---|---|
//! | 0 | `i64` Length = 32 | `i64` Length |
//! | 8 | `i64` `MsgId`：`GetIP` 1、`GetInfo` 5 | `i64` `MsgId`：`0x1001`、`0x1005` |
//! | 16 | `i64` 签名 `"LMIUDP\0\0"` | `i64` 状态，1 为成功 |
//! | 24 | `i64` 序列号（`GetIP` 为 0） | `i64` 签名 |
//!
//! [`handle_reply`] 按 `MsgId` 分派，不看数据报长度：回复只要含有用到的字段即可解析。`app_id` 与 `serial_number`
//! 都用序列号的低 32 位。读取方式与父模块相同：Length 字段不校验，字段不全的坏报文只得到 `Err`。

use std::net::Ipv4Addr;

use bytes::Buf;

use super::{DiscoveryError, Instance};

/// 经典发现的 UDP 端口：命令与回复都广播到这个端口。
pub const PORT: u16 = 3220;

/// 签名 `"LMIUDP\0\0"`。
const SIGNATURE: u64 = 0x0000_5044_5549_4D4C;
/// `GetIP` 命令的 `MsgId`。
const GET_IP: u64 = 0x0001;
/// `GetInfo` 命令的 `MsgId`。
const GET_INFO: u64 = 0x0005;
/// `GetIP` 回复的 `MsgId`。
const GET_IP_REPLY: u64 = 0x1001;
/// `GetInfo` 回复的 `MsgId`。
const GET_INFO_REPLY: u64 = 0x1005;
/// 回复中表示成功的状态。
const OK: u64 = 1;

/// `GetIP` 命令。
pub fn encode_get_ip() -> [u8; 32] {
    command(GET_IP, 0)
}

/// 查询 `serial` 的 `GetInfo` 命令。
pub fn encode_get_info(serial: u32) -> [u8; 32] {
    command(GET_INFO, serial)
}

/// 32 字节的命令：Length、`MsgId`、签名与序列号。
fn command(msg_id: u64, serial: u32) -> [u8; 32] {
    let mut out = [0; 32];
    super::put_u64s(&mut out, &[32, msg_id, SIGNATURE, serial.into()]);
    out
}

/// 处理经典 socket 上的一个数据报；`instances` 是本次发现的结果，只查找、修改其中的经典实例（经典实例与 GoPxL
/// 实例之间不去重）。
///
/// - `GetIP` 回复且序列号是新的：追加经典实例（`app_id` 为 `"classic-<序列号>"`、`app_name` 为
///   `"Gocator Classic"`），返回 `Some(序列号)`，调用方随后广播 [`encode_get_info`]；序列号已存在时返回 `None`；
/// - `GetInfo` 回复：按序列号就地更新 `app_version`、`control_port`、`gdp_port`（`DataPort`）、`web_port`，
///   返回 `None`；未知序列号忽略；
/// - 偏移 16 是签名的 `GetIP` / `GetInfo` 命令（包括自己广播的）返回 [`DiscoveryError::Command`]。
pub fn handle_reply(
    datagram: &[u8],
    instances: &mut Vec<Instance>,
) -> Result<Option<u32>, DiscoveryError> {
    let mut buf = datagram;
    let _length = buf.try_get_u64_le()?;
    let msg_id = buf.try_get_u64_le()?;
    let third = buf.try_get_u64_le()?; // 命令的签名，回复的状态
    let fourth = buf.try_get_u64_le()?; // 命令的序列号，回复的签名
    match msg_id {
        GET_IP | GET_INFO if third == SIGNATURE => Err(DiscoveryError::Command),
        _ if third != OK || fourth != SIGNATURE => Err(DiscoveryError::Malformed),
        GET_IP_REPLY => get_ip_reply(buf, instances),
        GET_INFO_REPLY => get_info_reply(buf, instances).map(|()| None),
        _ => Err(DiscoveryError::Malformed),
    }
}

/// `GetIP` 回复头部之后的部分：`i64` 序列号、`i64` DHCP，以及地址、掩码、网关，每个前面有 4 字节保留字。
fn get_ip_reply(
    mut buf: &[u8],
    instances: &mut Vec<Instance>,
) -> Result<Option<u32>, DiscoveryError> {
    // 序列号只取 `i64` 字段的低 32 位，与 `Instance::serial_number` 的 `u32` 一致。
    let serial = buf.try_get_u32_le()?;
    skip(&mut buf, 4)?;
    if find(instances, serial).is_some() {
        return Ok(None);
    }
    let dhcp = buf.try_get_u64_le()? != 0;
    instances.push(Instance {
        classic: true,
        address: Some(read_ip(&mut buf)?),
        mask: Some(read_ip(&mut buf)?),
        gateway: Some(read_ip(&mut buf)?),
        app_id: format!("classic-{serial}"),
        app_name: "Gocator Classic".to_owned(),
        serial_number: Some(serial),
        dhcp,
        ..Instance::default()
    });
    Ok(Some(serial))
}

/// `GetInfo` 回复头部之后的部分。字段紧密排列，只取序列号、版本与端口。
fn get_info_reply(mut buf: &[u8], instances: &mut [Instance]) -> Result<(), DiscoveryError> {
    skip(&mut buf, 2)?; // attr_size，含义未知
    let serial = buf.try_get_u32_le()?;
    let [major, minor, release, build] = buf.try_get_u32_le()?.to_be_bytes();
    // Uptime（8）、Dhcp（1）、地址版本与 16 字节地址（17）、前缀长度（4）、网关版本与 16 字节网关（17）
    skip(&mut buf, 8 + 1 + 17 + 4 + 17)?;
    let control_port = buf.try_get_u16_le()?;
    skip(&mut buf, 4)?; // UpgradePort、HealthPort
    let data_port = buf.try_get_u16_le()?;
    let web_port = buf.try_get_u16_le()?;
    if let Some(instance) = find(instances, serial) {
        instance.app_version = format!("{major}.{minor}.{release}.{build}");
        instance.control_port = control_port;
        instance.gdp_port = data_port;
        instance.web_port = web_port;
    }
    Ok(())
}

/// 序列号为 `serial` 的经典实例。
fn find(instances: &mut [Instance], serial: u32) -> Option<&mut Instance> {
    instances
        .iter_mut()
        .find(|i| i.classic && i.serial_number == Some(serial))
}

/// 跳过 4 字节保留字，读取网络字节序的 IPv4 地址（第一个字节是第一段）。
fn read_ip(buf: &mut &[u8]) -> Result<Ipv4Addr, DiscoveryError> {
    skip(buf, 4)?;
    Ok(Ipv4Addr::from(buf.try_get_u32()?))
}

/// 跳过 `n` 字节不使用的字段。
fn skip(buf: &mut &[u8], n: usize) -> Result<(), DiscoveryError> {
    *buf = buf.get(n..).ok_or(DiscoveryError::Malformed)?;
    Ok(())
}
