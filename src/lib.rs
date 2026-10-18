//! LMI GoPxL（Gocator 3D 传感器平台）客户端 SDK 的 Rust 实现（非官方）。
//!
//! 目标服务端是 GoPxL 1.5，协议为 REST API 9.0.0；替代随 GoPxL 1.5 发布的官方 C++ `GoPxLSdk` 与
//! .NET `GoPxLSdkNet`（SDK 自身版本号 8.5.62.52）。
//! 本 crate 直接实现线上协议（TCP 控制通道、GDP 数据通道、UDP 发现），不链接厂商库，也不依赖 C 库。
//!
//! 入口：[`System`]（一台设备，拥有控制连接；[`System::resource`] 取得带缓存的资源，[`System::gdp`] 连接数据通道）、
//! [`gdp::Client`]（测量数据）、[`discovery::discover`]（发现）。
//! `examples/` 是官方 C++ 示例的移植。设计约定（分层、所有权、线程、unsafe、错误）见 [`docs::architecture`]。

pub mod control;
pub mod discovery;
pub mod docs;
mod error;
pub mod frame;
pub mod gdp;
mod net;
pub mod resource;
pub mod schema;
mod system;
pub mod value;

pub use error::{DisconnectReason, Error, RequestError, Result};
pub use system::{RunState, System};
pub use value::{Map, PointerError, Value};

/// 实现的 REST API 版本，对应 C++ 的 `ApiVersion()`。设备的版本在 `read /version` 的 `apiVersion` 中，主版本
/// 不同时协议可能不兼容。
pub const API_VERSION: &str = "9.0.0";

/// 对应的 GoPxL 版本，对应 C++ 的 `ReleaseVersion()`。
pub const RELEASE_VERSION: &str = "1.5";
