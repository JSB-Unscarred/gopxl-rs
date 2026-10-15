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
| `GoJson::GoJson(const std::string&)`、`GoJson::ParseString` | `str::parse::<Value>()`（`FromStr`，`Err = serde_json::Error`） | 解析 JSON 文本 |
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

## 许可

MIT。本项目参考了 LMI GoPxL SDK、kApi 与 nlohmann/json 的行为（均为 MIT），不含 GoApi 的代码。
