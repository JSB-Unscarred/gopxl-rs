//! # 设计
//!
//! 本 crate 直接实现 GoPxL 的线上协议，不包装厂商 SDK：`GoPxLSdk` 动态链接许可不明的 GoApi，包装就要随产物
//! 分发 `GoApi.dll` / `libGoApi.so`；它的缺陷（响应错位、每条消息一个线程、析构阻塞、GDP 解析错误）也无法在
//! 包装层修复。
//!
//! ## 分层
//!
//! - 编解码：值模型（[`Value`](crate::Value)、MessagePack）、carrier 帧、GDP 消息、发现报文与协议状态机是
//!   纯函数或纯数据结构，不做 I/O、不起线程，直接用字节测试；
//! - 连接：每种连接类型读写自己的 socket，把字节交给编解码层，相当于 FFI 封装中“方法直接调用 `sys`”。
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
//!
//! ## 阻塞
//!
//! 只提供阻塞 API，不依赖异步运行时：原 SDK 的宿主（Qt、MFC、PLC 网关、Python 扩展）直接调用阻塞接口。
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
//! - `serde_json::Error`：`str::parse::<Value>()` 的 JSON 文本不合法。
