//! # 设计
//!
//! 本 crate 直接实现 GoPxL 的线上协议，不包装厂商 SDK：`GoPxLSdk` 动态链接许可不明的 GoApi，包装就要随产物
//! 分发 `GoApi.dll` / `libGoApi.so`；它的缺陷（响应错位、每条消息一个线程、析构阻塞、GDP 解析错误）也无法在
//! 包装层修复。
//!
//! ## 分层
//!
//! - 编解码：值模型（[`Value`](crate::Value)、MessagePack）、长度前缀帧（[`frame`](crate::frame)）、控制通道
//!   消息与协议状态机（[`control`](crate::control)）、GDP 消息（[`gdp`](crate::gdp)）、发现报文
//!   （[`discovery`](crate::discovery)）与 schema 校验（[`schema`](crate::schema)）是纯函数或纯数据结构，不做 I/O、
//!   不起线程，直接用字节测试。解码输入来自网络的文件以
//!   `#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used, clippy::panic)]` 开头（子模块继承），
//!   读取都经过 `bytes::Buf` 的 `try_get_*`，坏输入只得到 `Err`；
//! - 连接：每种连接类型读写自己的 socket，把字节交给编解码层，相当于 FFI 封装中“方法直接调用 `sys`”。
//!   控制通道的主连接（[`Client`](crate::control::Client)）、流连接（[`StreamHandle`](crate::control::StreamHandle)）、
//!   文件传输连接（[`Handle::transfer`](crate::control::Handle::transfer)），GDP 连接
//!   （[`GdpClient`](crate::gdp::GdpClient)）与发现（[`discover`](crate::discovery::discover)）。
//!
//! 两层在同一个 crate 的同一个模块里（例如 GDP 的解码与 GDP 连接），以文件区分。
//!
//! ## 所有权
//!
//! 每种网络资源（socket、读线程）由一个类型拥有，在 `Drop` 中释放；调用顺序用类型与借用表达，
//! 不用运行时状态检查。各类型拥有的资源随实现列在本节。
//!
//! - [`Value`](crate::Value) 是值语义：`Clone` 是深拷贝，修改一份不影响另一份（C++ 的 `GoJson` 赋值后共享存储）。
//!   `Bin` 是 `Bytes`，克隆只增加引用计数；解码得到的 `Bin` 与所在帧共享缓冲区，持有它会让整帧存活。
//! - [`FrameReader`](crate::frame::FrameReader) 为每帧按长度精确分配一块内存，返回的 `Bytes` 独占它；解码出的
//!   零拷贝切片（`Value::Bin`、GDP 的强度、图像像素、Mesh 字节通道与未知类型的消息体）只让自己所在的帧存活。
//!   GDP 的多字节数组（`i16`、`f32` …）解码为自有的 `Vec`，不引用帧。
//! - [`ControlCore`](crate::control::ControlCore) 拥有每个已写出、未应答请求的应答发送端；`register` 建立
//!   `sync_channel(1)`，把接收端（[`ReplyRx`](crate::control::ReplyRx)）交给调用方。调用方超时即丢弃接收端：槽位
//!   成为墓碑，仍按 FIFO 吸收迟到的响应，不从队列中删除（删除会让后面的响应全部错位）。每个槽位只发送一次：
//!   响应到达时，或拆连接时由 `fail_all` 以断开原因完成，所以读线程持锁发送时从不阻塞。
//! - [`SubscriptionExpr`](crate::control::SubscriptionExpr) 只持有编译好的正则，不引用连接。
//! - 有读线程的连接（主连接、流连接、GDP 连接）由句柄与读线程经 `Arc` 共享 socket。owner 的 Drop 只 shutdown，
//!   不 join，所以 Drop 从不阻塞（C++ 先 join 后 shutdown，读线程卡在半帧中时永远挂住）。之后的读写立即失败；读线程
//!   在 Linux 上随即退出，Windows 上 shutdown 唤不醒阻塞的 recv，要等对端关闭连接（控制通道的服务端读到 EOF 即
//!   关闭）或再发数据（GDP 服务端不读，等下一条数据）才退出并释放 socket。读线程退出前记下断开原因，句柄之后的
//!   调用都报这个原因。
//! - [`Client`](crate::control::Client) 是主连接的唯一 owner，不 `Clone`，Drop 即断开。
//!   [`Handle`](crate::control::Handle) 是 owned 句柄（`Arc`），可以移到别的线程、存进别的对象，但不让连接存活：
//!   `Client` Drop 之后经它发出的请求立即以 `Disconnected(Closed)` 失败。
//! - [`Transaction`](crate::control::Transaction) 拥有一个已写出请求的应答接收端，丢弃即 fire-and-forget。
//! - [`Subscription`](crate::control::Subscription) 拥有自己的通知队列与一份租约；读线程只持队列的 `Weak`。
//!   同一表达式的租约计数归零（最后一个订阅 Drop）时写出 `unSub`，不等应答。
//!   [`Listener`](crate::control::Listener) 拥有监听线程的停止开关，线程拥有 `Subscription`。
//! - [`StreamHandle`](crate::control::StreamHandle) 与 [`GdpClient`](crate::gdp::GdpClient) 各拥有一条连接与它的读线程；
//!   文件传输连接只在 `transfer` 调用期间存在。
//!
//! ## 线程与锁
//!
//! - 每个 `Client` 一个读线程 `gopxl-control`，每个流一个 `gopxl-stream`，每个 `GdpClient` 一个 `gopxl-gdp`，每个
//!   `Listener` 一个监听线程 `gopxl-listener`，主连接丢失时一个调用断开回调的线程 `gopxl-disconnect`；请求、文件传输
//!   与发现在调用方线程上执行。
//! - 读线程从不执行用户代码，也从不等待主连接上的消费者：订阅队列满了丢最老的。流与 GDP 的读线程在通道满时阻塞，
//!   背压经 TCP 传到服务端。
//! - 主连接两把锁，顺序固定为写锁 → 分派锁：写锁覆盖登记槽位与整帧写出，槽位顺序因此等于线上顺序；读线程只取
//!   分派锁，写出被阻塞时照常读。临界区内不调用用户代码。
//!
//! ## 阻塞
//!
//! 只提供阻塞 API，不依赖异步运行时：原 SDK 的宿主（Qt、MFC、PLC 网关、Python 扩展）直接调用阻塞接口。
//! 超时：建立控制连接 1 s、GDP 连接 5 s；请求等待应答默认 3 s（[`Transaction::timeout`](crate::control::Transaction::timeout)
//! 可改），从 `response()` 开始计；`sub` 与流的应答 5 s；写出 10 s，超时即拆连接。读不设超时（文件传输连接除外），
//! 空闲连接上的断线由 TCP keepalive（30 s 后每 10 s 探测）发现。
//!
//! ## unsafe
//!
//! `Cargo.toml` 中 `unsafe_code = "forbid"`：协议只涉及字节解析与 socket，安全 Rust 足够，unsafe 只出现在依赖中。
//!
//! ## 错误
//!
//! 错误类型与各变体的语义随实现记录在本节，函数文档不逐个写 `# Errors`。
//!
//! - 值的读取（`pointer`、`Index`、`as_*`）返回 `Option` 或 `&Value::Nil`，不报错；
//! - [`PointerError`](crate::PointerError)：`Value::set` 的指针穿过标量或不存在的数组下标，文档不变；
//! - [`msgpack::EncodeError`](crate::value::msgpack::EncodeError)：长度超过 `u32::MAX`；
//! - [`msgpack::DecodeError`](crate::value::msgpack::DecodeError)：消息体不是一条完整、合法的 MessagePack 文档；
//! - `serde_json::Error`：`str::parse::<Value>()` 的 JSON 文本不合法；
//! - [`FrameError`](crate::frame::FrameError)：读取失败、帧中途 EOF，或长度字段越界；字节流已错位，连接只能关闭。
//!   帧边界上的 EOF 是 `Ok(None)`，非阻塞 socket 的 `WouldBlock` 不丢失已读字节；
//! - [`control::InboundError`](crate::control::InboundError)：服务端从不产生的帧（carrier 头不符、消息体不是
//!   MessagePack 对象、必需字段缺失或类型错误），连接应当关闭；未知的 `type` 不是错误，而是 `Inbound::Other`；
//! - [`gdp::GdpError`](crate::gdp::GdpError)：`Truncated`（数据短于字段声明）或 `Malformed`（长度字段与外层不符）。
//!   信封或 CAS 坏掉时 `decode_message` 返回 `Err`；CAS 之后的部分坏掉时消息仍然返回，`body` 为
//!   `Body::Invalid`，`cas.is_last_msg` 照样能结束数据集；
//! - [`discovery::DiscoveryError`](crate::discovery::DiscoveryError)：丢弃这条数据报、继续接收；`Command` 是其他
//!   客户端（或自己）发出的命令，不是错误报文。Announce 的单个字段缺失或类型不对时取默认值，不报错；
//! - [`control::DisconnectReason`](crate::control::DisconnectReason)：连接必须拆掉，所有挂起的请求以同一个
//!   `Arc` 原因失败。`Closed` 是本端关闭，`Eof` 是服务端关闭，`Read` / `Write` 是 socket 错误（写出超时可能只写了
//!   半帧），`Decode` 是坏帧；`Unsolicited`（挂起队列为空时收到响应）与 `Desync`（响应的 `path` 与队首请求不同）
//!   说明 FIFO 已经或可能错位，拆连接好过把响应交给错误的调用方。流连接与 GDP 连接的断开也用它表示；流连接上
//!   应答之后的孤立响应不是错误，忽略；
//! - [`Error`](crate::Error)：连接层的错误。`Connect`（建立连接失败）、`Timeout`（应答未到，槽位成为墓碑，连接
//!   照常可用）、`Disconnected`（携带上面的原因，只能重连）、`Request`（`status` 不是 OK，携带完整响应；`status`
//!   为 OK 时 `payload.errors` 不升级为错误，与 SDK 一致）、`InvalidArgument`（尚未写出任何字节）；
//! - [`gdp::RecvError`](crate::gdp::RecvError)：`Timeout` 没有收到消息，`Incomplete` 数据集未结束（已收到的保留），
//!   `Disconnected`。信封或 CAS 坏掉的 GDP 消息由读线程丢弃，不报错；
//! - [`discover`](crate::discovery::discover) 只在绑定发现端口或枚举网卡失败时返回 `io::Error`；单个网卡发送失败、
//!   坏报文都只跳过；
//! - [`control::SubExprError`](crate::control::SubExprError)：订阅表达式含正则元字符或 NUL，或生成的正则无法编译（例如
//!   `?`），本地无法与服务端一致地匹配，不发送；
//! - schema：[`schema_for_path`](crate::schema::schema_for_path) 找不到路径时为 `None`，表示“不约束”；
//!   [`validate`](crate::schema::validate) 返回 [`Issue`](crate::schema::Issue) 列表，空表即通过。校验只是建议性的，
//!   服务端才是裁判。
