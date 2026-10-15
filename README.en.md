# gopxl

[中文](README.md)

An unofficial Rust implementation of the LMI GoPxL (Gocator 3D sensor platform) client SDK, replacing the official C++
GoPxLSdk and .NET GoPxLSdkNet. This crate implements the wire protocol directly and does not link the vendor
libraries.

## Versions

| Item | Version | Notes |
| --- | --- | --- |
| Target server | GoPxL 1.5 | Sensor firmware or the GoPxL PC application |
| Protocol | REST API 9.0.0 | Protocol version of the control channel |
| Replaced official SDK | 8.5.62.52 | The C++ GoPxLSdk and .NET GoPxLSdkNet ship with GoPxL 1.5; this is the SDK's own version number |

## Requirements

- Rust 1.87 or newer; no C dependencies, and building does not require the GoPxL SDK.
- Target platforms match the C++ SDK: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `armv7-unknown-linux-gnueabihf`; currently built and verified only on
  `x86_64-pc-windows-msvc`.

## Implemented interfaces

Based on the public headers of the official C++ SDK (8.5.62.52); a same-named .NET interface shares the row, with
behavioral differences in the Notes column, and .NET-only interfaces get rows of their own. Filled in as each layer is
implemented.

| SDK interface | Rust interface | Notes |
| --- | --- | --- |

## Data types and protocol

Filled in as each layer is implemented.

| SDK definition or protocol | Rust definition | Notes |
| --- | --- | --- |

## License

MIT. This project references the behavior of the LMI GoPxL SDK, kApi and nlohmann/json (all MIT) and contains no
GoApi code.
