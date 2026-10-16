//! Gocator 6.x 经典发现协议（UDP 3220），对应 C++ 的 `ClassicDiscoveryProto`，用来找到 GoPxL 之前的 Gocator 设备。
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
//! [`handle_reply`] 修掉 C++ 的两个问题：按 `MsgId` 而不是数据报长度分派（C++ 要求 `GetInfo` 回复至少 112 字节，
//! 丢掉只含 99 字节已知字段的回复）；`app_id` 与 `serial_number` 都用截断后的 `u32` 序列号。读取方式与
//! 父模块相同，坏报文只得到 `Err`。

use std::net::Ipv4Addr;

use bytes::Buf;

use super::{DiscoveryError, Instance};

/// 经典发现的 UDP 端口：命令与回复都广播到这个端口。
pub const PORT: u16 = 3220;

/// 签名 `"LMIUDP\0\0"`。
const SIGNATURE: u64 = 0x0000_5044_5549_4D4C;
const GET_IP: u64 = 0x0001;
const GET_INFO: u64 = 0x0005;
const GET_IP_REPLY: u64 = 0x1001;
const GET_INFO_REPLY: u64 = 0x1005;
const OK: u64 = 1;
/// `GetIP` 回复的固定长度（`ClassicDiscoveryGetIPReply`）。
const GET_IP_REPLY_LEN: usize = 80;
/// `GetInfo` 回复中已知字段的总长度，之后可能跟着可变的尾部。
const GET_INFO_REPLY_LEN: usize = 99;

/// `GetIP` 命令，对应 `ClassicDiscoveryProto::BroadcastGetIP`。
pub fn encode_get_ip() -> [u8; 32] {
    command(GET_IP, 0)
}

/// 查询 `serial` 的 `GetInfo` 命令，对应 `ClassicDiscoveryProto::SendGetInfo`。
pub fn encode_get_info(serial: u32) -> [u8; 32] {
    command(GET_INFO, serial)
}

fn command(msg_id: u64, serial: u32) -> [u8; 32] {
    let mut out = [0; 32];
    super::put_u64s(&mut out, &[32, msg_id, SIGNATURE, serial.into()]);
    out
}

/// 处理经典 socket 上的一个数据报；`instances` 是本次发现的结果，只查找、修改其中的经典实例（两种协议之间不去重，
/// 与 C++ 相同）。
///
/// - `GetIP` 回复（80 字节）且序列号是新的：追加经典实例（`app_id` 为 `"classic-<序列号>"`、`app_name` 为
///   `"Gocator Classic"`），返回 `Some(序列号)`，调用方随后广播 [`encode_get_info`]；序列号已存在时返回 `None`；
/// - `GetInfo` 回复（Length ≥ 99 且不超过数据报）：按序列号就地更新 `app_version`、`control_port`、
///   `gdp_port`（`DataPort`）、`web_port`，返回 `None`；未知序列号忽略；
/// - 偏移 16 是签名的 `GetIP` / `GetInfo` 命令（包括自己广播的）返回 [`DiscoveryError::Command`]。
pub fn handle_reply(
    datagram: &[u8],
    instances: &mut Vec<Instance>,
) -> Result<Option<u32>, DiscoveryError> {
    let mut buf = datagram;
    let length = usize::try_from(buf.try_get_u64_le()?).unwrap_or(usize::MAX);
    let msg_id = buf.try_get_u64_le()?;
    let third = buf.try_get_u64_le()?; // 命令的签名，回复的状态
    let fourth = buf.try_get_u64_le()?; // 命令的序列号，回复的签名
    match msg_id {
        GET_IP | GET_INFO if third == SIGNATURE => Err(DiscoveryError::Command),
        _ if third != OK || fourth != SIGNATURE => Err(DiscoveryError::Malformed),
        GET_IP_REPLY if length == GET_IP_REPLY_LEN && datagram.len() == GET_IP_REPLY_LEN => {
            get_ip_reply(buf, instances)
        }
        GET_INFO_REPLY if (GET_INFO_REPLY_LEN..=datagram.len()).contains(&length) => {
            get_info_reply(buf, instances)?;
            Ok(None)
        }
        _ => Err(DiscoveryError::Malformed),
    }
}

/// `GetIP` 回复头部之后的部分：`i64` 序列号、`i64` DHCP，以及地址、掩码、网关，每个前面有 4 字节保留字。
fn get_ip_reply(
    mut buf: &[u8],
    instances: &mut Vec<Instance>,
) -> Result<Option<u32>, DiscoveryError> {
    // 序列号只取低 32 位，与 C++ 的 `SerialNumber` 相同。
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

/// `GetInfo` 回复头部之后的部分。字段紧密排列（C++ 用 kSerializer 逐个读取），只取序列号、版本与端口，与 C++ 相同。
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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::{encode_get_info, encode_get_ip, handle_reply};
    use crate::discovery::{DiscoveryError, Instance};
    use crate::hex;

    /// §4.2 fixture（derived）：序列号 123456、DHCP、192.168.1.10/24、网关 .1。
    const GET_IP_REPLY: &str = "50 00 00 00 00 00 00 00 01 10 00 00 00 00 00 00 01 00 00 00 00 00 00 00 4C 4D 49 55 44 50 00 00 40 E2 01 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 C0 A8 01 0A 00 00 00 00 FF FF FF 00 00 00 00 00 C0 A8 01 01 00 00 00 00 00 00 00 00";
    /// §4.4 fixture（derived）：99 字节，序列号 123456，版本 0x06030A14，端口 3190/3192/3194/3196/80。
    const GET_INFO_REPLY: &str = "63 00 00 00 00 00 00 00 05 10 00 00 00 00 00 00 01 00 00 00 00 00 00 00 4C 4D 49 55 44 50 00 00 00 00 40 E2 01 00 14 0A 03 06 00 00 00 00 00 00 00 00 01 04 C0 A8 01 0A 00 00 00 00 00 00 00 00 00 00 00 00 18 00 00 00 04 C0 A8 01 01 00 00 00 00 00 00 00 00 00 00 00 00 76 0C 78 0C 7A 0C 7C 0C 50 00";

    // G16a/G16b（derived）：GetIP 与 GetInfo 命令与 C++ 逐字节一致；收到命令（含自己的回环）报 Command。
    #[test]
    fn commands_match_g16_and_are_ignored() {
        let cases = [
            (
                encode_get_ip(),
                "20 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 4C 4D 49 55 44 50 00 00 00 00 00 00 00 00 00 00",
            ),
            (
                encode_get_info(12_345),
                "20 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00 4C 4D 49 55 44 50 00 00 39 30 00 00 00 00 00 00",
            ),
        ];
        for (command, golden) in cases {
            assert_eq!(command.as_slice(), hex(golden));
            let mut instances = Vec::new();
            assert!(matches!(
                handle_reply(&command, &mut instances),
                Err(DiscoveryError::Command)
            ));
            assert_eq!(instances, []);
        }
    }

    // §4.2 / §4.4 fixture（derived）：GetIP 回复追加经典实例并要求发 GetInfo，重复的序列号忽略；99 字节的
    // GetInfo 回复（C++ 按 ≥ 112 字节分派会丢掉）就地更新版本与端口；未知序列号的 GetInfo 回复忽略。
    #[test]
    fn replies_add_and_update_classic_instances() {
        let get_ip = hex(GET_IP_REPLY);
        let mut instances = Vec::new();
        assert_eq!(
            handle_reply(&get_ip, &mut instances).unwrap(),
            Some(123_456)
        );
        assert_eq!(handle_reply(&get_ip, &mut instances).unwrap(), None);
        let mut expected = Instance {
            classic: true,
            address: Some(Ipv4Addr::new(192, 168, 1, 10)),
            mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            app_id: "classic-123456".into(),
            app_name: "Gocator Classic".into(),
            serial_number: Some(123_456),
            dhcp: true,
            ..Instance::default()
        };
        assert_eq!(instances, [expected.clone()]);

        let mut get_info = hex(GET_INFO_REPLY);
        assert_eq!(handle_reply(&get_info, &mut instances).unwrap(), None);
        expected.app_version = "6.3.10.20".into();
        expected.control_port = 3190;
        expected.gdp_port = 3196;
        expected.web_port = 80;
        assert_eq!(instances, [expected.clone()]);

        get_info[34] = 0x41; // 序列号 123457
        assert_eq!(handle_reply(&get_info, &mut instances).unwrap(), None);
        assert_eq!(instances, [expected]);
    }

    // Length 与数据报长度不符、状态 ≠ 1、签名或 MsgId 不对 → Malformed；签名不对的命令也是 Malformed；
    // 两种回复与命令的每个真前缀都是 Malformed，不越界读取。
    #[test]
    fn malformed_replies_are_rejected() {
        let get_ip = hex(GET_IP_REPLY);
        let get_info = hex(GET_INFO_REPLY);
        let command = encode_get_ip().to_vec();
        let mutations: [fn(&mut Vec<u8>); 4] =
            [|d| d[0] += 1, |d| d[16] = 2, |d| d[24] ^= 1, |d| d[8] = 2];
        let mut cases = Vec::new();
        let mut unsigned_command = command.clone();
        unsigned_command[16] ^= 1;
        cases.push(unsigned_command);
        cases.extend((0..command.len()).map(|end| command[..end].to_vec()));
        for base in [&get_ip, &get_info] {
            for mutate in mutations {
                let mut d = base.clone();
                mutate(&mut d);
                cases.push(d);
            }
            cases.extend((0..base.len()).map(|end| base[..end].to_vec()));
        }
        let mut long_get_ip = get_ip.clone();
        long_get_ip.push(0);
        let mut short_get_info = get_info.clone();
        short_get_info[0] = 98;
        short_get_info.pop();
        cases.extend([long_get_ip, short_get_info]);

        let mut instances = Vec::new();
        for datagram in cases {
            assert!(
                matches!(
                    handle_reply(&datagram, &mut instances),
                    Err(DiscoveryError::Malformed)
                ),
                "{datagram:02X?}"
            );
        }
        assert_eq!(instances, []);
    }
}
