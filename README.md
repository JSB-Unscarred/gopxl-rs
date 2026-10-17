# gopxl

[English](README.en.md)

LMI GoPxL（Gocator 3D 传感器平台）客户端 SDK 的 Rust 实现（非官方），替代官方的 C++ GoPxLSdk 与
.NET GoPxLSdkNet。本 crate 直接实现线上协议，不链接厂商库。

## 版本

| 项 | 版本 | 说明 |
| --- | --- | --- |
| 目标服务端 | GoPxL 1.5 | 传感器固件或 GoPxL PC 应用 |
| 协议 | REST API 9.0.0 | 控制通道的协议版本 |
| 被替代的官方 SDK | 8.5.62.52 | C++ GoPxLSdk 与 .NET GoPxLSdkNet 随 GoPxL 1.5 发布，这是 SDK 自身的版本号 |

## 环境

- Rust 1.87 或更新版本；不依赖 C 库，构建不需要安装 GoPxL SDK。
- 目标平台与 C++ SDK 相同：`x86_64-pc-windows-msvc`、`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、
  `armv7-unknown-linux-gnueabihf`；目前只在 `x86_64-pc-windows-msvc` 上构建验证。

## 示例

`examples/` 是官方 C++ 示例的移植，请求、顺序与超时与 C++ 相同；移植时修掉的示例缺陷写在各示例的文档开头。
设备地址取自环境变量 `GOPXL_IP`（`IP` 或 `IP:端口`，端口默认 3600）；未设置时先发现，使用唯一一台可连接
（未被远程控制）的实例，没有或有多台时报错：

```bash
GOPXL_IP=192.168.1.10 cargo run --example receive_measurement
```

| C++ 示例 | Rust 示例 |
| --- | --- |
| `Discover` | `discover` |
| `ConfigureSensor`、`ConfigureTool`、`AlignSensor`、`MultiSensorLayout` | `configure_sensor`、`configure_tool`、`align_sensor`、`multi_sensor_layout` |
| `ReceiveMeasurement`、`ReceiveString`、`ReceiveAsync` | `receive_measurement`、`receive_string`、`receive_async` |
| `ReceiveProfile`、`ReceiveSurface`、`MultilayerOutputs` | `receive_profile`、`receive_surface`、`multilayer_outputs` |
| `ReceiveImage`、`Receive2dImage`、`Acquire2dImage` | `receive_image`、`receive_2d_image`、`acquire_2d_image` |
| `ReceiveMetrics` | `receive_metrics` |
| `SaveJob`、`BackupRestore`、`ReplayData`、`SystemUpgrade` | `save_job`、`backup_restore`、`replay_data`、`system_upgrade` |
| `ResourceCommands`、`ResourceConfigureSensor`、`ResourceConfigureTool`、`ResourceSchema`、`ResourceSubscriptions`（`ResourceApi`） | `resource_commands`、`resource_configure_sensor`、`resource_configure_tool`、`resource_schema`、`resource_subscriptions` |

## 已实现的接口

以官方 C++ SDK（8.5.62.52）的公共头文件为准；.NET 的同名接口归入同一行，行为差异写在说明列，.NET 独有的接口单独
成行。各层实现时补充。

| SDK 接口 | Rust 接口 | 说明 |
| --- | --- | --- |
| `GoJson::GoJson()` | `Value::object() -> Value` | `Value::default()` 是 `Nil` |
| `GoJson::GoJson(const std::string&)`、`GoJson::ParseString` | `str::parse::<Value>()`（`FromStr`，`Err = serde_json::Error`） | 解析 JSON 文本；浮点数正确舍入，与 GoJson 逐位相同；整数文本 `-0` 为 `Float(-0.0)`（GoJson 为整数 0） |
| `GoJson::ToString` | `impl Display for Value` | 紧凑 JSON，`{:#}` 为缩进格式；`Bin` 输出为 `"<bin N bytes>"` |
| `GoJson::ToBinary` | `value::msgpack::encode(&Value, &mut Vec<u8>) -> Result<(), EncodeError>` | 与 nlohmann 3.8.0 逐字节一致；长度超过 `u32::MAX` 时报错 |
| `GoJson::ParseMsgPack` | `value::msgpack::decode(&Bytes) -> Result<Value, DecodeError>` | `Bin` 是输入的零拷贝切片；嵌套上限 128 层 |
| `GoJson::Get<T>`、`At` | `Value::pointer(&self, &str) -> Option<&Value>`；`Index<&str>`、`Index<usize>`（缺失时为 `&Value::Nil`）；`as_bool`、`as_i64`、`as_u64`、`as_f64`、`as_str`、`as_array`、`as_map` | 不修改文档；整数访问器不截断浮点数，`as_u64` 不回绕负数 |
| `GoJson::GetBinary` | `Value::as_bytes(&self) -> Option<&Bytes>` | 不复制 |
| `GoJson::Set`、`Replace` | `Value::set(&mut self, &str, impl Into<Value>) -> Result<(), PointerError>` | 只自动创建对象；数组用已有下标或 `-` 追加；出错时文档不变 |
| `GoJson::Remove` | `Value::remove(&mut self, &str) -> Option<Value>` | 数组元素前移，不留 null |
| `GoJson::Merge` | `Value::merge_patch(&mut self, &Value)` | RFC 7396 |
| `GoJson::Contains`、`HasKey`、`Is*`、`Size`、`Empty`、`Begin`、`End`、`Find` | `pointer(..).is_some()`、`is_nil()`、`matches!`、`as_array()` / `as_map()` 的长度与迭代器 |  |
| `GoJson::Add`、`Copy`、`Move`、`Patch`、`Flatten`、`Unflatten`、`GetHandle` | 不提供 | SDK 与示例都不调用；`Add`、`Copy`、`Move` 基于有损的 flatten，用 `set` / `remove` 组合即可；`GetHandle` 暴露 nlohmann 对象 |
| `GoRequest::GoRequest(method, uri, content, args)` | `control::Request::new(Method, impl Into<String>) -> Request`，再设置 `payload`、`args` 字段 | `payload`、`args` 默认为 `{}`（线上 `0x80`）；路径原样发送 |
| `GoRequest::Method`、`Uri`、`Content`、`Arguments` | `Request` 的 `method`、`path`、`payload`、`args` 字段 |  |
| `GoRequest::ToByteArray` | `Request::encode(&self, &mut Vec<u8>) -> Result<(), EncodeError>` | 写出含 carrier 头的整帧，与 C++ SDK 逐字节一致；长度超过 `u32::MAX` 时报错，不写出任何字节 |
| `GoRequest::Id` | 不提供 | 只是本地计数，线上没有请求 id |
| `GoRequestMethod::ToString` | `Method::as_str` | 另有 `ListSub`、`ClearSub`、`ListStream`；.NET 的全小写拼写（`unsub`、`cancelstream`）被服务端拒绝，这里用精确拼写 |
| `GoResponse::Type`、`GoResponseType::FromString`；`GoCtrlChannelV1` 的读帧 | `control::Inbound::decode(Bytes) -> Result<Inbound, InboundError>` | 未知 `type` 为 `Inbound::Other`（C++ 当作响应）；服务端从不产生的帧报错（C++ 丢弃后 FIFO 错位） |
| `GoResponse::Status`、`Path`、`Payload`（`GoRequestResponse`） | `control::Response` 的 `status`、`path`、`payload` 字段 | 缺少 `status` 是坏帧（C++ 当作成功）；`path` 缺失为空串，`payload` 缺失为 `Nil` |
| `GoNotificationResponse::NotificationType`；`GoNotificationType::FromString` | `control::Notification` 的 `event` 字段（`EventType`） | 未知字符串为 `EventType::Other`（C++ 当作 `Updated`） |
| `GoStreamResponse::StreamIdentifier`、`StreamStatus` | `control::StreamMessage` 的 `stream_id`、`state`（`StreamState`）字段 |  |
| `GoResponse::Raw` | 不提供 | 类型化的字段已包含全部内容 |
| `kStatus_Name` | `control::Status::name`、`is_ok`；`impl Display for Status` | 返回手册中的名字（`"notFound"`） |
| `GoUri::UrlEncode`、`UrlDecode` | `control::url_encode(&str) -> String`、`control::url_decode(&str) -> Vec<u8>` | 不提供 `spaceAsPlus`；解码不越界读取，结果可能不是 UTF-8，所以返回字节 |
| `GoRestClient` 的事务队列（读线程按发送顺序完成 `GoTransaction`） | `control::ControlCore`：`register(&Request) -> ReplyRx`、`on_inbound(Inbound) -> Result<Option<Notification>, DisconnectReason>`、`fail_all(&Arc<DisconnectReason>)` | 纯状态机，不做 I/O，连接层在锁外解码后交给它；写出前登记；超时的请求留下墓碑吸收迟到的响应；响应带 `path` 且队首不是 `create` 时按段比较，不一致时报错，由连接层拆连接（C++ 错位后继续）；挂起队列为空时的响应丢弃（同 C++） |
| `GoRestClient::Connect` | `control::Client::connect(SocketAddr) -> Result<Client>` | 超时 1 s；设置 `TCP_NODELAY`、keepalive（空闲 30 s 后每 10 s 探测）与 10 s 写超时；端口 0 报 `InvalidArgument`（C++ 直接交给 socket） |
| `GoRestClient::Disconnect` | `drop(client)` | 挂起的请求以 `Disconnected(Closed)` 失败；只 shutdown、不 join 读线程，从不阻塞（C++ 先 join 后 shutdown，可能挂住） |
| `GoRestClient::IsConnected` | `control::Handle::is_connected() -> bool` | 拆连接后为 `false`（C++ 连接丢失后仍为真）；`Client` 经 `Deref` 调用 `Handle` 的方法，`Client::handle()` 取得可移到别的线程的句柄 |
| `GoRestClient::Read`、`Update`、`Create`、`Delete`、`Call` | `Handle::read(&str)`、`update(&str, impl Into<Value>)`、`create(&str, impl Into<Value>)`、`delete(&str)`、`call(&str, impl Into<Value>)`，均返回 `Transaction` | 第二个参数即 C++ 的 `content`（线上的 `payload`）；`args` 经 `Handle::send` 设置；在写锁内先登记再写出（C++ 先入队后发送，发送失败留下孤儿事务）；写出失败或超时即拆连接 |
| — | `Handle::send(Request) -> Transaction` | 任意方法与 `args`，`listSub`、`clearSub`、`listStream` 也经它发送（两个 SDK 都没有）；`stream`、`cancelStream` 报 `InvalidArgument`，流见 `open_stream` |
| `GoTransaction::GetResponse`、`CheckResponse` | `Transaction::timeout(Duration) -> Transaction`、`response() -> Result<Response>` | 默认 3 s，从 `response()` 开始计；`status` 不是 OK 时为 `Error::Request`；超时为 `Error::Timeout`，槽位成为墓碑，连接照常可用；丢弃 `Transaction` 即 fire-and-forget |
| `GoTransaction::GetRequest`、`GetResponseFuture` | 不提供 | 请求在调用方手里；方法与路径出现在错误里 |
| `GoRestClient::Sub`、`UnSub`、`SetSubHandler` | `Handle::subscribe(&str) -> Result<Subscription>`；`Subscription::recv_timeout(Duration) -> Result<Option<Notice>>`、`iter()`；drop 即退订 | 拉取式：每个订阅一个队列（1024 条，满了丢最老的并产出 `Notice::Lagged`，读线程从不阻塞）；只收到服务端按该表达式会投递的通知（C++ 的全局处理器收到全部通知）；同一表达式的订阅共享服务端订阅，最后一个 drop 时写出 `unSub`，不等应答；`sub` 失败或超时时退回租约（超时后才执行的 `sub` 由随后的 `unSub` 撤销） |
| `GoRestClient::AddNotificationListener`、`RemoveNotificationListener`、`ClearAllListeners` | `Handle::add_listener(&str, impl FnMut(Notice)) -> Result<Listener>`；drop `Listener` | 每个监听器一个线程，按到达顺序回调（C++ 每条通知起一个线程、乱序；回调在锁内执行，重入即死锁）；`sub` 失败时不留下监听器；移除时丢弃未处理的通知 |
| `GoRestClient::ReplaceListenerCallback` | 不提供 | drop 旧的 `Listener` 再 `add_listener` |
| `GoRestClient::StartStream`、`StopStream`、`SetStreamHandler` | `Handle::open_stream(&str, impl Into<Value>) -> Result<StreamHandle>`；`StreamHandle::recv_timeout(Duration) -> Result<Option<StreamMessage>>`；drop 即停止 | 每个流一条连接，等待应答 5 s，`status` 不是 OK 时为 `Error::Request`；应答前到达的数据排在最前；结束消息（`Complete` / `Abort`）照常取出；应答之后的孤立响应忽略（C++ 当作下一个请求的应答）；关闭连接即停止，从不发 `cancelStream`；本地缓冲 4 条，消费者跟不上时背压到服务端 |
| `GoRestClient::SetDisconnectHandler`、`SetReadErrorHandler` | `Handle::on_disconnect(impl FnOnce(&DisconnectReason))` | 连接丢失时在一个新线程上调用一次并给出原因，回调里可以发请求、重连；`Client` 的 drop 不触发；已断开时立即调用；只在真正拆连接时触发（C++ 一个坏消息就触发） |
| — | `Handle::transfer(Request, Duration) -> Result<Response>` | 文件传输（归档、恢复、升级、作业与回放数据）用的临时连接，不阻塞主连接；超时是连续没有收到字节的时长；下载的 `Bin` 引用响应帧 |
| `GoSystem::GoSystem(address, port)`、`SetAddress`、`SetControlPort`、`Connect` | `System::connect(SocketAddr) -> Result<System>` | 存在即已连接，没有“未连接”的状态；端口 0 报 `InvalidArgument` |
| `GoSystem::Disconnect`、`~GoSystem` | `drop(system)` | 同 `GoRestClient::Disconnect` |
| `GoSystem::Client` | `System::client() -> &Client` | `IsConnected`、`SetDisconnectHandler` 经它调用 `is_connected`、`on_disconnect` |
| `GoSystem::Address`、`ControlPort` | `Handle::addr() -> SocketAddr` |  |
| `GoSystem::Start`、`Stop` | `System::start()`、`stop() -> Result<()>` | `call /system/commands/start` / `stop`，超时 15 s |
| `GoSystem::RunningState` | `System::running_state() -> Result<RunState>` | 读 `/system` 的 `runState`，超时 15 s；表外的值为 `RunState::Other`（C++ 直接强制转换） |
| `GoSystem::EnableQuickEdit`、`DisableQuickEdit`、`QuickEditEnabled` | `System::set_quick_edit(bool) -> Result<()>`、`quick_edit_enabled() -> Result<bool>` | `update` / `read /system` 的 `quickEditEnabled`，超时 15 s |
| `GoSystem::GdpPort` | `System::gdp_port() -> Result<u16>` | 读 `/controls/gocator` 的 `serverPort`，超时 15 s；不在 `u16` 内时报 `Error::Payload`（C++ 截断） |
| `GoSystem::SensorPath` | `System::sensor_path(&str) -> Result<Option<String>>` | 与 C++ 一样逐层读取引擎、扫描器、传感器并比较字符串 `serialNumber`；找不到时为 `None`（C++ 返回 `""`） |
| `ApiVersion`、`ReleaseVersion` | `API_VERSION`（`"9.0.0"`）、`RELEASE_VERSION`（`"1.5"`） |  |
| `BuildVersion`、`BuildVersionNumber` | 不提供 | crate 版本由 Cargo 给出 |
| `GoGdpClient::Connect`、`Close` | `gdp::GdpClient::connect(SocketAddr) -> Result<GdpClient>`；drop | 超时 5 s；读线程逐条解码；从不写 socket |
| `GoGdpClient::ReceiveDataSync`、`DataSet`、`ReceiveDataAsync` | `GdpClient::receive(&mut self, Duration) -> Result<Vec<Message>, RecvError>` | 按 `cas.is_last_msg` 组集；单一截止时间（C++ 读到一半时不限时）；区分 `Timeout` 与 `Incomplete`，已收到的消息保留到下一次；异步接收即在自己的线程里循环；本地只缓冲 4 条，不读时背压到服务端（C++ 的异步队列无界）；信封或 CAS 坏掉的消息丢弃 |
| `GoGdpClient::ClearData` | `GdpClient::clear_data(&mut self) -> Result<()>` | 重连，丢弃积压的数据与未完成的数据集 |
| `GoGdpClient::IsConnected`、`IpAddress`、`Port` | 不提供 | 断开后 `receive` 返回 `RecvError::Disconnected`；地址由调用方给出 |
| `GoDiscoveryClient::BlockingDiscover`、`InstanceList` | `discovery::discover(Duration, bool) -> io::Result<Vec<Instance>>` | 每个启用的 IPv4 网卡（含 169.254.x.x）各发一次，单个网卡失败不影响其他网卡；0、300、1000 ms 重发（C++ 只发一次）；按（地址，Web 端口）去重；GoPxL 实例在前；每次调用都是全新的状态（C++ 的经典列表从不清空） |
| `GoDiscoveryClient::GoPxLInstanceList`、`ClassicInstanceList`、`Instance`、`GoPxLInstance`、`ClassicInstance` | 按 `Instance::classic`、`address`、`web_port`、`serial_number` 筛选 `discover` 的结果 |  |
| `GoRestClient::AddNotificationListener` 的路径匹配 | `control::SubscriptionExpr::new(&str) -> Result<SubscriptionExpr, SubExprError>`、`delivers(&EventType, &str) -> bool`、`text()` | 按服务端的规则：`*` 跨越 `/`，`/a/*` 也匹配 `/a`，`embeddedUpdated` 匹配祖先（C++ 按前缀与祖先回溯，与服务端不一致）；与服务端一样按字节匹配，`.` 不匹配 `\r`、`\n`；拒绝正则元字符 `\| ( ) [ ] { } ^ \` 与 NUL（服务端在 NUL 处截断表达式），生成的正则无法编译时同样报错 |
| `GoSchemaValidator::SchemaForPath` | `schema::schema_for_path(&Value, &str) -> Option<&Value>` | 路径不在 schema 中时为 `None`（C++ 抛 `GoResourceError`）；路径与 `Value::pointer` 同一语法（`/` 是键 `""`，C++ 把它当作根）；`type` 为可空数组时同样导航 |
| `GoSchemaValidator::Validate` | `schema::validate(&Value, &Value) -> Vec<Issue>` | 空表即通过；`binary` 要求 `Bin`，支持可空类型数组，`multipleOf` 对负数不误报，`enum` 的整数精确比较，空 `enum` 不约束，长度按字符计；`type: "null"` 只接受 `Nil`（C++ 不认识它，一律通过） |
| `GoSystem::Resource` | `System::resource(impl Into<String>) -> resource::Resource` | 开启写入前的校验（与 `GoSystem` 的资源管理器相同）；每次调用得到一个独立的 `Resource`，各有各的缓存 |
| `GoResource::GoResource(client, uri)` | `Resource::new(Handle, impl Into<String>) -> Resource` | 不发请求；超时 5 s、展开层级 0、不校验 |
| `GoResource` 的拷贝构造与赋值 | 不提供 | 缓存与订阅只有一个 owner；需要第二个时 `Resource::new(r.handle().clone(), r.path())` |
| `GoResource::Uri` | `Resource::path() -> &str` |  |
| `GoResource::SetTimeout`、`Timeout` | `Resource::set_timeout(Duration)`、`timeout()` | 默认 5 s |
| `GoResource::SetExpandLevel`、`ExpandLevel` | `Resource::set_expand_level(i32)`、`expand_level()` | 设置时数据缓存失效，缓存总是按当前层级读到的 |
| `GoResource::Data` | `Resource::data() -> Result<&Value>` | 缓存为空或过期时读服务端，读参数 `{expandLevel, includeSchema: false}`；同一连接上发出任一 `create` / `update` / `delete` / `call`（含文件传输）之后、订阅收到通知之后或连接断开之后，缓存过期（取代 C++ 的 `SetNonIdempotentRequestHandler` 与 `ResetConnectionState`）；C++ 的 `Cache`（总是重读）即 `invalidate()` 之后 `data()` |
| `GoResource::GetBool`、`GetInt`、`GetInt64`、`GetFloat`、`GetDouble`、`GetString`、`GetObject`、`GetProp` | `Resource::get(&str, impl FnOnce(&Value) -> Option<T>) -> Result<T>`，例如 `get(ptr, Value::as_i64)`、`get(ptr, Some)` | 字段缺失或类型不对时为 `Error::Payload`（C++ 抛 `GoResourceError`）；整数不截断浮点数 |
| `GoResource::InvalidateCache` | `Resource::invalidate()` | 丢弃数据与 schema 缓存 |
| `GoResource::HasRemoteChanges` | `Resource::has_remote_changes() -> bool` | 下一次 `data` 是否读服务端；从未读取时也为 `true` |
| `GoResource::SetBool`、`SetInt`、`SetInt64`、`SetFloat`、`SetDouble`、`SetString`、`SetProp` | `Resource::set(&str, impl Into<Value>) -> Result<()>` | 只含这一个值的 `update`，立即发送 |
| `GoResource::SetJson`、`Update(payload)`；`ScopedUpdate`、`BeginUpdate`、`EndUpdate`、`Flush`、`GoUpdateScope` | `Resource::update(impl Into<Value>) -> Result<()>` | 多个修改用 `Value::set` 写进同一个补丁，一次发送；开启校验时补丁中对象以外的每个值按所在位置校验（与 `Set*` 相同；C++ 的 `SetJson` 不校验），判为不合格时先重读一次 schema（schema 随设备状态变化，C++ 只用缓存的 schema） |
| `GoResource::CancelUpdate`、`IsUpdateCancelled`、`IsUpdateDeferred`、`DiscardPending`；`GoUpdateScope::Cancel` | 不提供 | 补丁在调用方手里，不发送即取消；没有共享的待发补丁（C++ 的 `Flush` 失败时丢失补丁，作用域的异常路径清掉外层作用域的改动） |
| `GoResource::Schema` | `Resource::schema() -> Result<&Value>` | 读参数 `{includeSchema: true}`，只更新 schema 缓存（C++ 用不展开的数据覆盖已展开的数据缓存）；没有 `_schema` 时为 `Nil` |
| `GoResource::SchemaFor` | `schema::schema_for_path(resource.schema()?, &str)` | 不在 schema 中时为 `None`（C++ 抛 `GoResourceError`） |
| `GoResource::Validate` | `Resource::validate(&str, &Value) -> Result<Vec<Issue>>` | 空表即通过；不在 schema 中的位置不约束 |
| `GoResource::EnableValidation`、`IsValidationEnabled` | `Resource::set_validation(bool)`、`validation_enabled()` | `System::resource` 得到的资源默认开启 |
| `GoResource::Subscribe`、`Unsubscribe`、`IsSubscribed` | `Resource::subscribe() -> Result<()>`、`unsubscribe()`、`is_subscribed()` | 订阅 `<路径>/*`，收得到子项的 `created` / `deleted`（C++ 订阅精确路径）；订阅时数据缓存失效；`extId=` 路径与不规范的路径（不以 `/` 开头、有空段）收不到通知（通知带规范路径）；取消订阅时先取空队列；取消订阅与 Drop 不等 `unSub` 的应答（C++ 的析构最多阻塞 5 s） |
| `GoResource::Links`、`Embedded`、`CallArguments` | `data()?["_links"]`、`data()?["_embedded"]`、`data()?.pointer("/parameters")` |  |
| `GoResource::ChildUris`、`ChildCount` | `Resource::child_paths(&str) -> Result<Vec<String>>` 及其长度 | rel 名见 `resource::rel`；rel 为单个对象时也读出（C++ 返回空） |
| `GoResource::Children` | `Resource::children(&str) -> Result<Vec<Resource>>` | 子资源继承超时与校验设置；展开层级不为 0 时条目放进子资源的缓存，子资源的展开层级取下一层（C++ 按键数猜测是否已展开，把带 `ordIndex` 的链接当作数据） |
| `GoResource::Child` | `Resource::child(&str) -> Resource` | 不发请求；继承超时与校验设置 |
| `GoResource::CreateChild` | `Resource::create_child(impl Into<Value>) -> Result<Resource>` | 新路径取响应的 `path`，没有时取 `_links.self.href`，都没有时为 `Error::Payload` |
| `GoResource::Delete`、`DeleteChild` | `Resource::delete(self) -> Result<()>` | 消耗资源；`notFound` 也算成功；子项用 `children` 得到的资源，或对 `child_paths` 的绝对路径 `Resource::new` 后删除 |
| `GoResource::DeleteAllChildren` | 对 `children(rel)?` 逐个 `delete()` |  |
| `GoResource::Call`、`CallCommand`、`CallAction` | `Resource::call(impl Into<Value>)`、`command(&str, impl Into<Value>)`、`action(&str, impl Into<Value>)`，均返回 `Transaction` | 超时为资源的超时，`Transaction::timeout` 可改；`notFound` 不把资源标记为已删除（C++ 的命令与动作失败时标记父资源） |
| `GoResource::CommandNames`、`ActionNames` | `Resource::command_names()`、`action_names() -> Result<Vec<String>>` | 动作同时读 `go:action` 与 `go:actions`（手册中传感器的例子用复数） |
| `GoResource::Read`、`Read(args)`、`Update(payload, args)` | `Resource::handle()` 的 `send(Request)` | 带任意参数的请求不进缓存 |
| `GoResource::IsDeleted`、`IsManaged`；`SetAutoFetchSchema`、`AutoFetchSchema`、`SetAutoInvalidation`、`AutoInvalidation` | 不提供 | 删除之后资源已被消耗，别处删除时下一次读取返回 `notFound`；没有管理器；schema 单独读取并缓存；自身的写入总是使缓存过期（C++ 关闭 `AutoInvalidation` 只对已订阅的资源有效，使它返回旧数据） |
| `GoResourceManager::GetOrCreate`、`SetAutoValidation`、`AutoValidation` | `System::resource`（开启校验）、`Resource::new`（不校验） | 没有身份映射：写计数与订阅让各自的缓存一起过期 |
| `GoResourceManager::SetAutoSubscribe`、`SubscribeAll`、`UnsubscribeAll`、`EnableValidationAll`、`DisableValidationAll`、`InvalidateAll`、`Remove`、`Cleanup`；`GoSystem::ResourceManager` | 不提供 | 资源由调用方拥有，没有中心注册表：对自己持有的资源逐个调用 `subscribe`、`set_validation`、`invalidate`，不再需要时 drop |
| `GoResourceManager::ResetConnectionState` | 不提供 | 连接断开后缓存自动过期；不重连，重建 `System` 与资源 |
| `GoResourceManager::SetSubscriptionOptimizedInvalidation`、`SubscriptionOptimizedInvalidation` | 不提供 | 已订阅的资源同样按写计数过期（相当于 C++ 关闭这个优化），订阅只用于发现其他客户端与设备自身的修改，写入之后不会读到通知到达之前的旧数据 |
| `GoResourceManager::Client` | `Resource::handle() -> &Handle` |  |
| `GO_PXL_SDK_DEFAULT_CONTROL_PORT`、`GO_PXL_SDK_DEFAULT_GDP_SERVER_PORT` | `control::DEFAULT_PORT`、`gdp::DEFAULT_PORT` | 3600、3601 |
| `GoGdpMsg::Deserialize`；`GoGdpClient` 的消息分派 | `gdp::decode_message(Bytes) -> Result<Message, GdpError>` | 每条消息都解析 CAS（C++ 跳过未知类型，丢掉 isLastMsg）；CAS 之后坏掉时为 `Body::Invalid` |
| — | `gdp::decode_all(Bytes) -> Result<Vec<Message>, GdpError>` | 控制通道 `/dataSources` 流的 bin |
| `GoGdpMsg::Type` | `gdp::Message` 的 `ty`、`body`（`Body`）字段 |  |
| `GoGdpMsg::DataSourceId`、`StampSourceId`、`SpaceType`、`HasTransform`、`Transform`、`HasBoundingBox`、`BoundingBox`、`ArrayedCount`、`ArrayedIndex`、`DataSetId`、`IsLastMsg`、`GdpId` | `gdp::Cas` 的字段 | 变换与包围盒是 `Option`；`gdp_id` 在旧服务端的短 CAS 或 0xFFFF 时为 `None` |
| `GoGdpNull::ErrorStatus` | `Body::Null(Status)` |  |
| `GoGdpStamp` 的 getter | `gdp::Stamp` 的字段 | `Timestamp` 为 `timestamp` |
| `GoGdpProfileBase`、`GoGdpProfileUniform`、`GoGdpProfilePointCloud` 的 getter | `gdp::ProfileAttrs`、`UniformProfile`、`ProfilePointCloud` 的字段 | `Resolution` / `Offset` 拆成 `x_scale`、`z_offset` 等；`Width`、`IntensityWidth` 即点数组与强度的长度；点是 `Vec`，强度是帧的切片 |
| `GoGdpSurfaceBase`、`GoGdpSurfaceUniform`、`GoGdpSurfacePointCloud` 的 getter | `gdp::SurfaceAttrs`、`UniformSurface`、`SurfacePointCloud` 的字段；`SurfaceAttrs::intensity_row_size` |  |
| `GoGdpImage` 的 getter | `gdp::Image` 的字段；`Image::row_size` | `ColumnBased` 为 `transposed`；1.3 的 scale / offset 是 `Option`，剩余字节不足时不读取 |
| `GoGdpSpots` 的 getter | `gdp::Spots` 的字段 | `PointCount` 即 `spots.len()` |
| `GoGdpMesh` 的 getter（`ChannelType(i)` 等按下标） | `gdp::Mesh` 的字段，`channels[i]`（`MeshChannel`） | 通道数据按 ID 解码为 `MeshData`，`AllocatedChannelDataCount` 即其长度；先确认字节够再分配 |
| `GoGdpMeasurement`、`GoGdpString` 的 getter | `gdp::Measurement`、`GdpString` 的字段 | 标签位置是 `Option`；字符串保留内嵌 NUL |
| `GoGdpRendering::GetGraphics`；`GoGraphics::*Count`、`*At` | `Body::Rendering(Rendering)` 中各图元的 `Vec` | 计数按 `u16` 读取（C++ 为 `k16s`）；未知区域类型为 `Region::Unknown` 并跳过；标签另读出 1.5 的对齐与颜色 |
| `GoGdpFeaturePoint`、`GoGdpFeatureLine`、`GoGdpFeaturePlane`、`GoGdpFeatureCircle` 的 getter | `gdp::PointFeature`、`LineFeature`、`PlaneFeature`、`CircleFeature` 的字段 |  |
| `GoGdpPixelFormat::PixelBits`、`IsNative`、`ToString` | `gdp::PixelFormat::bits`、`is_legacy`、`name` | `PixelBytes` 不提供（即 `bits() / 8`） |
| `GoPxLDiscoveryProto::Broadcast` 发出的报文 | `discovery::encode_discover() -> [u8; 24]` |  |
| `GoPxLDiscoveryProto::ParseReply`、`GoDiscoveryClient::ParseReply` | `discovery::parse_announce(&[u8]) -> Result<Instance, DiscoveryError>` | 只解析收到的字节；字段缺失或类型不对时取默认值（C++ 丢弃整条 Announce） |
| `ClassicDiscoveryProto::BroadcastGetIP`、`SendGetInfo` 发出的报文 | `discovery::classic::encode_get_ip() -> [u8; 32]`、`encode_get_info(u32) -> [u8; 32]` |  |
| `ClassicDiscoveryProto::ParseClassicGetIPReply`、`ParseClassicGetInfoReply` | `discovery::classic::handle_reply(&[u8], &mut Vec<Instance>) -> Result<Option<u32>, DiscoveryError>` | 返回需要发 `GetInfo` 的新序列号；按 `MsgId` 分派（C++ 按长度分派，丢掉 99 字节的 `GetInfo` 回复） |
| `GoInstance::Get*`、`Set*` | `discovery::Instance` 的字段 | 地址是 `Option<Ipv4Addr>`；另有 `classic`、`truncated` |
| — | `Instance::is_connectable`、`control_addr`、`gdp_addr` | SDK 示例 `Discover.cpp` 的连接判断 |

## 数据类型与协议

各层实现时补充。

| SDK 定义或协议 | Rust 定义 | 说明 |
| --- | --- | --- |
| `GoJson`（nlohmann::json 3.8.0）；.NET 的 `JObject` | `Value`：`Nil`、`Bool`、`Int(i64)`、`UInt(u64)`、`Float(f64)`、`Str`、`Bin(Bytes)`、`Array`、`Map` | 值语义，`Clone` 是深拷贝；能放进 `i64` 的整数一律是 `Int`，`UInt` 只用于更大的值；float32 解码时拓宽为 `f64` |
| nlohmann 对象（`std::map`） | `Map = BTreeMap<String, Value>` | 键按字节序，编码顺序与 C++ SDK 相同 |
| nlohmann `binary_t` | `Value::Bin(Bytes)` | `From<Vec<u8>>` 得到 `Bin`，与 `GoJson::Set(path, std::vector<k8u>)` 相同 |
| `GoJsonPointer` | `&str` | RFC 6901；与 `GoJsonPointer` 相同，缺少前导 `/` 时补上 |
| `GoJsonIterator` | `as_array()` / `as_map()` 返回的切片与 `BTreeMap` 的迭代器 |  |
| `GoJsonError` | `PointerError`、`value::msgpack::DecodeError`、`value::msgpack::EncodeError`、`serde_json::Error` | 读取返回 `Option`，不报错 |
| 控制通道消息体（carrier 类型 0xB000，MessagePack） | `value::msgpack` | 编码与 nlohmann 3.8.0 逐字节一致（最短整数、float64、键按字节序）；解码不依赖整数宽度与键序，ext 当作 `Bin` |
| 长度前缀帧：carrier 帧、GDP 消息 | `frame::FrameReader`、`frame::FrameError`；`control::HEADER_LEN`（14）、`gdp::HEADER_LEN`（6） | 小端 `u32` 长度包含自身；先校验长度再精确分配；`WouldBlock` 不丢失已读字节 |
| carrier 帧（`GoCtrlChannelV1`） | `Request::encode`、`Inbound::decode` | 客户端只收发 0xB000：服务端按请求的格式回复，C++ SDK 也只收 0xB000；与 C++ 一样不检查 `CarrierStatus` |
| `GoRequestMethod::Method` | `control::Method` |  |
| `GoResponseType` | `control::Inbound`：`Response`、`Notification`、`Stream`、`Other` |  |
| `GoNotificationType` | `control::EventType` | 含 `Other(String)` |
| 流状态字符串（`streaming` / `complete` / `abort`） | `control::StreamState` | 含 `Other(String)` |
| `kStatus` | `control::Status(i32)` 与全部常量（含 −975 `OVERHEAT`） | 表外的值原样保留 |
| `GoCtrlChannel::ReadErrorType` | `frame::FrameError`、`control::InboundError` |  |
| `SetDisconnectHandler` 通知的断开（C++ 不给原因） | `control::DisconnectReason`：`Closed`、`Eof`、`Read`、`Write`、`Decode`、`Desync` | 所有挂起的请求以同一个 `Arc` 原因失败；流连接与 GDP 连接的断开也用它表示 |
| `GoChannelError`、`GoRequestError`（.NET 的 `GoChannelException`、`GoRequestException`） | `Error`：`Connect`、`Timeout`、`Disconnected`、`Request(Box<RequestError>)`、`Payload`、`Validation`、`InvalidArgument`；`RequestError` 携带方法、路径与完整的 `Response` | `Display` 带第一条 `payload.errors` 的说明；`status` 为 OK 时 `payload.errors` 不升级为错误（与 SDK 一致）；`Payload` 是 `System`、`Resource::get` 或 `create_child` 要读取的字段缺失或类型不对（C++ 抛 `GoJsonError` / `GoResourceError`）；`Validation` 见 `GoResourceValidationError` |
| `GoSystem::State` | `RunState`：`Ready`、`Running`、`Conflict`、`Other(i64)` | 0、1、2 与表外的值 |
| `GoTransaction` | `control::Transaction` |  |
| 应答通道（C++ 的 `GoTransaction` 内部状态） | `control::ReplyRx` = `Receiver<Result<Response, Arc<DisconnectReason>>>` | 由 `register` 建立的 `sync_channel(1)`，读线程持锁发送时从不阻塞；丢弃即墓碑 |
| `GoNotificationHandler`、`NotificationCallback` 收到的通知 | `control::Notice`：`Event(Arc<Notification>)`、`Lagged(u64)` | 一条通知投递给多个订阅时共享；`Lagged` 是在此之前丢掉的条数 |
| `NotificationListenerId` | `control::Listener` | drop 即移除 |
| `GoStreamHandler` 收到的 `GoStreamResponse` | `control::StreamMessage`（`StreamHandle::recv_timeout` 取出） | 结束消息照常取出：`state` 为 `Complete` / `Abort`，`status` 为结束原因；`/dataSources` 的数据可能紧跟在结束消息之后（服务端每轮先写响应队列、再写流队列） |
| `GoDataSet` | `Vec<gdp::Message>` | 以 `is_last_msg` 的消息结束 |
| `ReceiveDataSync` 的超时与不完整 | `gdp::RecvError`：`Timeout`、`Incomplete { messages }`、`Disconnected` |  |
| 服务端的 `SubscriptionExpression` | `control::SubscriptionExpr`、`control::SubExprError` | 两个正则（R、E）逐字移植后由 regex-lite 编译 |
| `_schema`（基于 JSON Schema Draft 2019-09 的方言） | `Value` | 注解（`unit`、`visibility`、`extendedType` …）直接读取 |
| `GoResourceValidationError::Errors()` | `schema::Issue` | 每个关键字一个变体 |
| `GoRelationType` | `resource::rel`：`ITEM`、`SCANNER`、`SUB_TASK`、`CONTENT`、`COMMAND`、`ACTION` | `GoResource::DEFAULT_RELATION_TYPE` 即 `ITEM`；Rust 的方法显式传入 rel |
| `GoResource::DEFAULT_TIMEOUT_MS`、`DEFAULT_EXPAND_LEVEL` | `Resource::new` 的默认值：5 s、0 |  |
| `GoResourceError` | `Error::Payload`（`get` 的字段缺失或类型不对，`create_child` 的响应中找不到新路径） | C++ 的其余抛出点（资源已删除、缺少管理器、`EndUpdate` 不配对）在 Rust 中不存在 |
| `GoResourceValidationError` | `Error::Validation { path, pointer, issues }` | `Errors()` 即 `issues`，`pointer` 是补丁中不合格的值的位置 |
| HAL 的 rel（数组、单个对象、`[]`、nil 或缺失） | `Resource::child_paths`、`children`、`command_names`、`action_names` 按条目列表读取 | 服务端只有一个引用时写出单个对象 |
| `GoGdpMsg`；`MessageType` | `gdp::Message`；`gdp::Body` 的变体 | 未知类型为 `Body::Unknown(Bytes)` |
| `CommonAttributes` | `gdp::Cas` |  |
| `GoGdpTransform`、`GoGdpBoundingBox` | `Option<[f32; 12]>`、`Option<[f32; 6]>` | 线上顺序，保留 `f32` |
| `kArray1<k16s>`、`kArray2<k16s>`、`kPoint16s`、`kPoint3d16s` 的数组 | `Vec<i16>`、`Vec<[i16; 2]>`、`Vec<[i16; 3]>` | 行主序；`k16S_NULL` 即 `i16::MIN` |
| `kArray1<k8u>`、`kArray2<k8u>`（强度、图像像素） | `Bytes` | 帧的零拷贝切片 |
| `GoGdpPixelFormat::FormatType` | `gdp::PixelFormat(i32)` 与全部常量 | BayerXX16 用 PFNC 值 0x0110002E 起（.NET 的值错误） |
| `GdpSpot` | `gdp::Spot` |  |
| `MeshMsgChannel`、`Channel_ID` | `gdp::MeshChannel`、`gdp::MeshData` |  |
| `GoGraphics` 的 `GoPointSet`、`GoLineSet`、`GoRegion`、`GoPlane`、`GoRay`、`GoLabel`、`GoPosition` | `gdp::PointSet`、`LineSet`、`Region`、`Plane`、`Ray`、`Label`、`Position` |  |
| 以 `k64F_NULL` 表示的缺失字段 | `Option` |  |
| `GoInstance` | `discovery::Instance` |  |
| 发现报文（`DiscoveryBroadcastHeader`、`DiscoveryServerHeader`、`ClassicDiscoveryGetIPCommand`、`ClassicDiscoveryGetIPReply`、`ClassicDiscoveryGetInfoReply`）与 Announce 的 JSON | `discovery` 的编解码函数与 `Instance` 字段 | 整数小端；经典回复中的 IPv4 地址按网络字节序 |
| 发现报文的解析错误 | `discovery::DiscoveryError` | `Command` 表示收到的是客户端命令（含自己的回环） |

## 许可

MIT。本项目参考了 LMI GoPxL SDK、kApi 与 nlohmann/json 的行为（均为 MIT），不含 GoApi 的代码。
