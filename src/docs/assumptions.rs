//! # 未经真机验证的协议假设
//!
//! 协议事实来自手册（manual）、两个 SDK 的源码（sdk-code）与服务端二进制的静态分析（static-binary），尚未在设备上
//! 确认。与 C++ SDK 行为相同的部分不列；每条给出推翻时的现象与改法。“暴露”是在真机上会因它失败的示例，有设备后运行
//! 全部示例即验证了这些条目；示例覆盖不到的写“手动”检查方法。未注明依据的条目来自 static-binary；依赖这些假设的
//! 代码在注释中引用条目编号。
//!
//! 控制通道：
//!
//! - **C1** 服务端读取一帧时不会停顿 10 s 以上。C++ 不设写超时；这里每 64 KiB 块 10 s 内写不出即拆连接。推翻时去掉写
//!   超时，死连接交给 keepalive 与 TCP 重传。手动：上传期间让服务端暂停 15 s，客户端报 `Write(TimedOut)`。
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
//! - **T3** 流的消费者慢时，服务端在 10 s 写超时后只关闭这一条连接，不阻塞生产者、不影响其他客户端。流连接的读任务
//!   在 4 条缓冲满时停止读取，C++ 从不停止。推翻时流消息改用丢最老的有界队列。手动：开流后 20 s 不取，约 10 s 后应
//!   得到 `Disconnected`，另一个客户端的流不受影响。
//! - **T4** `/dataSources` 流的每条数据是一个 `bin`，内容是与 GDP-TCP 逐字节相同的完整 GDP 消息，表面强度的行宽
//!   规则也相同。C++ 不解码流载荷，[`gdp::decode_all`](crate::gdp::decode_all) 依赖它。推翻时按抓包调整
//!   `decode_all` 的切分与强度行宽。手动：同一数据源经 `gdp::Client` 与流各收一次，比较解码结果。
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
//!   `discover_with_classic` 与 C++ 的结果。
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
