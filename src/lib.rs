//! LMI GoPxL（Gocator 3D 传感器平台）客户端 SDK 的 Rust 实现（非官方）。
//!
//! 目标服务端是 GoPxL 1.5，协议为 REST API 9.0.0；替代随 GoPxL 1.5 发布的官方 C++ `GoPxLSdk` 与
//! .NET `GoPxLSdkNet`（SDK 自身版本号 8.5.62.52）。
//! 本 crate 直接实现线上协议（TCP 控制通道、GDP 数据通道、UDP 发现），不链接厂商库，也不依赖 C 库。
//!
//! 入口：[`control::Client`]（控制通道）、[`gdp::GdpClient`]（测量数据）、[`discovery::discover`]（发现）。
//! 设计约定（分层、所有权、线程、unsafe、错误）见 [`docs::architecture`]。

pub mod control;
pub mod discovery;
pub mod docs;
mod error;
pub mod frame;
pub mod gdp;
mod net;
pub mod schema;
pub mod value;

pub use error::{Error, RequestError, Result};
pub use value::{Map, PointerError, Value};
