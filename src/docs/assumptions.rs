//! # 未经真机验证的协议假设
//!
//! 下列协议行为来自手册与对服务端的分析，尚未在设备上确认。每条写明依赖它的代码、推翻时的改法与验证方法；
//! 代码注释用编号引用这些条目。“示例”是假设不成立时会失败的示例，有设备后运行全部示例即验证了这些条目；示例覆盖
//! 不到的写“手动”检查方法。
//!
//! 控制通道：
//!
//! - **C1** 服务端读取一帧时不会停顿 10 s 以上。依赖它的是写超时：每 64 KiB 块 10 s 内写不出即断开连接。推翻时
//!   去掉写超时，死连接交给 keepalive 与 TCP 重传。手动：上传期间让服务端暂停 15 s，客户端报 `Write(TimedOut)`。
//!
//! 订阅：
//!
//! - **S1** 同一表达式重复 `sub` 无害，服务端删掉同文本的旧条目再追加。依赖它的是每个 `subscribe` 都发 `sub`。
//!   推翻时只在该表达式是第一个本地订阅时才发。手动：对 `/system` 订阅两次，两个都应收到通知，`listSub` 只列一条。
//! - **S2** 服务端只按 [`SubscriptionExpr`](crate::control::SubscriptionExpr) 文档中的两条正则投递通知：
//!   `created` / `updated` / `deleted` 用 R，`embeddedUpdated` 用 E。`SubscriptionExpr::delivers` 按同一规则把通知
//!   分给本地订阅。推翻时按观察到的规则修改 `delivers`。手动：`subscribe("/tools")` 后新建 tool，线上出现
//!   `created /tools/<id>` 而订阅取不到即推翻。
//! - **S3** `sub <路径>/*` 的 `*` 是通配，投递路径本身与全部后代；`unSub` 只删除文本相同的那一条。依赖它的是
//!   `Resource::subscribe`。推翻时改为订阅精确路径。手动：一条连接订阅资源，另一条连接修改它，前者的
//!   `has_remote_changes()` 应变为 `true`。
//!
//! 流与文件传输：
//!
//! - **T1** 同一客户端可以同时持有主连接、每个流一条连接与文件传输的临时连接。依赖它的是 `open_stream` 与
//!   `transfer` 各开一条连接。推翻时多个流共用一条连接，文件传输改走主连接。示例：`receive_metrics`（3 条流连接）、
//!   `backup_restore`、`save_job`、`replay_data`、`system_upgrade` 报 `Connect` 或 `Disconnected`。
//! - **T2** 发出请求的连接关闭后，服务端仍把已开始的命令执行完。依赖它的是文件传输读到应答或超时即关闭临时连接，
//!   `system_upgrade` 之后靠主连接的通知等待升级完成。推翻时传输连接改为读到应答或 EOF 才关闭。示例：
//!   `system_upgrade` 报升级在 5 分钟内未完成；超时的恢复或上传只生效一部分。
//! - **T3** 流的消费者慢时，服务端在 10 s 写超时后只关闭这一条连接，不阻塞生产者、不影响其他客户端。依赖它的是流
//!   连接的读任务在 4 条缓冲满时暂停读取。推翻时流消息改用丢最老的有界队列。手动：开流后 20 s 不取，约 10 s 后应
//!   得到 `Disconnected`，另一个客户端的流不受影响。
//! - **T4** `/dataSources` 流的每条数据是一个 `bin`，内容是与 GDP 连接上逐字节相同的完整 GDP 消息，表面强度的行宽
//!   规则也相同。依赖它的是 [`gdp::decode_all`](crate::gdp::decode_all)。推翻时按抓包调整 `decode_all` 的切分与
//!   强度行宽。手动：同一数据源经 `gdp::Client` 与流各收一次，比较解码结果。
//!
//! GDP：
//!
//! - **G1** GoPxL 1.5 的 Label 段在位置之后带 `i32` 对齐与 `u32` 颜色（手册）。依赖它的是 `Label::alignment`、
//!   `color`。推翻时删掉这两个字段或按抓包修正。手动：检查标签段长，以及颜色是否与 GoPxL 界面一致。
//! - **G2** 单条 GDP 消息不超过 1 GiB（32 位平台 256 MiB）。依赖它的是各连接共用的帧长度上限：超出即报
//!   `FrameError::Length` 并断开。推翻时放宽上限。示例：`receive_surface` 接收超大表面时 `receive` 返回
//!   `Disconnected`。
//!
//! 发现：
//!
//! - **D1** Gocator 6.x 经典设备把回复发往 UDP 3220，而非命令的源端口。依赖它的是经典命令从临时端口发出、在 3220
//!   接收。推翻时经典命令改从绑定 3220 的 socket 发出。手动：对 6.x 设备运行 `discover_with_classic`，应列出它。
//! - **D2** 本机 GoPxL 能收到经物理网卡发出的受限广播，不需要从回环接口再发。依赖它的是发现跳过回环接口。推翻时
//!   加上回环接口。示例：在运行虚拟节点的 PC 上 `discover` 列不出本机实例。
//!
//! 资源：
//!
//! - **R1** `expandLevel` 不为 0 时，`_embedded` 的每个条目都是完整的读结果，下一层的层级为 −1 → −1、n → n − 1。
//!   依赖它的是 `Resource::children` 把条目直接放进子资源的缓存。推翻时只缓存含 `_links` 以外键的条目。示例：
//!   `resource_commands` 的作业文件一节报 `response payload has no valid "/jobName"`。
//! - **R2** `create` 的响应带新资源的 `path` 或 `_links.self.href`，形如 `/tools/<id>`，末段即数据源 ID 中的工具 ID
//!   （手册）。依赖它的是 `Resource::create_child`、`configure_tool` 与 `resource_configure_tool`。推翻时按响应形态
//!   修改 `create_child`，示例改读输出的 `dataSourceId`。示例：`resource_configure_tool` 报 `Payload`，或之后收不到
//!   测量值。
