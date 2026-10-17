//! UDP 发现的 socket 部分：[`discover`] 在调用方线程上广播命令、收集回复，对应 C++ 的
//! `GoDiscoveryClient::BlockingDiscover`。
//!
//! 修掉 C++ 的问题（重写方案 §5.4 第 29–31 条）：单个网卡发送失败不影响其他网卡；接收缓冲 2048 字节，大于
//! 服务端最长的 1536 字节回复；Discover 按 0、300、1000 ms 重发，UDP 丢一个包不会漏掉设备；每次调用都是全新的状态。

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use super::{Instance, PORT, classic, encode_discover, parse_announce};

/// Discover（与经典协议的 `GetIP`）的发送时刻，从调用开始计。
const RESEND_AT: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_millis(300),
    Duration::from_millis(1000),
];

/// 空闲的接收 socket 每轮等待的时长：开启经典协议时两个 socket 轮流等待，它决定另一个 socket 上回复的处理延迟；
/// 排队的数据报每轮都取完，吞吐不受它限制。
const SLICE: Duration = Duration::from_millis(20);

/// 发现所有实例，等满 `timeout` 才返回（与 C++ 相同，回复的数量事先未知）。`classic` 为真时另在 UDP 3220 上
/// 发现 Gocator 6.x 设备，排在 GoPxL 实例之后。
///
/// GoPxL 实例按（地址，Web 端口）去重，先到者胜。绑定 3320 失败时返回错误：服务端本身也绑定这个端口，所以接收
/// socket 设置 `SO_REUSEADDR`，本机运行 GoPxL PC 应用时同样能发现它。
pub fn discover(timeout: Duration, classic: bool) -> io::Result<Vec<Instance>> {
    let start = Instant::now();
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
    let mut resends = RESEND_AT.iter().peekable();
    while start.elapsed() < timeout {
        if resends.next_if(|&&at| at <= start.elapsed()).is_some() {
            broadcast(&senders, &encode_discover(), PORT);
            if classic {
                broadcast(&senders, &classic::encode_get_ip(), classic::PORT);
            }
        }
        drain(
            &gopxl,
            &mut buf,
            timeout.saturating_sub(start.elapsed()),
            |datagram| {
                // 其他客户端的 Discover（包括自己的回环）与坏报文直接跳过。
                if let Ok(found) = parse_announce(datagram) {
                    let duplicate = instances.iter().any(|i: &Instance| {
                        i.address == found.address && i.web_port == found.web_port
                    });
                    if !duplicate {
                        instances.push(found);
                    }
                }
            },
        );
        if let Some(socket) = &legacy {
            drain(
                socket,
                &mut buf,
                timeout.saturating_sub(start.elapsed()),
                |datagram| {
                    if let Ok(Some(serial)) = classic::handle_reply(datagram, &mut classics) {
                        broadcast(&senders, &classic::encode_get_info(serial), classic::PORT);
                    }
                },
            );
        }
    }
    instances.append(&mut classics);
    Ok(instances)
}

/// 绑定 `0.0.0.0:port` 的接收 socket。命令与回复都广播到这个端口。
fn receiver(port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    Ok(socket.into())
}

/// 每个已启用的非回环（协议假设 D2）IPv4 地址一个发送 socket，绑定临时端口（协议假设 D1）：受限广播只从绑定地址
/// 所在的网卡发出，多网卡的工控机要逐个发送。包含 169.254.x.x：直连设备、没有 DHCP 时网卡就是这类地址。单个网卡
/// 出错只跳过它。
fn senders() -> io::Result<Vec<UdpSocket>> {
    let senders = if_addrs::get_if_addrs()?
        .into_iter()
        .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(ip) => {
                let socket = UdpSocket::bind((ip, 0)).ok()?;
                socket.set_broadcast(true).ok()?;
                Some(socket)
            }
            IpAddr::V6(_) => None,
        })
        .collect();
    Ok(senders)
}

/// 从每个网卡广播一个命令；发送错误只影响该网卡。
fn broadcast(senders: &[UdpSocket], packet: &[u8], port: u16) {
    for socket in senders {
        let _ = socket.send_to(packet, (Ipv4Addr::BROADCAST, port));
    }
}

/// 取完 `socket` 上排队的数据报，逐个交给 `handle`：空闲时最多等 [`SLICE`]，最长 `budget`（数据持续到达时也不越过
/// 截止时间）。超时与其他接收错误（例如别的程序发来超过缓冲区的数据报）都结束这一轮：数据报互相独立，UDP 的接收
/// 超时也不影响之后的接收。
fn drain(socket: &UdpSocket, buf: &mut [u8], budget: Duration, mut handle: impl FnMut(&[u8])) {
    let begin = Instant::now();
    let mut wait = budget.min(SLICE);
    while begin.elapsed() < budget {
        // set_read_timeout(Some(0)) 报 InvalidInput。
        let received = socket
            .set_read_timeout(Some(wait.max(Duration::from_millis(1))))
            .and_then(|()| socket.recv(buf));
        let Some(datagram) = received.ok().and_then(|n| buf.get(..n)) else {
            return;
        };
        handle(datagram);
        wait = Duration::ZERO;
    }
}
