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
| `GoJson::GoJson()` | `Value::object() -> Value` | `Value::default()` is `Nil` |
| `GoJson::GoJson(const std::string&)`, `GoJson::ParseString` | `str::parse::<Value>()` (`FromStr`, `Err = serde_json::Error`) | Parses JSON text |
| `GoJson::ToString` | `impl Display for Value` | Compact JSON, indented with `{:#}`; `Bin` prints as `"<bin N bytes>"` |
| `GoJson::ToBinary` | `value::msgpack::encode(&Value, &mut Vec<u8>) -> Result<(), EncodeError>` | Byte-identical to nlohmann 3.8.0; errors when a length exceeds `u32::MAX` |
| `GoJson::ParseMsgPack` | `value::msgpack::decode(&Bytes) -> Result<Value, DecodeError>` | `Bin` is a zero-copy slice of the input; nesting is limited to 128 levels |
| `GoJson::Get<T>`, `At` | `Value::pointer(&self, &str) -> Option<&Value>`; `Index<&str>`, `Index<usize>` (`&Value::Nil` when missing); `as_bool`, `as_i64`, `as_u64`, `as_f64`, `as_str`, `as_array`, `as_map` | Never modifies the document; integer accessors do not truncate floats, and `as_u64` does not wrap negative numbers |
| `GoJson::GetBinary` | `Value::as_bytes(&self) -> Option<&Bytes>` | No copy |
| `GoJson::Set`, `Replace` | `Value::set(&mut self, &str, impl Into<Value>) -> Result<(), PointerError>` | Creates only objects automatically; arrays take an existing index or `-` to append; the document is unchanged on error |
| `GoJson::Remove` | `Value::remove(&mut self, &str) -> Option<Value>` | Later array elements shift down, leaving no null |
| `GoJson::Merge` | `Value::merge_patch(&mut self, &Value)` | RFC 7396 |
| `GoJson::Contains`, `HasKey`, `Is*`, `Size`, `Empty`, `Begin`, `End`, `Find` | `pointer(..).is_some()`, `is_nil()`, `matches!`, length and iterators of `as_array()` / `as_map()` |  |
| `GoJson::Add`, `Copy`, `Move`, `Patch`, `Flatten`, `Unflatten`, `GetHandle` | Not provided | Neither the SDK nor the samples call them; `Add`, `Copy` and `Move` are built on the lossy flatten and are covered by combining `set` / `remove`; `GetHandle` exposes the nlohmann object |

## Data types and protocol

Filled in as each layer is implemented.

| SDK definition or protocol | Rust definition | Notes |
| --- | --- | --- |
| `GoJson` (nlohmann::json 3.8.0); .NET `JObject` | `Value`: `Nil`, `Bool`, `Int(i64)`, `UInt(u64)`, `Float(f64)`, `Str`, `Bin(Bytes)`, `Array`, `Map` | Value semantics, `Clone` is a deep copy; every integer that fits in `i64` is `Int`, `UInt` is only for larger values; float32 is widened to `f64` when decoding |
| nlohmann object (`std::map`) | `Map = BTreeMap<String, Value>` | Keys in byte order, so encoding order matches the C++ SDK |
| nlohmann `binary_t` | `Value::Bin(Bytes)` | `From<Vec<u8>>` produces `Bin`, like `GoJson::Set(path, std::vector<k8u>)` |
| `GoJsonPointer` | `&str` | RFC 6901; like `GoJsonPointer`, a missing leading `/` is added |
| `GoJsonIterator` | The slice and `BTreeMap` iterators returned by `as_array()` / `as_map()` |  |
| `GoJsonError` | `PointerError`, `value::msgpack::DecodeError`, `value::msgpack::EncodeError`, `serde_json::Error` | Reads return `Option` and never fail |
| Control channel message body (carrier type 0xB000, MessagePack) | `value::msgpack` | Encoding is byte-identical to nlohmann 3.8.0 (shortest integers, float64, keys in byte order); decoding does not depend on integer width or key order, and ext is read as `Bin` |

## License

MIT. This project references the behavior of the LMI GoPxL SDK, kApi and nlohmann/json (all MIT) and contains no
GoApi code.
