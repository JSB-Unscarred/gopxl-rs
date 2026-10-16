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
| `GoRestClient` 的事务队列（读线程按发送顺序完成 `GoTransaction`） | `control::ControlCore`：`register(&Request) -> ReplyRx`、`on_inbound(Inbound) -> Result<Option<Notification>, DisconnectReason>`、`fail_all(&Arc<DisconnectReason>)` | 纯状态机，不做 I/O，连接层在锁外解码后交给它；写出前登记；超时的请求留下墓碑吸收迟到的响应；响应带 `path` 且队首不是 `create` 时按段比较，不一致或挂起队列为空时报错，由连接层拆连接（C++ 错位后继续） |
| `GoRestClient` 的流消息分派（`SetStreamHandler`） | `control::StreamCore::on_inbound(Inbound) -> StreamEvent` | 每个流一条连接；应答前的数据照常产出，应答之后的孤立响应忽略（C++ 当作下一个请求的应答） |
| `GoRestClient::AddNotificationListener` 的路径匹配 | `control::SubscriptionExpr::new(&str) -> Result<SubscriptionExpr, SubExprError>`、`delivers(&EventType, &str) -> bool`、`text()` | 按服务端的规则：`*` 跨越 `/`，`/a/*` 也匹配 `/a`，`embeddedUpdated` 匹配祖先（C++ 按前缀与祖先回溯，与服务端不一致）；与服务端一样按字节匹配，`.` 不匹配 `\r`、`\n`；拒绝正则元字符 `\| ( ) [ ] { } ^ \` 与 NUL（服务端在 NUL 处截断表达式），生成的正则无法编译时同样报错 |
| `GoSchemaValidator::SchemaForPath` | `schema::schema_for_path(&Value, &str) -> Option<&Value>` | 路径不在 schema 中时为 `None`（C++ 抛 `GoResourceError`）；路径与 `Value::pointer` 同一语法（`/` 是键 `""`，C++ 把它当作根）；`type` 为可空数组时同样导航 |
| `GoSchemaValidator::Validate` | `schema::validate(&Value, &Value) -> Vec<Issue>` | 空表即通过；`binary` 要求 `Bin`，支持可空类型数组，`multipleOf` 对负数不误报，`enum` 的整数精确比较，空 `enum` 不约束，长度按字符计；`type: "null"` 只接受 `Nil`（C++ 不认识它，一律通过） |
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
| carrier 帧（`GoCtrlChannelV1`） | `Request::encode`、`Inbound::decode` | 客户端只收发 0xB000：服务端按请求的格式回复，C++ SDK 也只收 0xB000；`CarrierStatus` 必须为 1 |
| `GoRequestMethod::Method` | `control::Method` |  |
| `GoResponseType` | `control::Inbound`：`Response`、`Notification`、`Stream`、`Other` |  |
| `GoNotificationType` | `control::EventType` | 含 `Other(String)` |
| 流状态字符串（`streaming` / `complete` / `abort`） | `control::StreamState` | 含 `Other(String)` |
| `kStatus` | `control::Status(i32)` 与全部常量（含 −975 `OVERHEAT`） | 表外的值原样保留 |
| `GoCtrlChannel::ReadErrorType` | `frame::FrameError`、`control::InboundError` |  |
| `SetDisconnectHandler` 通知的断开（C++ 不给原因） | `control::DisconnectReason`：`Decode`、`Unsolicited`、`Desync` | 所有挂起的请求以同一个 `Arc` 原因失败；I/O 原因随连接层加入 |
| 应答通道（C++ 的 `GoTransaction` 内部状态） | `control::ReplyRx` = `Receiver<Result<Response, Arc<DisconnectReason>>>` | 由 `register` 建立的 `sync_channel(1)`，读线程持锁发送时从不阻塞；丢弃即墓碑 |
| `GoStreamHandler` 收到的 `GoStreamResponse` 与 `stream` 的应答 | `control::StreamEvent`：`Reply`、`Data`、`End(Status)`、`Ignored` | `End` 的 `OK` 即 complete，否则是 abort 的原因；`/dataSources` 的数据可能紧跟在 `End` 之后 |
| 服务端的 `SubscriptionExpression` | `control::SubscriptionExpr`、`control::SubExprError` | 两个正则（R、E）逐字移植后由 regex-lite 编译 |
| `_schema`（基于 JSON Schema Draft 2019-09 的方言） | `Value` | 注解（`unit`、`visibility`、`extendedType` …）直接读取 |
| `GoResourceValidationError::Errors()` | `schema::Issue` | 每个关键字一个变体 |
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
