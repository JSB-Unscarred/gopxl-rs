//! 连接层的进程内测试：回环地址上的 `TcpListener` 扮演服务端，按脚本收发 carrier 帧与 GDP 消息。只钉住本 crate
//! 自有、无法与官方 SDK 对照的线程与所有权约定（FIFO 墓碑、拆连接一次、Drop 不阻塞、订阅的本地引用计数、流的
//! 早到消息、资源缓存失效、GDP 组集）；协议事实留给真机上的示例验证。

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::Bytes;
use gopxl::control::{Client, DisconnectReason, Method, Notice, Request, StreamState};
use gopxl::gdp::{Body, GdpClient, RecvError};
use gopxl::resource::Resource;
use gopxl::value::msgpack;
use gopxl::{Error, Value};

/// carrier 类型：MessagePack。
const MSGPACK: u16 = 0xB000;

/// 脚本与客户端每一步等待的上限：对方卡住时测试失败而不是挂起。
const STEP: Duration = Duration::from_secs(5);

/// 假服务端：`script` 在自己的线程里处理依次到来的连接，[`finish`](Self::finish) 把脚本里的断言失败带回测试线程。
struct Server {
    addr: SocketAddr,
    script: JoinHandle<()>,
}

impl Server {
    fn start(script: impl FnOnce(&TcpListener) + Send + 'static) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let script = thread::spawn(move || script(&listener));
        Self { addr, script }
    }

    fn finish(self) {
        self.script.join().unwrap();
    }
}

/// 接受下一条连接，读等待上限 [`STEP`]。
fn accept(listener: &TcpListener) -> TcpStream {
    let (conn, _) = listener.accept().unwrap();
    conn.set_read_timeout(Some(STEP)).unwrap();
    conn
}

/// 读一个客户端 → 服务端的 carrier 帧（`u32 Length`、`u16 0xB000`、`u32 N`、N 字节 MessagePack），返回消息体。
fn read_request(conn: &mut TcpStream) -> Value {
    let mut length = [0; 4];
    conn.read_exact(&mut length).unwrap();
    let mut frame = vec![0; u32::from_le_bytes(length) as usize - 4];
    conn.read_exact(&mut frame).unwrap();
    assert_eq!(frame[..2], MSGPACK.to_le_bytes());
    msgpack::decode(&Bytes::from(frame.split_off(6))).unwrap()
}

/// 读一个请求并断言它的方法与路径。
fn expect_request(conn: &mut TcpStream, method: &str, path: &str) -> Value {
    let request = read_request(conn);
    assert_eq!(request["method"], Value::from(method));
    assert_eq!(request["path"], Value::from(path));
    request
}

/// 写一个服务端 → 客户端的帧（`u32 Length = 14 + N`、`u16 0xB000`、`i32 CarrierStatus = 1`、`u32 N`、消息体）。
fn write_message(conn: &mut TcpStream, body: &Value) {
    let mut payload = Vec::new();
    msgpack::encode(body, &mut payload).unwrap();
    let n = u32::try_from(payload.len()).unwrap();
    let mut frame = (14 + n).to_le_bytes().to_vec();
    frame.extend(MSGPACK.to_le_bytes());
    frame.extend(1_i32.to_le_bytes());
    frame.extend(n.to_le_bytes());
    frame.extend(payload);
    conn.write_all(&frame).unwrap();
}

/// 等对端关闭：客户端 Drop 即 shutdown，这里读到 EOF（Windows 上也可能是连接重置）；[`STEP`] 内没有关闭即失败。
fn wait_for_close(conn: &mut TcpStream) {
    let mut byte = [0];
    match conn.read(&mut byte) {
        Ok(0) => {}
        Ok(_) => panic!("unexpected bytes before the client closed"),
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            panic!("client did not close within {STEP:?}")
        }
        Err(_) => {}
    }
}

fn json(text: &str) -> Value {
    text.parse().unwrap()
}

/// `status` 为 OK 的响应。
fn response(payload: impl Into<Value>) -> Value {
    let mut message = json(r#"{"type":"response","status":1}"#);
    message.set("/payload", payload).unwrap();
    message
}

/// `path` 的 `updated` 通知。
fn notification(path: &str) -> Value {
    let mut message = json(r#"{"type":"notification","eventType":"updated"}"#);
    message.set("/path", path).unwrap();
    message
}

/// 流 7 的一条消息。
fn stream_message(state: &str, payload: impl Into<Value>) -> Value {
    let mut message = json(r#"{"type":"stream","streamId":7,"status":1,"path":"/system/metrics"}"#);
    message.set("/streamStatus", state).unwrap();
    message.set("/payload", payload).unwrap();
    message
}

/// 一条 GDP Signal 消息（类型 1，只有 CAS）：`[u32 size][u16 type][CAS]`，CAS 不带 transform、包围盒与 gdpId。
fn gdp_signal(is_last: bool) -> Vec<u8> {
    let mut cas = 28_u32.to_le_bytes().to_vec();
    cas.extend([0, 0, 0]); // spaceType、无 transform、无包围盒
    cas.extend(0_u32.to_le_bytes()); // arrayCount
    cas.extend(0_u32.to_le_bytes()); // arrayIndex
    cas.extend(0_u16.to_le_bytes()); // dataSourceId 为空
    cas.extend(0_u16.to_le_bytes()); // stampSourceId 为空
    cas.extend(0_u64.to_le_bytes()); // dataSetId
    cas.push(u8::from(is_last));
    assert_eq!(cas.len(), 28);
    let mut message = (6 + 28_u32).to_le_bytes().to_vec();
    message.extend(1_u16.to_le_bytes());
    message.extend(cas);
    message
}

// FIFO 对应与墓碑：超时的请求丢弃接收端后，它的槽位仍占住 FIFO 的位置吸收迟到的响应；下一个请求拿到自己的响应，
// 连接照常可用（C++ SDK 此后全部错位）。Client Drop 即 shutdown，对端读到 EOF。
#[test]
fn late_response_is_absorbed_by_the_timed_out_slot() {
    let server = Server::start(|listener| {
        let mut conn = accept(listener);
        expect_request(&mut conn, "read", "/slow");
        expect_request(&mut conn, "read", "/fast");
        write_message(&mut conn, &response("slow"));
        write_message(&mut conn, &response("fast"));
        wait_for_close(&mut conn);
    });
    let client = Client::connect(server.addr).unwrap();
    let slow = client
        .read("/slow")
        .timeout(Duration::from_millis(50))
        .response();
    assert!(matches!(slow, Err(Error::Timeout { .. })));
    let fast = client.read("/fast").response().unwrap();
    assert_eq!(fast.payload, Value::from("fast"));
    assert!(client.is_connected());
    drop(client);
    server.finish();
}

// 拆连接只执行一次：服务端关闭连接时，所有挂起的请求以同一个 Arc 原因（Eof）失败；之后 is_connected 为假，新请求与
// 阻塞在 recv_timeout 上的订阅都立即报 Disconnected。
#[test]
fn server_close_fails_every_pending_request_with_one_reason() {
    let server = Server::start(|listener| {
        let mut conn = accept(listener);
        expect_request(&mut conn, "sub", "/system");
        write_message(&mut conn, &response(Value::Nil));
        expect_request(&mut conn, "read", "/a");
        expect_request(&mut conn, "read", "/b");
        // 两个请求都已入队，不回应，直接关闭。
    });
    let client = Client::connect(server.addr).unwrap();
    let subscription = client.subscribe("/system").unwrap();
    let a = client.read("/a");
    let b = client.read("/b");
    let Err(Error::Disconnected(reason_a)) = a.response() else {
        panic!("expected Disconnected")
    };
    let Err(Error::Disconnected(reason_b)) = b.response() else {
        panic!("expected Disconnected")
    };
    assert!(matches!(*reason_a, DisconnectReason::Eof));
    assert!(Arc::ptr_eq(&reason_a, &reason_b));
    assert!(!client.is_connected());

    let start = Instant::now();
    assert!(matches!(
        client.read("/c").response(),
        Err(Error::Disconnected(_))
    ));
    assert!(matches!(
        subscription.recv_timeout(Duration::MAX),
        Err(Error::Disconnected(_))
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    server.finish();
}

// Drop 从不阻塞：读线程卡在半帧里时，Client 的 Drop 只 shutdown、不 join，立即返回（C++ 先 join 后关闭 socket，
// 此时永远挂住）。挂起的请求以 Closed 失败；Handle 不让连接存活，之后经它发出的请求立即失败。
#[test]
fn drop_returns_while_the_reader_is_stuck_in_a_half_frame() {
    let (half_sent, half_received) = mpsc::channel();
    let (dropped, dropped_received) = mpsc::channel::<()>();
    let server = Server::start(move |listener| {
        let mut conn = accept(listener);
        // 只写出长度字段：声明一帧 100 字节，剩下的永远不来。
        conn.write_all(&100_u32.to_le_bytes()).unwrap();
        expect_request(&mut conn, "read", "/x");
        half_sent.send(()).unwrap();
        // 测试线程确认 Drop 已返回后才关闭；超过 STEP 也关闭，让一个会 join 的 Drop 结束、由测试里的计时报错。
        let _ = dropped_received.recv_timeout(STEP);
        wait_for_close(&mut conn);
    });
    let client = Client::connect(server.addr).unwrap();
    let handle = client.handle();
    let pending = client.read("/x");
    half_received.recv().unwrap();

    let start = Instant::now();
    drop(client);
    assert!(start.elapsed() < Duration::from_secs(1));
    dropped.send(()).unwrap();

    let Err(Error::Disconnected(reason)) = pending.response() else {
        panic!("expected Disconnected")
    };
    assert!(matches!(*reason, DisconnectReason::Closed));
    assert!(!handle.is_connected());
    assert!(matches!(
        handle.read("/y").response(),
        Err(Error::Disconnected(_))
    ));
    server.finish();
}

// 订阅：同一表达式的本地订阅各自收到通知，按表达式投递（不匹配的路径收不到）；Drop 时只有该表达式最后一个本地订阅
// 写出 unSub，不等应答。
#[test]
fn last_local_subscription_sends_unsub() {
    let server = Server::start(|listener| {
        let mut conn = accept(listener);
        for _ in 0..2 {
            expect_request(&mut conn, "sub", "/system");
            write_message(&mut conn, &response(Value::Nil));
        }
        write_message(&mut conn, &notification("/system"));
        write_message(&mut conn, &notification("/tools/1"));
        // 第一个订阅 Drop 后没有 unSub：下一帧已经是探针请求。
        expect_request(&mut conn, "read", "/probe");
        write_message(&mut conn, &response(Value::Nil));
        expect_request(&mut conn, "unSub", "/system");
        wait_for_close(&mut conn);
    });
    let client = Client::connect(server.addr).unwrap();
    let first = client.subscribe("/system").unwrap();
    let second = client.subscribe("/system").unwrap();
    for subscription in [&first, &second] {
        let notice = subscription.recv_timeout(STEP).unwrap().unwrap();
        assert!(matches!(notice, Notice::Event(n) if n.path == "/system"));
    }
    drop(first);
    client.read("/probe").response().unwrap();
    // 两条通知都排在探针的应答之前，/tools/1 的那条没有投给 /system 的订阅。
    assert!(second.recv_timeout(Duration::ZERO).unwrap().is_none());
    drop(second);
    drop(client);
    server.finish();
}

// 流：每个流一条自有连接，只写出 stream 请求；应答之前到达的流消息先缓存，应答之后按原顺序交给消费者；应答之后的
// 孤立响应忽略；结束消息照常取出。
#[test]
fn stream_delivers_early_data_after_the_reply_in_order() {
    let server = Server::start(|listener| {
        let _main = accept(listener);
        let mut conn = accept(listener);
        let request = expect_request(&mut conn, "stream", "/system/metrics");
        assert_eq!(request["payload"]["period"], Value::from(1000));
        write_message(&mut conn, &stream_message("streaming", 1));
        write_message(&mut conn, &response(json(r#"{"streamId":7}"#)));
        write_message(&mut conn, &response(Value::Nil));
        write_message(&mut conn, &stream_message("streaming", 2));
        write_message(&mut conn, &stream_message("complete", Value::Nil));
        wait_for_close(&mut conn);
    });
    let client = Client::connect(server.addr).unwrap();
    let stream = client
        .open_stream("/system/metrics", json(r#"{"period":1000}"#))
        .unwrap();
    let next = || stream.recv_timeout(STEP).unwrap().unwrap();
    assert_eq!(next().payload, Value::from(1));
    assert_eq!(next().payload, Value::from(2));
    assert_eq!(next().state, StreamState::Complete);
    drop(stream);
    drop(client);
    server.finish();
}

// 资源缓存按连接的写计数失效：命中缓存时不发请求；同一连接上的变更请求、订阅本身、订阅收到的通知都使下一次读取
// 重新向服务端请求；Resource Drop 时订阅随之退订。
#[test]
fn resource_cache_expires_on_writes_and_notices() {
    let server = Server::start(|listener| {
        let mut conn = accept(listener);
        let read = expect_request(&mut conn, "read", "/x");
        assert_eq!(read["args"]["expandLevel"], Value::from(0));
        write_message(&mut conn, &response(json(r#"{"v":1}"#)));
        // 第二次读命中缓存：下一帧已经是 update。
        expect_request(&mut conn, "update", "/y");
        write_message(&mut conn, &response(Value::Nil));
        expect_request(&mut conn, "read", "/x");
        write_message(&mut conn, &response(json(r#"{"v":2}"#)));
        expect_request(&mut conn, "sub", "/x/*");
        write_message(&mut conn, &response(Value::Nil));
        expect_request(&mut conn, "read", "/x");
        // 通知排在应答之前到达：刚读到的数据可能已过期，下一次访问重读。
        write_message(&mut conn, &notification("/x/child"));
        write_message(&mut conn, &response(json(r#"{"v":3}"#)));
        expect_request(&mut conn, "read", "/x");
        write_message(&mut conn, &response(json(r#"{"v":4}"#)));
        expect_request(&mut conn, "unSub", "/x/*");
        wait_for_close(&mut conn);
    });
    let client = Client::connect(server.addr).unwrap();
    let mut resource = Resource::new(client.handle(), "/x");
    assert_eq!(resource.get::<i64>("/v").unwrap(), 1);
    assert_eq!(resource.get::<i64>("/v").unwrap(), 1);
    assert!(!resource.has_remote_changes());
    client.update("/y", Value::object()).response().unwrap();
    assert!(resource.has_remote_changes());
    assert_eq!(resource.get::<i64>("/v").unwrap(), 2);
    resource.subscribe().unwrap();
    assert_eq!(resource.get::<i64>("/v").unwrap(), 3);
    assert!(resource.has_remote_changes());
    assert_eq!(resource.get::<i64>("/v").unwrap(), 4);
    drop(resource);
    drop(client);
    server.finish();
}

// 文件传输：在临时连接上完成一个往返，读到应答即关闭该连接；主连接不参与。
#[test]
fn transfer_round_trips_on_its_own_connection() {
    let server = Server::start(|listener| {
        let mut main = accept(listener);
        let mut conn = accept(listener);
        let request = expect_request(&mut conn, "call", "/system/commands/archive");
        assert_eq!(request["payload"]["data"], Value::from(vec![1_u8, 2, 3]));
        let mut payload = Value::object();
        payload.set("/data", vec![4_u8, 5]).unwrap();
        write_message(&mut conn, &response(payload));
        wait_for_close(&mut conn);
        wait_for_close(&mut main);
    });
    let client = Client::connect(server.addr).unwrap();
    let mut payload = Value::object();
    payload.set("/data", vec![1_u8, 2, 3]).unwrap();
    let request = Request::with_payload(Method::Call, "/system/commands/archive", payload);
    let response = client.transfer(request, STEP).unwrap();
    assert_eq!(response.payload["data"], Value::from(vec![4_u8, 5]));
    drop(client);
    server.finish();
}

// GDP：receive 以 is_last_msg 结束一个数据集；期限内未结束时报 Incomplete 并保留已收到的消息，下一次接着组装；
// 服务端关闭后（取完已到的消息）报 Disconnected。
#[test]
fn gdp_receive_assembles_a_data_set_across_calls() {
    let (sent, received) = mpsc::channel();
    let (go_on, wait) = mpsc::channel::<()>();
    let server = Server::start(move |listener| {
        let mut conn = accept(listener);
        conn.write_all(&gdp_signal(false)).unwrap();
        conn.write_all(&gdp_signal(false)).unwrap();
        sent.send(()).unwrap();
        wait.recv().unwrap();
        conn.write_all(&gdp_signal(true)).unwrap();
        // 数据集写完即关闭。
    });
    let mut gdp = GdpClient::connect(server.addr).unwrap();
    received.recv().unwrap();
    assert!(matches!(
        gdp.receive(Duration::from_secs(1)),
        Err(RecvError::Incomplete { messages: 2 })
    ));
    go_on.send(()).unwrap();
    let data_set = gdp.receive(STEP).unwrap();
    assert_eq!(data_set.len(), 3);
    assert!(data_set.iter().all(|m| matches!(m.body, Body::Signal)));
    assert!(data_set[2].cas.is_last_msg);
    assert!(matches!(gdp.receive(STEP), Err(RecvError::Disconnected(_))));
    server.finish();
}
