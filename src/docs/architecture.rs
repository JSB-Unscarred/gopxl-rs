//! # 架构
//!
//! ## 分层
//!
//! - 编解码（纯函数与纯数据结构，不做 I/O、不起任务）：[`value`](crate::value)（[`Value`](crate::Value) 与
//!   MessagePack）、[`frame`](crate::frame)（长度前缀帧）、[`control`](crate::control)（控制通道消息、FIFO 对应器
//!   `ControlCore`、订阅匹配 [`SubscriptionExpr`](crate::control::SubscriptionExpr)）、[`gdp`](crate::gdp)（GDP 消息）、
//!   [`discovery`](crate::discovery)（发现报文）、[`schema`](crate::schema)（schema 导航与校验）。
//! - 连接（tokio 任务与 socket）：主连接 [`Connection`](crate::control::Connection) / [`Client`](crate::control::Client)、
//!   流连接 [`StreamHandle`](crate::control::StreamHandle)、文件传输连接 [`Client::transfer`](crate::control::Client::transfer)、
//!   GDP 连接 [`gdp::Client`](crate::gdp::Client)、UDP 发现 [`discover`](crate::discovery::discover)。
//! - 门面：[`System`](crate::System)（一台设备，拥有主连接）；资源层 [`Resource`](crate::resource::Resource)（路径、
//!   读缓存、schema 校验、订阅）。
//!
//! 所有 I/O 都是 `async fn`，在调用方的 tokio 运行时上执行。
//!
//! ## 数据流
//!
//! 控制通道的请求：[`Transaction::response`](crate::control::Transaction::response) → 取写锁 → `ControlCore::register`
//! （槽位入 FIFO，返回 `oneshot` 接收端）→ 写出整帧 → 释放写锁 → 等应答（带超时）。
//!
//! 主连接的读任务：socket → [`FrameReader`](crate::frame::FrameReader) → [`Inbound::decode`](crate::control::Inbound::decode)
//! → 取分派锁 → 响应交给 FIFO 队首槽位（接收端已丢弃的槽位照样出队），通知按
//! [`SubscriptionExpr::delivers`](crate::control::SubscriptionExpr::delivers) 放进各订阅的有界队列（满了丢最老的，
//! 计入 [`Notice::Lagged`](crate::control::Notice::Lagged)）。
//!
//! 流：[`open_stream`](crate::control::Client::open_stream) 开一条连接，写出 `stream` 请求；读任务把应答交给
//! `oneshot`，流消息进容量 4 的 `mpsc`（应答之前到达的先缓存，应答之后按原序放入）。
//!
//! 文件传输：[`transfer`](crate::control::Client::transfer) 开一条连接，写出请求，读到第一条响应即关闭。
//!
//! GDP：读任务 socket → `FrameReader` → [`decode_message`](crate::gdp::decode_message) → 容量 4 的 `mpsc`；
//! [`receive`](crate::gdp::Client::receive) 取消息直到 `is_last_msg`，未完成的数据集留到下一次。
//!
//! 发现：每个 IPv4 网卡一个发送 socket，在 0、300、1000 ms 广播 Discover；接收 socket 收 Announce，`select!` 直到
//! 超时；开启经典协议时另有 3220 端口的一对收发。
//!
//! 资源：[`data`](crate::resource::Resource::data) 先 `refresh`（连接的写计数变了、连接断开或订阅队列有通知时丢弃
//! 缓存）→ 缓存为空时 `read` → 缓存；[`update`](crate::resource::Resource::update) 开启校验时先按 `_schema` 逐值校验，
//! 再发补丁。`create` / `update` / `delete` / `call` 在写锁内给写计数加一，文件传输在完成后加一。
//!
//! ## 所有权
//!
//! - `Connection` 唯一拥有主连接，不 `Clone`；`Client` 是 `Arc` 句柄，可 clone、可移到别的任务，不让连接存活。
//!   `Connection` Drop → `teardown`：记下原因、shutdown、中止读任务、挂起的请求以同一个 `Arc<DisconnectReason>` 失败、
//!   关闭订阅队列。读任务出错或 EOF 时走同一条路径。
//! - `StreamHandle`、`gdp::Client` 各拥有一条连接与它的读任务，Drop 即 shutdown 并中止读任务。
//! - [`Subscription`](crate::control::Subscription) 拥有自己的队列，读任务只持 `Weak`。
//!   [`unsubscribe`](crate::control::Subscription::unsubscribe) 或 Drop 时从分派表注销；同一表达式的最后一个本地订阅
//!   写出 `unSub`（Drop 时交给一个 task）。
//! - [`Transaction`](crate::control::Transaction) 持 `Client` 克隆与请求，`response` 时才写出。
//! - `Resource` 由调用方唯一拥有：持 `Client`、数据与 schema 缓存、可选的 `Subscription`。
//! - `FrameReader` 切出的帧独占内存；解码得到的 `Value::Bin` 与 GDP 字节切片引用所在的帧，多字节数组解码为自有的
//!   `Vec`。`Value` 是值语义，`Clone` 深拷贝（`Bin` 只加引用计数）。
//!
//! ## 任务与锁
//!
//! - 每条主连接、流连接、GDP 连接一个读任务；请求、文件传输与发现在调用方的任务上执行。读任务不执行用户代码：
//!   通知由调用方拉取，流与 GDP 的有界通道满时读任务等待，背压经 TCP 传到服务端。
//! - 主连接两把锁，顺序固定为写锁 → 分派锁：写锁是 `tokio::sync::Mutex<写半边>`，覆盖登记槽位、整帧写出与订阅的
//!   登记、注销；分派锁是 `std::sync::Mutex`，读任务处理每一帧时持有，不跨 `await`。
//!
//! ## 超时
//!
//! 建立控制连接 1 s、GDP 连接 5 s；应答默认 3 s（[`Transaction::timeout`](crate::control::Transaction::timeout)），`sub`
//! 与 `stream` 的应答 5 s，`Resource` 5 s，`System` 15 s；写出每 64 KiB 块 10 s；读不设超时，文件传输连接按连续无
//! 数据的时长；keepalive 空闲 30 s 后每 10 s 探测。
//!
//! ## 错误
//!
//! - 值的读取（`pointer`、`Index`、`as_*`）返回 `Option` 或 `&Value::Nil`。
//! - [`PointerError`](crate::PointerError)：`Value::set` 的指针穿过标量或不存在的数组下标，文档不变。
//! - [`msgpack::EncodeError`](crate::value::msgpack::EncodeError)：长度超过 `u32::MAX`；
//!   [`msgpack::DecodeError`](crate::value::msgpack::DecodeError)：消息体不是一条完整、合法的 MessagePack 文档；
//!   `serde_json::Error`：JSON 文本不合法；`serde::de::value::Error`：`T::deserialize(&value)` 失败。
//! - [`FrameError`](crate::frame::FrameError)：读取失败、读取超时、帧中途 EOF 或长度越界，连接随之关闭。
//! - [`control::InboundError`](crate::control::InboundError)：服务端从不产生的帧，连接随之关闭；未知的 `type` 是
//!   `Inbound::Other`。
//! - [`gdp::Error`](crate::gdp::Error)：`Truncated`、`Malformed`。信封或 CAS 坏掉时 `decode_message` 返回 `Err`，
//!   之后的部分坏掉时 `body` 为 `Body::Invalid`。
//! - [`discovery::DiscoveryError`](crate::discovery::DiscoveryError)：丢弃该数据报；`Command` 是客户端发出的命令。
//! - [`DisconnectReason`](crate::DisconnectReason)：`Closed`（本端关闭）、`Eof`（服务端关闭）、`Read` / `Write`
//!   （socket 错误或超时）、`Decode`（坏帧）；同一条连接上所有挂起的请求得到同一个 `Arc`。
//! - [`Error`](crate::Error)：`Connect`、`Timeout`（槽位成为墓碑，连接照常可用）、`Disconnected`、`Request`
//!   （`status` 不是 OK，携带完整响应）、`Payload`（要读取的字段缺失或无法反序列化）、`InvalidArgument`、
//!   `Validation`（补丁不符合 schema，请求未发出）。
//! - [`gdp::RecvError`](crate::gdp::RecvError)：`Timeout`、`Incomplete`（已收到的保留）、`Disconnected`。
//! - [`control::SubExprError`](crate::control::SubExprError)：订阅表达式含正则元字符或 NUL，或无法编译。
//! - schema：[`schema_for_path`](crate::schema::schema_for_path) 找不到路径时为 `None`；
//!   [`validate`](crate::schema::validate) 返回被违反的关键字列表，空表即通过。
//!
//! 未经真机验证的协议假设见 [`assumptions`](crate::docs::assumptions)。
