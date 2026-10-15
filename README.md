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

## 数据类型与协议

各层实现时补充。

| SDK 定义或协议 | Rust 定义 | 说明 |
| --- | --- | --- |

## 许可

MIT。本项目参考了 LMI GoPxL SDK、kApi 与 nlohmann/json 的行为（均为 MIT），不含 GoApi 的代码。
