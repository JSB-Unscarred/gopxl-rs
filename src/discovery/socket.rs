//! UDP 发现的 socket 部分：[`discover`] 在调用方的任务上广播命令、收集回复。
//!
//! 单个网卡发送失败不影响其他网卡；接收缓冲 2048 字节，大于服务端最长的 1536 字节回复；Discover 按 0、300、
//! 1000 ms 重发，UDP 丢一个包不会漏掉设备；每次调用都从全新的状态开始。

use std::future::pending;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::time::{Instant, sleep_until};

use super::{Instance, PORT, classic, encode_discover, parse_announce};

/// Discover（与经典协议的 `GetIP`）的发送时刻，从调用开始计。
const RESEND_AT: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_millis(300),
    Duration::from_millis(1000),
];

/// 发现所有 GoPxL 实例，等满 `timeout` 才返回：回复的数量事先未知。
///
/// 实例按（地址，Web 端口）去重，先到者胜。绑定 3320 失败时返回错误：服务端本身也绑定这个端口，所以接收
/// socket 设置 `SO_REUSEADDR`，本机运行 GoPxL PC 应用时同样能发现它。
pub async fn discover(timeout: Duration) -> io::Result<Vec<Instance>> {
    discover_all(timeout, false).await
}

/// 同 [`discover`]，另在 UDP 3220 上发现 Gocator 6.x 经典设备（[`classic`]），排在 GoPxL 实例之后。
pub async fn discover_with_classic(timeout: Duration) -> io::Result<Vec<Instance>> {
    discover_all(timeout, true).await
}

/// 两个入口共用的发现流程；`classic` 决定是否同时发经典协议的命令。两个接收 socket 与重发、截止时间一起
/// `select!`，谁先就绪处理谁。
async fn discover_all(timeout: Duration, classic: bool) -> io::Result<Vec<Instance>> {
    let start = Instant::now();
    let deadline = start + timeout;
    let gopxl = receiver(PORT)?;
    let legacy = if classic {
        Some(receiver(classic::PORT)?)
    } else {
        None
    };
    let senders = senders()?;
    let mut instances = Vec::new();
    let mut classics = Vec::new();
    let mut buf = [0; 2048];
    let mut legacy_buf = [0; 2048];
    let mut resends = RESEND_AT.iter().peekable();
    loop {
        let resend_at = resends.peek().map(|&&at| start + at);
        tokio::select! {
            () = sleep_until(deadline) => break,
            () = async {
                match resend_at {
                    Some(at) => sleep_until(at).await,
                    None => pending().await,
                }
            } => {
                resends.next();
                broadcast(&senders, &encode_discover(), PORT).await;
                if classic {
                    broadcast(&senders, &classic::encode_get_ip(), classic::PORT).await;
                }
            }
            // 接收错误（例如别的程序发来超过缓冲区的数据报、Windows 上的 ICMP 重置）只丢掉这一条。
            received = gopxl.recv(&mut buf) => {
                // 其他客户端的 Discover（包括自己的回环）与坏报文直接跳过。
                if let Ok(found) = received.and_then(|n| parse_announce(buf.get(..n).unwrap_or_default()).map_err(io::Error::other)) {
                    let duplicate = instances.iter().any(|i: &Instance| {
                        i.address == found.address && i.web_port == found.web_port
                    });
                    if !duplicate {
                        instances.push(found);
                    }
                }
            }
            received = async {
                match &legacy {
                    Some(socket) => socket.recv(&mut legacy_buf).await,
                    None => pending().await,
                }
            } => {
                if let Ok(n) = received {
                    let datagram = legacy_buf.get(..n).unwrap_or_default();
                    if let Ok(Some(serial)) = classic::handle_reply(datagram, &mut classics) {
                        broadcast(&senders, &classic::encode_get_info(serial), classic::PORT).await;
                    }
                }
            }
        }
    }
    instances.append(&mut classics);
    Ok(instances)
}

/// 绑定 `0.0.0.0:port` 的接收 socket。命令与回复都广播到这个端口；只收不发，不需要 `SO_BROADCAST`。
fn receiver(port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

/// 每个已启用的非回环 IPv4 地址一个发送 socket，绑定临时端口：受限广播只从绑定地址所在的网卡发出，多网卡的工控机
/// 要逐个发送。包含 169.254.x.x：直连设备、无 DHCP 服务时网卡就是这类地址。单个网卡出错只跳过它。跳过回环见
/// [协议假设 D2](crate::docs::assumptions)，临时端口见 [协议假设 D1](crate::docs::assumptions)。
fn senders() -> io::Result<Vec<UdpSocket>> {
    let senders = if_addrs::get_if_addrs()?
        .into_iter()
        .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(ip) => {
                let socket = std::net::UdpSocket::bind((ip, 0)).ok()?;
                socket.set_broadcast(true).ok()?;
                socket.set_nonblocking(true).ok()?;
                UdpSocket::from_std(socket).ok()
            }
            IpAddr::V6(_) => None,
        })
        .collect();
    Ok(senders)
}

/// 从每个网卡广播一个命令；发送错误只影响该网卡。
async fn broadcast(senders: &[UdpSocket], packet: &[u8], port: u16) {
    for socket in senders {
        let _ = socket.send_to(packet, (Ipv4Addr::BROADCAST, port)).await;
    }
}
