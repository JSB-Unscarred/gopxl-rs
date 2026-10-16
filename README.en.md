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
| `GoRequest::GoRequest(method, uri, content, args)` | `control::Request::new(Method, impl Into<String>) -> Request`, then set the `payload` and `args` fields | `payload` and `args` default to `{}` (`0x80` on the wire); the path is sent verbatim |
| `GoRequest::Method`, `Uri`, `Content`, `Arguments` | The `method`, `path`, `payload` and `args` fields of `Request` |  |
| `GoRequest::ToByteArray` | `Request::encode(&self, &mut Vec<u8>) -> Result<(), EncodeError>` | Writes the whole frame including the carrier header, byte-identical to the C++ SDK; errors when the length exceeds `u32::MAX`, writing nothing |
| `GoRequest::Id` | Not provided | Only a local counter; there is no request id on the wire |
| `GoRequestMethod::ToString` | `Method::as_str` | Adds `ListSub`, `ClearSub`, `ListStream`; the server rejects the all-lowercase .NET spellings (`unsub`, `cancelstream`), so the exact spelling is used |
| `GoResponse::Type`, `GoResponseType::FromString`; frame reading in `GoCtrlChannelV1` | `control::Inbound::decode(Bytes) -> Result<Inbound, InboundError>` | An unknown `type` becomes `Inbound::Other` (C++ treats it as a response); frames the server never produces are errors (C++ drops them and the FIFO goes out of step) |
| `GoResponse::Status`, `Path`, `Payload` (`GoRequestResponse`) | The `status`, `path` and `payload` fields of `control::Response` | A missing `status` is a malformed frame (C++ treats it as success); a missing `path` is an empty string, a missing `payload` is `Nil` |
| `GoNotificationResponse::NotificationType`; `GoNotificationType::FromString` | The `event` field (`EventType`) of `control::Notification` | Unknown strings become `EventType::Other` (C++ maps them to `Updated`) |
| `GoStreamResponse::StreamIdentifier`, `StreamStatus` | The `stream_id` and `state` (`StreamState`) fields of `control::StreamMessage` |  |
| `GoResponse::Raw` | Not provided | The typed fields hold the whole content |
| `kStatus_Name` | `control::Status::name`, `is_ok`; `impl Display for Status` | Returns the manual's name (`"notFound"`) |
| `GoUri::UrlEncode`, `UrlDecode` | `control::url_encode(&str) -> String`, `control::url_decode(&str) -> Vec<u8>` | No `spaceAsPlus`; decoding never reads out of bounds and the result may not be UTF-8, so it returns bytes |
| `GoRestClient` transaction queue (the reader thread completes each `GoTransaction` in send order) | `control::ControlCore`: `register(&Request) -> ReplyRx`, `on_inbound(Inbound) -> Result<Option<Notification>, DisconnectReason>`, `fail_all(&Arc<DisconnectReason>)` | Pure state machine with no I/O; the connection layer decodes each frame outside the lock and passes it in; registered before the request is written; a timed-out request leaves a tombstone that absorbs its late response; when a response carries a `path` and the head request is not `create`, the two are compared segment by segment, and a mismatch or a response with no pending request is an error, so the connection layer drops the connection (C++ carries on misaligned) |
| `GoRestClient` stream message dispatch (`SetStreamHandler`) | `control::StreamCore::on_inbound(Inbound) -> StreamEvent` | One connection per stream; data before the reply is still yielded, and spurious responses after the reply are ignored (C++ takes them as the next request's reply) |
| Path matching of `GoRestClient::AddNotificationListener` | `control::SubscriptionExpr::new(&str) -> Result<SubscriptionExpr, SubExprError>`, `delivers(&EventType, &str) -> bool`, `text()` | Server rules: `*` crosses `/`, `/a/*` also matches `/a`, `embeddedUpdated` matches ancestors (C++ uses prefix and ancestor walks that disagree with the server); like the server, matching is byte-wise and `.` does not match `\r` or `\n`; regex metacharacters `\| ( ) [ ] { } ^ \` are rejected, and so is an expression whose generated regex does not compile |
| `GoSchemaValidator::SchemaForPath` | `schema::schema_for_path(&Value, &str) -> Option<&Value>` | `None` when the path is not in the schema (C++ throws `GoResourceError`); paths use the same syntax as `Value::pointer` (`/` is the key `""`, which C++ treats as the root); also navigates when `type` is a nullable array |
| `GoSchemaValidator::Validate` | `schema::validate(&Value, &Value) -> Vec<Issue>` | An empty list means valid; `binary` requires `Bin`, nullable type arrays are supported, `multipleOf` is correct for negative values, integers in `enum` compare exactly, an empty `enum` imposes no constraint, lengths count characters |
| `GO_PXL_SDK_DEFAULT_CONTROL_PORT`, `GO_PXL_SDK_DEFAULT_GDP_SERVER_PORT` | `control::DEFAULT_PORT`, `gdp::DEFAULT_PORT` | 3600, 3601 |
| `GoGdpMsg::Deserialize`; message dispatch in `GoGdpClient` | `gdp::decode_message(Bytes) -> Result<Message, GdpError>` | The CAS is parsed for every message (C++ skips unknown types and loses isLastMsg); a broken part after the CAS gives `Body::Invalid` |
| — | `gdp::decode_all(Bytes) -> Result<Vec<Message>, GdpError>` | The bin of a control-channel `/dataSources` stream |
| `GoGdpMsg::Type` | The `ty` and `body` (`Body`) fields of `gdp::Message` |  |
| `GoGdpMsg::DataSourceId`, `StampSourceId`, `SpaceType`, `HasTransform`, `Transform`, `HasBoundingBox`, `BoundingBox`, `ArrayedCount`, `ArrayedIndex`, `DataSetId`, `IsLastMsg`, `GdpId` | The fields of `gdp::Cas` | Transform and bounding box are `Option`; `gdp_id` is `None` for an old server's short CAS or 0xFFFF |
| `GoGdpNull::ErrorStatus` | `Body::Null(Status)` |  |
| Getters of `GoGdpStamp` | The fields of `gdp::Stamp` | `Timestamp` is `timestamp` |
| Getters of `GoGdpProfileBase`, `GoGdpProfileUniform`, `GoGdpProfilePointCloud` | The fields of `gdp::ProfileAttrs`, `UniformProfile`, `ProfilePointCloud` | `Resolution` / `Offset` are split into `x_scale`, `z_offset` etc.; `Width` and `IntensityWidth` are the lengths of the point array and the intensity; points are a `Vec`, intensity is a slice of the frame |
| Getters of `GoGdpSurfaceBase`, `GoGdpSurfaceUniform`, `GoGdpSurfacePointCloud` | The fields of `gdp::SurfaceAttrs`, `UniformSurface`, `SurfacePointCloud`; `SurfaceAttrs::intensity_row_size` |  |
| Getters of `GoGdpImage` | The fields of `gdp::Image`; `Image::row_size` | `ColumnBased` is `transposed`; the 1.3 scale / offset are `Option` and are not read when too few bytes remain |
| Getters of `GoGdpSpots` | The fields of `gdp::Spots` | `PointCount` is `spots.len()` |
| Getters of `GoGdpMesh` (`ChannelType(i)` etc. by index) | The fields of `gdp::Mesh`, `channels[i]` (`MeshChannel`) | Channel data is decoded by id into `MeshData`, whose length is `AllocatedChannelDataCount`; byte counts are checked before allocating |
| Getters of `GoGdpMeasurement`, `GoGdpString` | The fields of `gdp::Measurement`, `GdpString` | The label position is `Option`; strings keep embedded NULs |
| `GoGdpRendering::GetGraphics`; `GoGraphics::*Count`, `*At` | The `Vec` of each primitive in `Body::Rendering(Rendering)` | Counts are read as `u16` (C++ uses `k16s`); unknown region types become `Region::Unknown` and are skipped; labels also expose the 1.5 alignment and color |
| Getters of `GoGdpFeaturePoint`, `GoGdpFeatureLine`, `GoGdpFeaturePlane`, `GoGdpFeatureCircle` | The fields of `gdp::PointFeature`, `LineFeature`, `PlaneFeature`, `CircleFeature` |  |
| `GoGdpPixelFormat::PixelBits`, `IsNative`, `ToString` | `gdp::PixelFormat::bits`, `is_legacy`, `name` | `PixelBytes` is not provided (it is `bits() / 8`) |
| The datagram sent by `GoPxLDiscoveryProto::Broadcast` | `discovery::encode_discover() -> [u8; 24]` |  |
| `GoPxLDiscoveryProto::ParseReply`, `GoDiscoveryClient::ParseReply` | `discovery::parse_announce(&[u8]) -> Result<Instance, DiscoveryError>` | Parses only the received bytes; a missing or ill-typed field takes its default (C++ drops the whole Announce) |
| The datagrams sent by `ClassicDiscoveryProto::BroadcastGetIP`, `SendGetInfo` | `discovery::classic::encode_get_ip() -> [u8; 32]`, `encode_get_info(u32) -> [u8; 32]` |  |
| `ClassicDiscoveryProto::ParseClassicGetIPReply`, `ParseClassicGetInfoReply` | `discovery::classic::handle_reply(&[u8], &mut Vec<Instance>) -> Result<Option<u32>, DiscoveryError>` | Returns a new serial that needs a `GetInfo`; dispatches by `MsgId` (C++ dispatches by length and drops 99-byte `GetInfo` replies) |
| `GoInstance::Get*`, `Set*` | The fields of `discovery::Instance` | Addresses are `Option<Ipv4Addr>`; adds `classic` and `truncated` |
| — | `Instance::is_connectable`, `control_addr`, `gdp_addr` | The connect check of the SDK sample `Discover.cpp` |

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
| Length-prefixed frames: carrier frames, GDP messages | `frame::FrameReader`, `frame::FrameError`; `control::HEADER_LEN` (14), `gdp::HEADER_LEN` (6) | The little-endian `u32` length includes itself; the length is checked before an exact allocation; `WouldBlock` loses no bytes already read |
| Carrier frame (`GoCtrlChannelV1`) | `Request::encode`, `Inbound::decode` | The client sends and receives only 0xB000: the server replies in the request's format, and the C++ SDK also accepts only 0xB000; `CarrierStatus` must be 1 |
| `GoRequestMethod::Method` | `control::Method` |  |
| `GoResponseType` | `control::Inbound`: `Response`, `Notification`, `Stream`, `Other` |  |
| `GoNotificationType` | `control::EventType` | Includes `Other(String)` |
| Stream status strings (`streaming` / `complete` / `abort`) | `control::StreamState` | Includes `Other(String)` |
| `kStatus` | `control::Status(i32)` with every constant (including −975 `OVERHEAT`) | Values outside the table are kept as is |
| `GoCtrlChannel::ReadErrorType` | `frame::FrameError`, `control::InboundError` |  |
| Disconnects reported through `SetDisconnectHandler` (C++ gives no reason) | `control::DisconnectReason`: `Decode`, `Unsolicited`, `Desync` | Every pending request fails with the same `Arc` reason; I/O reasons are added with the connection layer |
| Reply channel (internal state of a C++ `GoTransaction`) | `control::ReplyRx` = `Receiver<Result<Response, Arc<DisconnectReason>>>` | A `sync_channel(1)` created by `register`, so the reader thread never blocks when it sends under the lock; dropping it makes a tombstone |
| `GoStreamResponse` received by a `GoStreamHandler`, and the `stream` reply | `control::StreamEvent`: `Reply`, `Data`, `End(Status)`, `Ignored` | `OK` in `End` means complete, anything else is the abort reason; `/dataSources` data may arrive right after `End` |
| The server's `SubscriptionExpression` | `control::SubscriptionExpr`, `control::SubExprError` | Both regexes (R, E) are ported verbatim and compiled with regex-lite |
| `_schema` (a dialect based on JSON Schema Draft 2019-09) | `Value` | Annotations (`unit`, `visibility`, `extendedType` …) are read directly |
| `GoResourceValidationError::Errors()` | `schema::Issue` | One variant per keyword |
| `GoGdpMsg`; `MessageType` | `gdp::Message`; the variants of `gdp::Body` | Unknown types become `Body::Unknown(Bytes)` |
| `CommonAttributes` | `gdp::Cas` |  |
| `GoGdpTransform`, `GoGdpBoundingBox` | `Option<[f32; 12]>`, `Option<[f32; 6]>` | Wire order, kept as `f32` |
| Arrays of `kArray1<k16s>`, `kArray2<k16s>`, `kPoint16s`, `kPoint3d16s` | `Vec<i16>`, `Vec<[i16; 2]>`, `Vec<[i16; 3]>` | Row-major; `k16S_NULL` is `i16::MIN` |
| `kArray1<k8u>`, `kArray2<k8u>` (intensity, image pixels) | `Bytes` | Zero-copy slices of the frame |
| `GoGdpPixelFormat::FormatType` | `gdp::PixelFormat(i32)` with every constant | BayerXX16 uses the PFNC values from 0x0110002E (the .NET values are wrong) |
| `GdpSpot` | `gdp::Spot` |  |
| `MeshMsgChannel`, `Channel_ID` | `gdp::MeshChannel`, `gdp::MeshData` |  |
| `GoPointSet`, `GoLineSet`, `GoRegion`, `GoPlane`, `GoRay`, `GoLabel`, `GoPosition` of `GoGraphics` | `gdp::PointSet`, `LineSet`, `Region`, `Plane`, `Ray`, `Label`, `Position` |  |
| Missing fields marked with `k64F_NULL` | `Option` |  |
| `GoInstance` | `discovery::Instance` |  |
| Discovery datagrams (`DiscoveryBroadcastHeader`, `DiscoveryServerHeader`, `ClassicDiscoveryGetIPCommand`, `ClassicDiscoveryGetIPReply`, `ClassicDiscoveryGetInfoReply`) and the Announce JSON | The codec functions of `discovery` and the fields of `Instance` | Little-endian integers; IPv4 addresses in classic replies are in network byte order |
| Discovery parse errors | `discovery::DiscoveryError` | `Command` means the datagram is a client command (including our own loopback) |

## License

MIT. This project references the behavior of the LMI GoPxL SDK, kApi and nlohmann/json (all MIT) and contains no
GoApi code.
