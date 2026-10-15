//! LMI GoPxL（Gocator 3D 传感器平台）客户端 SDK 的 Rust 实现（非官方）。
//!
//! 目标服务端是 GoPxL 1.5，协议为 REST API 9.0.0；替代随 GoPxL 1.5 发布的官方 C++ `GoPxLSdk` 与
//! .NET `GoPxLSdkNet`（SDK 自身版本号 8.5.62.52）。
//! 本 crate 直接实现线上协议（TCP 控制通道、GDP 数据通道、UDP 发现），不链接厂商库，也不依赖 C 库。
//!
//! 设计约定（分层、所有权、阻塞、unsafe、错误）见 [`docs::architecture`]。

pub mod docs;
pub mod value;

pub use value::{Map, PointerError, Value};

// 两份 README 中的 Rust 示例参与 doctest，避免与 API 脱节。
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../README.en.md")]
struct ReadmeDoctests;
