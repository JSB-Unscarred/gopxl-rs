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
//!   读取都经过 `bytes::Buf` 的 `try_get_*`，坏输入只得到 `Err`。`Value` 实现 serde 的 `Serialize` / `Deserialize`，
//!   `&Value` 实现 `Deserializer`：载荷按需反序列化成调用方自己的类型，类型化模型由调用方按自己的固件版本定义；
//!   线上 MessagePack 仍手写，与 C++ SDK 逐字节一致；
//! - 连接：每种连接类型读写自己的 socket，把字节交给编解码层，相当于 FFI 封装中“方法直接调用 `sys`”。
//!   控制通道的主连接（[`Client`](crate::control::Client)）、流连接（[`StreamHandle`](crate::control::StreamHandle)）、
//!   文件传输连接（[`Handle::transfer`](crate::control::Handle::transfer)），GDP 连接
//!   （[`GdpClient`](crate::gdp::GdpClient)）与发现（[`discover`](crate::discovery::discover)）；
//! - 门面：[`System`](crate::System) 对应 C++ 的 `GoSystem`，只包装它已有的几个便捷操作。其余资源与官方示例一样
//!   经原始动词与 `Value` 访问，不建类型化模型：资源由服务端在运行时定义，字段随版本变化；
//! - 资源层：[`Resource`](crate::resource::Resource) 对应 C++ 的 `GoResource`，在原始动词之上缓存读结果、按 schema
//!   校验写入、沿 HAL 链接取子资源，`ResourceApi` 示例经它访问资源。
//!
//! 前两层在同一个 crate 的同一个模块里（例如 GDP 的解码与 GDP 连接），以文件区分。
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
//! - `ControlCore`（crate 内部） 拥有每个已写出、未应答请求的应答发送端；`register` 建立
//!   `sync_channel(1)`，把接收端（`ReplyRx`）交给调用方。调用方超时即丢弃接收端：槽位
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
//! - [`System`](crate::System) 唯一拥有一个 `Client`，Drop 即断开；它的方法只借用这个连接。
//! - [`Resource`](crate::resource::Resource) 由调用方唯一拥有：持有一个 `Handle`（不让连接存活）、自己的数据与 schema
//!   缓存和可选的 `Subscription`，方法取 `&mut self`。没有 C++ 的 `GoResourceManager` 注册表，同一路径的两个
//!   `Resource` 各有各的缓存，`children` 等得到的子资源同样是独立的值。缓存在访问时判断是否过期，不需要线程或回调：
//!   连接的写计数变了（`create` / `update` / `delete` / `call` 在写锁内加一，文件传输在完成后加一；资源记下读取之前的
//!   计数）、连接已断开，或订阅队列中有通知（订阅 `<路径>/*`）。C++ 用回调把管理器中的资源标脏，回调捕获裸 `this`、
//!   写钩子只有一个槽位。
//! - [`Transaction`](crate::control::Transaction) 拥有一个已写出请求的应答接收端，丢弃即 fire-and-forget。
//! - [`Subscription`](crate::control::Subscription) 拥有自己的通知队列；读线程只持队列的 `Weak`。Drop 时注销队列，
//!   没有别的本地订阅用同一表达式时写出 `unSub`，不等应答。
//! - [`StreamHandle`](crate::control::StreamHandle) 与 [`GdpClient`](crate::gdp::GdpClient) 各拥有一条连接与它的读线程；
//!   文件传输连接只在 `transfer` 调用期间存在。
//!
//! ## 线程与锁
//!
//! - 每个 `Client` 一个读线程 `gopxl-control`，每个流一个 `gopxl-stream`，每个 `GdpClient` 一个 `gopxl-gdp`；请求、
//!   文件传输与发现在调用方线程上执行。没有回调：通知由调用方拉取，断开由各阻塞调用以 `Disconnected` 报告。
//! - 读线程从不执行用户代码，也从不等待主连接上的消费者：订阅队列满了丢最老的。流与 GDP 的读线程在通道满时阻塞，
//!   背压经 TCP 传到服务端。
//! - 主连接两把锁，顺序固定为写锁 → 分派锁：写锁覆盖登记槽位与整帧写出，槽位顺序因此等于线上顺序；读线程只取
//!   分派锁，写出被阻塞时照常读。临界区内不调用用户代码。
//!
//! ## 阻塞
//!
//! 只提供阻塞 API，不依赖异步运行时：原 SDK 的宿主（Qt、MFC、PLC 网关、Python 扩展）直接调用阻塞接口。
//! 超时：建立控制连接 1 s、GDP 连接 5 s；请求等待应答默认 3 s（[`Transaction::timeout`](crate::control::Transaction::timeout)
//! 可改），从 `response()` 开始计；`sub` 与流的应答 5 s；`System` 的操作 15 s（同 C++）；写出 10 s，超时即拆连接。读不设超时（文件传输连接除外），
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
//! - `serde::de::value::Error`：`T::deserialize(&value)` 的类型不符、整数越界或结构体缺字段，只带一句说明；
//!   `Resource::get` 把它折进 `Error::Payload` 的 `reason`；
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
//!   半帧），`Decode` 是坏帧。流连接与 GDP 连接的断开也用它表示。挂起队列为空时到达的响应（主连接），以及流连接上应答之后
//!   的孤立响应不属于任何请求，与 C++ 一样丢弃，不是错误；
//! - [`Error`](crate::Error)：连接层与门面的错误。`Connect`（建立连接失败）、`Timeout`（应答未到，槽位成为墓碑，
//!   连接照常可用）、`Disconnected`（携带上面的原因，只能重连）、`Request`（`status` 不是 OK，携带完整响应；`status`
//!   为 OK 时 `payload.errors` 不升级为错误，与 SDK 一致）、`Payload`（`System`、`Resource::get` 或
//!   `create_child` 要读取的字段缺失或无法反序列化，`reason` 是 `"missing"` 或 serde 的说明）、`InvalidArgument`
//!   （尚未写出任何字节）、`Validation`（资源开启了
//!   校验，补丁中的值不符合 schema，请求没有发出）；
//! - [`gdp::RecvError`](crate::gdp::RecvError)：`Timeout` 没有收到消息，`Incomplete` 数据集未结束（已收到的保留），
//!   `Disconnected`。信封或 CAS 坏掉的 GDP 消息由读线程丢弃，不报错；
//! - [`discover`](crate::discovery::discover) 只在绑定发现端口或枚举网卡失败时返回 `io::Error`；单个网卡发送失败、
//!   坏报文都只跳过；
//! - [`control::SubExprError`](crate::control::SubExprError)：订阅表达式含正则元字符或 NUL，或生成的正则无法编译（例如
//!   `?`），本地无法与服务端一致地匹配，不发送；
//! - schema：[`schema_for_path`](crate::schema::schema_for_path) 找不到路径时为 `None`，表示“不约束”；
//!   [`validate`](crate::schema::validate) 返回被违反的关键字与界限的列表，空表即通过。校验只是建议性的，
//!   服务端才是裁判。
//!
//! ## 未经真机验证的协议假设
//!
//! 协议事实来自手册（manual）、两个 SDK 的源码（sdk-code）与服务端二进制的静态分析（static-binary），尚未在设备上
//! 确认。C++ SDK 已用于生产，与它行为相同的部分不列；这里只列本 crate 与 C++ 不同的假设，每条给出推翻时的现象与
//! 改法。“暴露”是在真机上会因它失败的示例，有设备后运行全部示例即验证了这些条目；示例覆盖不到的写“手动”检查方法。
//! 未注明依据的条目来自 static-binary；依赖这些假设的代码在注释中引用条目编号。
//!
//! 控制通道：
//!
//! - **C1** 服务端读取一帧时不会停顿 10 s 以上。C++ 不设写超时；这里 10 s 内写不出字节即拆连接，阻塞的写出（包括
//!   `Subscription` Drop 时的 `unSub`）因此有上限。推翻时去掉写超时，死连接交给 keepalive 与 TCP 重传。手动：上传
//!   期间让服务端暂停 15 s，Linux 客户端报 `Write(TimedOut)`（Windows 回环上整帧一次交给内核，不触发）。
//! - **C2** 服务端读到 FIN 后随即关闭连接。Windows 上读线程靠它退出（见“所有权”）；C++ 先 join 再关闭 socket。
//!   推翻时读线程改为带超时地等待可读。手动：循环 connect 与 drop，观察线程数是否增长。
//!
//! 订阅：
//!
//! - **S1** 同一表达式重复 `sub` 无害，服务端删掉同文本的旧条目再追加。C++ 对同一路径只发一次；这里每个
//!   `subscribe` 都发。推翻时只在该表达式没有别的本地订阅时才发。手动：对 `/system` 订阅两次，两个都应收到通知，
//!   `listSub` 只列一条。
//! - **S2** 服务端对 `created` / `updated` / `deleted` 只按 R 正则投递，对 `embeddedUpdated` 只按 E。这里按
//!   同一规则把通知分给订阅；C++ 另有接收全部通知的全局回调，并按祖先回溯分派。推翻时按观察到的规则修改
//!   `SubscriptionExpr::delivers`。手动：`subscribe("/tools")` 后新建 tool，线上出现 `created /tools/<id>` 而订阅
//!   取不到即推翻。
//! - **S3** `sub <路径>/*` 的 `*` 是通配，投递路径本身与全部后代；`unSub` 只删除文本相同的那一条。
//!   `Resource::subscribe` 依赖它，C++ 订阅精确路径。推翻时改为订阅精确路径。手动：一条连接订阅资源，另一条连接
//!   修改它，前者的 `has_remote_changes()` 应变为 `true`。
//!
//! 流与文件传输：
//!
//! - **T1** 同一客户端可以同时保持主连接、每个流一条连接与文件传输的临时连接。C++ 全部走一条连接。推翻时
//!   多个流共用一条连接，文件传输改走主连接。暴露：`receive_metrics`（3 条流连接）、`backup_restore`、`save_job`、
//!   `replay_data`、`system_upgrade` 报 `Connect` 或 `Disconnected`。
//! - **T2** 发出请求的连接关闭后，服务端仍把已开始的命令执行完。文件传输读到应答或超时即关闭临时连接，
//!   `system_upgrade` 之后靠主连接的通知等待升级完成；C++ 超时后连接仍在，升级在同一连接上等待。推翻时传输连接改为
//!   读到应答或 EOF 才关闭。暴露：`system_upgrade` 报升级在 5 分钟内未完成；超时的恢复或上传只生效一部分。
//! - **T3** 流的消费者慢时，服务端在 10 s 写超时后只关闭这一条连接，不阻塞生产者、不影响其他客户端。流连接的读线程
//!   在 4 条缓冲满时停止读取，C++ 从不停止。推翻时流消息改用丢最老的有界队列。手动：开流后 20 s 不取，约 10 s 后应
//!   得到 `Disconnected`，另一个客户端的流不受影响。
//! - **T4** `/dataSources` 流的每条数据是一个 `bin`，内容是与 GDP-TCP 逐字节相同的完整 GDP 消息，表面强度的行宽
//!   规则也相同。C++ 不解码流载荷，[`gdp::decode_all`](crate::gdp::decode_all) 依赖它。推翻时按抓包调整
//!   `decode_all` 的切分与强度行宽。手动：同一数据源经 `GdpClient` 与流各收一次，比较解码结果。
//!
//! GDP：
//!
//! - **G1**（manual）1.5 的 Label 段在位置之后带 `i32` 对齐与 `u32` 颜色。C++ 跳过这些字节；这里读出为
//!   `Label::alignment`、`color`。推翻时删掉这两个字段或按抓包修正。手动：检查标签段长，以及颜色是否与 GoPxL 界面
//!   一致。
//! - **G2** 单条 GDP 消息不超过 1 GiB（32 位平台 256 MiB）：服务端每连接队列 200 MB，部分平台 2 GB。C++ 不设上限；
//!   这里 GDP 连接与流连接超出即报 `FrameError::Length` 并断开。推翻时放宽各连接共用的 `MAX_FRAME`。暴露：
//!   `receive_surface` 接收超大表面时 `receive` 返回 `Disconnected`。
//!
//! 发现：
//!
//! - **D1**（sdk-code）Gocator 6.x 经典设备把回复发往 UDP 3220，而非命令的源端口：两个 SDK 的发送 socket 都绑定
//!   3220，这里的绑定临时端口。推翻时经典命令改从绑定 3220 的 socket 发出。手动：对 6.x 设备比较
//!   `discover(_, true)` 与 C++ 的结果。
//! - **D2** 本机 GoPxL 能收到经物理网卡发出的受限广播，不需要从回环接口再发。两个 SDK 也从 127.0.0.1
//!   发送。推翻时不再跳过回环接口（承担锁定网卡的风险）。暴露：在运行虚拟节点的 PC 上 `discover` 列不出
//!   本机实例。
//!
//! 资源：
//!
//! - **R1** `expandLevel` 不为 0 时，`_embedded` 的每个条目都是完整的读结果，下一层的层级为 −1 → −1、n → n − 1。
//!   `Resource::children` 把条目直接放进子资源的缓存；C++ 按键数判断条目是否已展开。推翻时只缓存含 `_links` 以外键
//!   的条目。暴露：`resource_commands` 的作业文件一节报 `response payload has no valid "/jobName"`。
//! - **R2**（manual）`create` 的响应带新资源的 `path` 或 `_links.self.href`，形如 `/tools/<id>`，末段即数据源 ID 中
//!   的工具 ID。`Resource::create_child` 与 `resource_configure_tool` 依赖它；C++ 示例写死数据源 ID。推翻时
//!   按响应形态修改 `create_child`，示例改读输出的 `dataSourceId`。暴露：`resource_configure_tool` 报 `Payload`，或
//!   之后收不到测量值。
