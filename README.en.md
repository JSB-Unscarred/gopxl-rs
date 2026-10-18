# gopxl

[中文](README.md)

An unofficial Rust implementation of the LMI GoPxL (Gocator 3D sensor platform) client SDK, usable in place of the
official C++ GoPxLSdk and .NET GoPxLSdkNet. This crate implements the wire protocol directly and does not link the
vendor libraries.

## Versions

| Item | Version | Notes |
| --- | --- | --- |
| Target server | GoPxL 1.5 | Sensor firmware or the GoPxL PC application |
| Protocol | REST API 9.0.0 | Protocol version of the control channel |
| Reference official SDK | 8.5.62.52 | Version number of the C++ GoPxLSdk and .NET GoPxLSdkNet shipped with GoPxL 1.5 |

## Requirements

- Rust 1.87 or newer; no C dependencies, and building does not require the GoPxL SDK.
- The connection layer is built on tokio: all I/O is `async fn` and the caller provides the runtime (the examples use `#[tokio::main]`).
- Target platforms: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `armv7-unknown-linux-gnueabihf`; currently built and verified only on `x86_64-pc-windows-msvc`.

## Examples

`examples/` corresponds to the official SDK samples. The device address comes from the `GOPXL_IP` environment
variable (`IP` or `IP:port`, port 3600 by default); when it is unset, the examples run discovery first and use the
only connectable instance (one not controlled by another PC), failing when none or several are found:

```bash
GOPXL_IP=192.168.1.10 cargo run --example receive_measurement
```

| SDK sample | Rust example |
| --- | --- |
| `Discover` | `discover` |
| `ConfigureSensor`, `ConfigureTool`, `AlignSensor`, `MultiSensorLayout` | `configure_sensor`, `configure_tool`, `align_sensor`, `multi_sensor_layout` |
| `ReceiveMeasurement`, `ReceiveString`, `ReceiveAsync` | `receive_measurement`, `receive_string`, `receive_async` |
| `ReceiveProfile`, `ReceiveSurface`, `MultilayerOutputs` | `receive_profile`, `receive_surface`, `multilayer_outputs` |
| `ReceiveImage`, `Receive2dImage`, `Acquire2dImage` | `receive_image`, `receive_2d_image`, `acquire_2d_image` |
| `ReceiveMetrics` | `receive_metrics` |
| `SaveJob`, `BackupRestore`, `ReplayData`, `SystemUpgrade` | `save_job`, `backup_restore`, `replay_data`, `system_upgrade` |
| `ResourceCommands`, `ResourceConfigureSensor`, `ResourceConfigureTool`, `ResourceSchema`, `ResourceSubscriptions` (`ResourceApi`) | `resource_commands`, `resource_configure_sensor`, `resource_configure_tool`, `resource_schema`, `resource_subscriptions` |

## Implemented interfaces

The left column lists the public interfaces of the official SDK (8.5.62.52); same-named C++ and .NET interfaces
share a row. The notes column describes only this crate's behavior. Methods of the connection layer, `System` and
`Resource` are `async fn`; the signatures below omit `async` and `.await`.

| SDK interface | Rust interface | Notes |
| --- | --- | --- |
| `GoJson::GoJson()` | `Value::object() -> Value` | `Value::default()` is `Nil` |
| `GoJson::GoJson(const std::string&)`, `GoJson::ParseString` | `str::parse::<Value>()` (`FromStr`, `Err = serde_json::Error`) | Parses JSON text with correctly rounded floats; the integer text `-0` becomes `Float(-0.0)` |
| `GoJson::ToString` | `impl Display for Value` | Compact JSON, `{:#}` for indented output; `Bin` prints as `"<bin N bytes>"` |
| `GoJson::ToBinary` | `value::msgpack::encode(&Value, &mut Vec<u8>) -> Result<(), EncodeError>` | Encoding rules are under "Data types and protocol" below; errors when a length exceeds `u32::MAX` |
| `GoJson::ParseMsgPack` | `value::msgpack::decode(&Bytes) -> Result<Value, DecodeError>` | `Bin` is a zero-copy slice of the input; nesting limit of 128 levels |
| `GoJson::Get<T>`, `At` | `Value::pointer(&self, &str) -> Option<&Value>`; `Index<&str>`, `Index<usize>` (`&Value::Nil` when missing); `as_bool`, `as_i64`, `as_u64`, `as_f64`, `as_str`, `as_array`, `as_map`; any type via `T::deserialize(&value)` (`&Value` implements `serde::Deserializer`) | Reads never modify the document; integer accessors never truncate floats and `as_u64` never wraps negatives; deserialization errors on integer overflow, and `&str` / `&[u8]` borrow without copying |
| `GoJson::GetBinary` | `Value::as_bytes(&self) -> Option<&Bytes>` | No copy |
| `GoJson::Set`, `Replace` | `Value::set(&mut self, &str, impl Into<Value>) -> Result<(), PointerError>` | Only objects are created automatically; arrays take an existing index or `-` to append; the document is unchanged on error |
| `GoJson::Remove` | `Value::remove(&mut self, &str) -> Option<Value>` | Later array elements shift down |
| `GoJson::Merge` | `Value::merge_patch(&mut self, &Value)` | RFC 7396 |
| `GoJson::Contains`, `HasKey`, `Is*`, `Size`, `Empty`, `Begin`, `End`, `Find` | `pointer(..).is_some()`, `is_nil()`, `matches!`, the length and iterators of `as_array()` / `as_map()` |  |
| `GoJson(const GoJson&)`, `operator=`, `operator==`, `operator<<` | `Clone`, assignment of `Value::from(..)`, `PartialEq`, `Display` | Deep copy; every number has exactly one representation, so `PartialEq` is derived |
| `GoJson::Add`, `Copy`, `Move`, `Patch`, `Flatten`, `Unflatten`, `GetHandle` | Not provided | Combine `set` / `remove` |
| `GoRequest::GoRequest(method, uri, content, args)` | `control::Request::new(Method, impl Into<String>)`, `Request::with_payload(Method, impl Into<String>, impl Into<Value>) -> Request`; set the `args` field separately | `payload` and `args` default to `{}` (`0x80` on the wire); the path is sent as is |
| `GoRequest::Method`, `Uri`, `Content`, `Arguments` | The `method`, `path`, `payload`, `args` fields of `Request` |  |
| `GoRequest::ToByteArray` | `Request::encode(&self, &mut Vec<u8>) -> Result<(), EncodeError>` | Writes the whole frame including the carrier header; errors when a length exceeds `u32::MAX`, writing nothing |
| `GoRequest::Id` (.NET `RequestId`) | Not provided | The wire has no request id |
| .NET `IGoCtrlChannel` and its three delegates, `GoRequest(string, string)`, `ToJsonArray`, `Dispose`, `GetHashCode` | Not provided | Method names use the `Method` enum; the wire uses only MessagePack; `Dispose` is drop |
| `GoRequestMethod::ToString` | `Method::as_str` | Also `ListSub`, `ClearSub`, `ListStream`; spellings match the server exactly (`unSub`, `cancelStream`) |
| `GoResponse::Type`, `GoResponseType::FromString`; frame reading in `GoCtrlChannelV1` | `control::Inbound::decode(Bytes) -> Result<Inbound, InboundError>` | An unknown `type` is `Inbound::Other`; frames the server never produces are errors and the connection is closed |
| `GoResponse::Status`, `Path`, `Payload` (`GoRequestResponse`) | The `status`, `path`, `payload` fields of `control::Response` | A missing `status` is a malformed frame; a missing `path` is an empty string and a missing `payload` is `Nil` |
| `GoNotificationResponse::NotificationType`; `GoNotificationType::FromString` | The `event` field (`EventType`) of `control::Notification` | Unknown strings are `EventType::Other` |
| `GoStreamResponse::StreamIdentifier`, `StreamStatus` | The `stream_id` and `state` (`StreamState`) fields of `control::StreamMessage` |  |
| `GoResponse::Raw` | Not provided | The typed fields already carry everything |
| `kStatus_Name` | `control::Status::name`, `is_ok`; `impl Display for Status` | Returns the name from the manual (`"notFound"`) |
| `GoUri::UrlEncode`, `UrlDecode` | `control::url_encode(&str) -> String`, `control::url_decode(&str) -> Vec<u8>` | No `spaceAsPlus`; the decoded result may not be UTF-8, so it returns bytes |
| `GoRestClient::Connect` | `control::Connection::connect(SocketAddr) -> Result<Connection>` | 1 s timeout; sets `TCP_NODELAY`, keepalive (probe every 10 s after 30 s idle) and a 10 s write timeout; port 0 is `InvalidArgument` |
| `GoRestClient::Disconnect` | `drop(connection)` | Pending requests fail with `Disconnected(Closed)`; shuts down and aborts the reader task, never blocks |
| `GoRestClient::IsConnected` | `control::Client::is_connected() -> bool` | `false` once the connection is closed; `Connection::client()` returns a handle that can be cloned and moved to other tasks and does not keep the connection alive |
| `GoRestClient::Read`, `Update`, `Create`, `Delete`, `Call` | `Client::read(&str)`, `update(&str, impl Into<Value>)`, `create(&str, impl Into<Value>)`, `delete(&str)`, `call(&str, impl Into<Value>)`, all returning `Transaction` | The second argument is the wire `payload`; set `args` through `Client::send`; a failed or timed-out write closes the connection |
| — | `Client::send(Request) -> Transaction` | Any method and `args`; `listSub`, `clearSub`, `listStream` are sent through it too; `stream` and `cancelStream` are `InvalidArgument`, see `open_stream` for streams |
| `GoTransaction::GetResponse`, `CheckResponse` | `Transaction::timeout(Duration) -> Transaction`, `response() -> Result<Response>` | The request is written only by `response()`; the default 3 s covers only the wait for the reply; a non-OK `status` is `Error::Request`; a timeout is `Error::Timeout`, a late reply is discarded and the connection stays usable; dropping the `Transaction` sends nothing |
| `GoTransaction::GetRequest`, `GetResponseFuture` | Not provided | The caller holds the request; method and path appear in errors |
| `GoRestClient::Sub`, `UnSub`, `SetSubHandler` | `Client::subscribe(&str) -> Result<Subscription>`; `Subscription::recv_timeout(Duration) -> Result<Option<Notice>>`, `recv() -> Result<Notice>`, `try_recv() -> Result<Option<Notice>>`; `unsubscribe()` waits until `unSub` is written, drop hands it to a task | The caller pulls: one queue per subscription (1024 entries; when full, the oldest is dropped and `Notice::Lagged` is produced, the reader task never waits); receives only the notifications the server delivers for that expression; subscriptions with the same expression share the server subscription, and the last one to unregister writes `unSub` without waiting for the reply; a failed or timed-out `sub` unregisters too |
| `GoRestClient::AddNotificationListener`, `RemoveNotificationListener`, `ReplaceListenerCallback`, `ClearAllListeners` | Not provided | Loop on `recv` of a `Subscription` in your own task; drop removes it |
| `GoRestClient::StartStream`, `StopStream`, `SetStreamHandler` | `Client::open_stream(&str, impl Into<Value>) -> Result<StreamHandle>`; `StreamHandle::recv_timeout(&mut self, Duration) -> Result<Option<StreamMessage>>`, `recv()`, `try_recv()`; drop stops it | One connection per stream, 5 s wait for the reply, a non-OK `status` is `Error::Request`; data arriving before the reply comes first; end messages (`Complete` / `Abort`) are returned as usual; extra responses after the reply are ignored; closing the connection stops the stream, `cancelStream` is never sent; 4 messages are buffered locally, and when full the reader pauses so the server slows down |
| `GoRestClient::SetDisconnectHandler`, `SetReadErrorHandler` | Not provided | After a disconnect `is_connected` is false, and every request, subscription and stream returns `Disconnected(reason)` |
| — | `Client::transfer(Request, Duration) -> Result<Response>` | A temporary connection for file transfers (archive, restore, upgrade, job and replay data) that does not block the main connection; the timeout is the time without receiving any byte; downloaded `Bin` references the response frame |
| `GoSystem::GoSystem(address, port)`, `SetAddress`, `SetControlPort`, `Connect` | `System::connect(SocketAddr) -> Result<System>` | Connected on construction; after a disconnect, requests fail with `Disconnected`; port 0 is `InvalidArgument` |
| `GoSystem::Disconnect`, `~GoSystem` (.NET `Dispose`) | `drop(system)` | Same as `GoRestClient::Disconnect` |
| `GoSystem::Client` | `System::client() -> &Client` | Call `is_connected` through it for `IsConnected` |
| `GoSystem::Address`, `ControlPort` | `Client::addr() -> SocketAddr` |  |
| `GoSystem::Start`, `Stop` | `System::start()`, `stop() -> Result<()>` | `call /system/commands/start` / `stop`, 15 s timeout |
| `GoSystem::RunningState` | `System::running_state() -> Result<RunState>` | Reads `runState` of `/system`, 15 s timeout; values outside the table are `RunState::Other` |
| `GoSystem::EnableQuickEdit`, `DisableQuickEdit`, `QuickEditEnabled` | `System::set_quick_edit(bool) -> Result<()>`, `quick_edit_enabled() -> Result<bool>` | `update` / `read /system` `quickEditEnabled`, 15 s timeout |
| `GoSystem::GdpPort` | `System::gdp_port() -> Result<u16>` | Reads `serverPort` of `/controls/gocator`, 15 s timeout; `Error::Payload` when it does not fit in `u16` |
| — | `System::gdp() -> Result<gdp::Client>` | Connects to the GDP server: same address as the main connection, port from `gdp_port`, 5 s timeout |
| `GoSystem::SensorPath` | `System::sensor_path(&str) -> Result<Option<String>>` | Reads engines, scanners and sensors level by level and compares the string `serialNumber`; `None` when not found |
| `ApiVersion`, `ReleaseVersion` | `API_VERSION` (`"9.0.0"`), `RELEASE_VERSION` (`"1.5"`) |  |
| `BuildVersion`, `BuildVersionNumber`, the `GO_PXL_SDK_*` version number constants (.NET `Sdk*Constants`) | Not provided | Cargo provides the crate version |
| `GoGdpClient::Connect`, `Close` (.NET `Dispose`) | `gdp::Client::connect(SocketAddr) -> Result<gdp::Client>`; drop | 5 s timeout; the reader task decodes message by message; never writes to the socket |
| `GoGdpClient::ReceiveDataSync`, `DataSet`, `ReceiveDataAsync` | `gdp::Client::receive(&mut self, Duration) -> Result<Vec<Message>, RecvError>` | Assembles data sets by `cas.is_last_msg`; one deadline for the whole call; distinguishes `Timeout` from `Incomplete`, keeping received messages for the next call; for asynchronous receiving, loop in your own task; only 4 messages are buffered locally, and when not read the reader pauses so the server slows down; messages with a broken header or CAS are dropped |
| `GoGdpClient::ClearData` | `gdp::Client::clear_data(&mut self) -> Result<()>` | Reconnects, discarding backlogged data and incomplete data sets |
| `GoGdpClient::IsConnected`, `IpAddress`, `Port`; `GoDataSet::Sender`, `SetSender` | Not provided | After a disconnect `receive` returns `RecvError::Disconnected`; the caller supplies the address; data sets come from the `gdp::Client` that `receive` was called on, so the source is known |
| `GoDataSet::GdpMsgAt`, `Count`, `Clear`, `Add` | Indexing, length and methods of `Vec<gdp::Message>` |  |
| `GoDiscoveryClient::BlockingDiscover`, `InstanceList` | `discovery::discover(Duration) -> io::Result<Vec<Instance>>`, `discover_with_classic(Duration)` (also finds Gocator 6.x classic devices) | Sends once on every enabled IPv4 interface (including 169.254.x.x), and a failing interface does not affect the others; resends at 0, 300 and 1000 ms; deduplicates by (address, web port); GoPxL instances first; every call starts from an empty list |
| `GoDiscoveryClient::GoPxLInstanceList`, `ClassicInstanceList`, `Instance`, `GoPxLInstance`, `ClassicInstance` | Filter the result of `discover` by `Instance::classic`, `address`, `web_port`, `serial_number` |  |
| Matching rules of `GoRestClient::Sub` expressions | `control::SubscriptionExpr::new(&str) -> Result<SubscriptionExpr, SubExprError>`, `delivers(&EventType, &str) -> bool`, `text()` | Follows the server rules: `*` crosses `/`, `/a/*` also matches `/a`, `embeddedUpdated` matches ancestors; matches bytes, and `.` does not match `\r` or `\n`; rejects the regex metacharacters `\| ( ) [ ] { } ^ \` and NUL, and also errors when the generated regex does not compile |
| `GoSchemaValidator::SchemaForPath` | `schema::schema_for_path(&Value, &str) -> Option<&Value>` | `None` when the path is not in the schema; paths use the same syntax as `Value::pointer` (`/` is the key `""`); also navigates when `type` is a nullable array |
| `GoSchemaValidator::Validate` | `schema::validate(&Value, &Value) -> Vec<String>` | Each entry is a violated keyword and its bound (`maximum 10`), an empty list means valid; `binary` requires `Bin`, nullable type arrays are supported, `enum` compares integers exactly, an empty `enum` imposes nothing, lengths count characters; `type: "null"` accepts only `Nil` |
| `GoSystem::Resource` | `System::resource(impl Into<String>) -> resource::Resource` | Validation before writes is enabled; every call returns an independent `Resource` with its own cache |
| `GoResource::GoResource(client, uri)` | `Resource::new(Client, impl Into<String>) -> Resource` | Sends no request; 5 s timeout, expand level 0, no validation |
| Copy construction and assignment of `GoResource` | Not provided | The cache and subscription have a single owner; for a second one use `Resource::new(r.client().clone(), r.path())` |
| `GoResource::Uri` | `Resource::path() -> &str` |  |
| `GoResource::SetTimeout`, `Timeout` | `Resource::set_timeout(Duration)`, `timeout()` | 5 s by default |
| `GoResource::SetExpandLevel`, `ExpandLevel` | `Resource::set_expand_level(i32)`, `expand_level()` | Setting it invalidates the data cache, which always matches the current level |
| `GoResource::Data` | `Resource::data() -> Result<&Value>` | Reads from the server when the cache is empty or stale, with read arguments `{expandLevel, includeSchema: false}`; the cache becomes stale after any `create` / `update` / `delete` / `call` (including file transfers) on the same connection, after the subscription receives a notification, or after the connection is closed; force a reread with `invalidate()` then `data()` |
| `GoResource::GetBool`, `GetInt`, `GetInt64`, `GetFloat`, `GetDouble`, `GetString`, `GetObject`, `GetProp` | `Resource::get::<T: Deserialize>(&str) -> Result<T>`, e.g. `get::<i64>(ptr)`, `get::<&str>(ptr)`, `get::<Value>(ptr)` or a `#[derive(Deserialize)]` struct | `Error::Payload` with the reason when the field is missing or cannot be deserialized; integers never truncate floats and error on overflow |
| `GoResource::InvalidateCache` | `Resource::invalidate()` | Drops the data and schema caches |
| `GoResource::HasRemoteChanges` | `Resource::has_remote_changes() -> bool` | Whether the next `data` reads from the server; also `true` before the first read |
| `GoResource::SetBool`, `SetInt`, `SetInt64`, `SetFloat`, `SetDouble`, `SetString`, `SetProp` | `Resource::set(&str, impl Into<Value>) -> Result<()>` | An `update` containing only this value, sent immediately |
| `GoResource::SetJson`, `Update(payload)`; `ScopedUpdate`, `BeginUpdate`, `EndUpdate`, `Flush`, `GoUpdateScope` | `Resource::update(impl Into<Value>) -> Result<()>` | Put several changes into one patch with `Value::set` and send it once; with validation enabled every non-object value in the patch is validated at its location, and a value judged invalid triggers one schema reread before deciding (the schema changes with the device state) |
| `GoResource::CancelUpdate`, `IsUpdateCancelled`, `IsUpdateDeferred`, `DiscardPending`; `GoUpdateScope::Cancel` | Not provided | The caller holds the patch; not sending it cancels it |
| `GoResource::Schema` | `Resource::schema() -> Result<&Value>` | Read arguments `{includeSchema: true}`; updates only the schema cache; `Nil` when the response has no `_schema` |
| `GoResource::SchemaFor` | `schema::schema_for_path(resource.schema()?, &str)` | `None` when not in the schema |
| `GoResource::Validate` | `Resource::validate(&str, &Value) -> Result<Vec<String>>` | An empty list means valid; locations not in the schema are unconstrained |
| `GoResource::EnableValidation`, `IsValidationEnabled` | `Resource::set_validation(bool)`, `validation_enabled()` | Enabled by default for resources from `System::resource` |
| `GoResource::Subscribe`, `Unsubscribe`, `IsSubscribed` | `Resource::subscribe() -> Result<()>`, `unsubscribe()`, `is_subscribed()` | Subscribes to `<path>/*`, so `created` / `deleted` of children arrive; subscribing invalidates the data cache; `extId=` paths and non-canonical paths (not starting with `/`, or with empty segments) receive no notifications (notifications carry canonical paths); unsubscribing drains the queue first; unsubscribe and Drop do not wait for the `unSub` reply |
| `GoResource::Links`, `Embedded`, `CallArguments` | `data()?["_links"]`, `data()?["_embedded"]`, `data()?.pointer("/parameters")` |  |
| `GoResource::ChildUris`, `ChildCount` | `Resource::child_paths(&str) -> Result<Vec<String>>` and its length | rel names are in `resource::rel`; a rel that is a single object is read too |
| `GoResource::Children` | `Resource::children(&str) -> Result<Vec<Resource>>` | Children inherit the timeout and validation settings; with a non-zero expand level the entries go into the children's caches, and each child's expand level is the next level down |
| `GoResource::Child` | `Resource::child(&str) -> Resource` | Sends no request; inherits the timeout and validation settings |
| `GoResource::CreateChild` | `Resource::create_child(impl Into<Value>) -> Result<Resource>` | The new path is the response's `path`, then `_links.self.href`; `Error::Payload` when both are missing |
| `GoResource::Delete`, `DeleteChild` | `Resource::delete(self) -> Result<()>` | Consumes the resource; `notFound` counts as success; for children use resources from `children`, or `Resource::new` on an absolute path from `child_paths` and delete it |
| `GoResource::DeleteAllChildren` | `delete()` each of `children(rel)?` |  |
| `GoResource::Call`, `CallCommand`, `CallAction` | `Resource::call(impl Into<Value>)`, `command(&str, impl Into<Value>)`, `action(&str, impl Into<Value>)`, all returning `Transaction` | The timeout is the resource's, changeable with `Transaction::timeout`; `notFound` is only returned as an error and the resource stays usable |
| `GoResource::CommandNames`, `ActionNames` | `Resource::command_names()`, `action_names() -> Result<Vec<String>>` | Actions read both `go:action` and `go:actions` (the manual's sensor example uses the plural) |
| `GoResource::Read`, `Read(args)`, `Update(payload, args)` | `send(Request)` on `Resource::client()` | Requests with arbitrary arguments bypass the cache |
| `GoResource::IsDeleted`, `IsManaged`; `SetAutoFetchSchema`, `AutoFetchSchema`, `SetAutoInvalidation`, `AutoInvalidation` | Not provided | A deleted resource has been consumed, and when deleted elsewhere the next read returns `notFound`; the schema is read and cached separately; the resource's own writes always make the cache stale |
| `GoResourceManager::GetOrCreate`, `SetAutoValidation`, `AutoValidation` | `System::resource` (validation on), `Resource::new` (validation off) | Several `Resource`s for one path each have a cache, and writes and notifications make them stale together |
| `GoResourceManager::SetAutoSubscribe`, `SubscribeAll`, `UnsubscribeAll`, `EnableValidationAll`, `DisableValidationAll`, `InvalidateAll`, `Remove`, `Cleanup`; `GoSystem::ResourceManager` | Not provided | The caller owns resources: call `subscribe`, `set_validation`, `invalidate` on each resource you hold, and drop it when no longer needed |
| `GoResourceManager::ResetConnectionState` | Not provided | Caches go stale automatically after a disconnect; rebuild the `System` and resources to reconnect |
| `GoResourceManager::SetSubscriptionOptimizedInvalidation`, `SubscriptionOptimizedInvalidation` | Not provided | Subscribed resources also go stale after writes; subscriptions only detect changes by other clients and by the device itself, so reads after a write return new data |
| `GoResourceManager::Client` | `Resource::client() -> &Client` |  |
| `GO_PXL_SDK_DEFAULT_CONTROL_PORT`, `GO_PXL_SDK_DEFAULT_GDP_SERVER_PORT`, `GO_PXL_SDK_DEFAULT_TCP_TIMEOUT_MILLISECONDS` | `control::DEFAULT_PORT`, `gdp::DEFAULT_PORT`; the 1 s timeout for establishing TCP connections (`Connection::connect`, opening streams and file transfers) | 3600, 3601, 1000 ms |
| `GO_PXL_SDK_DEFAULT_WEB_PORT`, `GO_PXL_SDK_MILLISECONDS_TO_MICROSECONDS_CONVERSION`, discovery's `MAX_MESSAGE_SIZE` and similar constants; `GoNotificationType::ToString`, `GoResponseType::ToString`; the `Byte`, `ByteArray`, `SerialNum`, `ResourcePath` aliases | Not provided | The protocol does not use the web port (an instance's is in `Instance::web_port`); timeouts use `Duration`; enums implement `Debug`; the aliases are `u8`, `Vec<u8>`, `String` |
| `GoGdpMsg::Deserialize`; message dispatch in `GoGdpClient` | `gdp::decode_message(Bytes) -> Result<Message, gdp::Error>` | Parses the CAS of every message; damage after the CAS gives `Body::Invalid` |
| — | `gdp::decode_all(Bytes) -> Result<Vec<Message>, gdp::Error>` | Decodes the `bin` of the control channel's `/dataSources` stream |
| `GoGdpMsg::Type` | The `ty` and `body` (`Body`) fields of `gdp::Message` |  |
| `GoGdpMsg::DataSourceId`, `StampSourceId`, `SpaceType`, `HasTransform`, `Transform`, `HasBoundingBox`, `BoundingBox`, `ArrayedCount`, `ArrayedIndex`, `DataSetId`, `IsLastMsg`, `GdpId` | Fields of `gdp::Cas` | Transform and bounding box are `Option`; `gdp_id` is `None` for the short CAS of older servers or 0xFFFF |
| `GoGdpNull::ErrorStatus` | `Body::Null(Status)` |  |
| `GoGdpStamp` getters | Fields of `gdp::Stamp` | `Timestamp` is `timestamp` |
| `GoGdpProfileBase`, `GoGdpProfileUniform`, `GoGdpProfilePointCloud` getters | Fields of `gdp::ProfileAttrs`, `UniformProfile`, `ProfilePointCloud` | `Resolution` / `Offset` are split into `x_scale`, `z_offset` and so on; `Width` and `IntensityWidth` are the lengths of the point array and the intensities; points are a `Vec`, intensities a slice of the frame |
| `GoGdpSurfaceBase`, `GoGdpSurfaceUniform`, `GoGdpSurfacePointCloud` getters | Fields of `gdp::SurfaceAttrs`, `UniformSurface`, `SurfacePointCloud`; `SurfaceAttrs::intensity_row_size` |  |
| `GoGdpImage` getters | Fields of `gdp::Image`; `Image::row_size` | `ColumnBased` is `transposed`; the 1.3 scale / offset are `Option` and are not read when too few bytes remain |
| `GoGdpSpots` getters | Fields of `gdp::Spots` | `PointCount` is `spots.len()` |
| `GoGdpMesh` getters (indexed ones such as `ChannelType(i)`) | Fields of `gdp::Mesh`, `channels[i]` (`MeshChannel`) | Channel data is decoded into `MeshData` by ID, and `AllocatedChannelDataCount` is its length; enough bytes are confirmed before allocating |
| `GoGdpMeasurement`, `GoGdpString` getters | Fields of `gdp::Measurement`, `GdpString` | Label positions are `Option`; strings keep embedded NULs |
| `GoGdpRendering::GetGraphics`; `GoGraphics::*Count`, `*At` | The per-primitive `Vec`s in `Body::Rendering(Rendering)` | Counts are read as `u16`; unknown region types are `Region::Unknown` and skipped; labels also read the 1.5 alignment and color |
| `GoGdpFeaturePoint`, `GoGdpFeatureLine`, `GoGdpFeaturePlane`, `GoGdpFeatureCircle` getters | Fields of `gdp::PointFeature`, `LineFeature`, `PlaneFeature`, `CircleFeature` |  |
| `GoGdpPixelFormat::PixelBits`, `IsNative`, `ToString` | `gdp::PixelFormat::bits`, `is_legacy`, `name` | `PixelBytes` is not provided; it is `bits() / 8` |
| Packet sent by `GoPxLDiscoveryProto::Broadcast` | `discovery::encode_discover() -> [u8; 24]` |  |
| `GoPxLDiscoveryProto::ParseReply`, `GoDiscoveryClient::ParseReply` | `discovery::parse_announce(&[u8]) -> Result<Instance, DiscoveryError>` | Parses only the received bytes; missing, mistyped or unparsable fields take default values |
| Packets sent by `ClassicDiscoveryProto::BroadcastGetIP`, `SendGetInfo` | `discovery::classic::encode_get_ip() -> [u8; 32]`, `encode_get_info(u32) -> [u8; 32]` |  |
| `ClassicDiscoveryProto::ParseClassicGetIPReply`, `ParseClassicGetInfoReply` | `discovery::classic::handle_reply(&[u8], &mut Vec<Instance>) -> Result<Option<u32>, DiscoveryError>` | Returns a new serial number that needs a `GetInfo`; dispatches by `MsgId` |
| `ConstructReceiver`, `ConstructSender`, `Instance`, `GoPxlInstances` / `ClassicInstances` of `GoPxLDiscoveryProto` / `ClassicDiscoveryProto` | Not provided | The sockets and result list inside `discover` |
| `GoInstance::Get*`, `Set*` | Fields of `discovery::Instance` | The address is `Option<Ipv4Addr>`; `serial_number` and `hmi_status` are `Option`; also `classic` and `truncated` |
| — | `Instance::control_addr` | The control channel address; `None` for classic devices, instances controlled by another PC, or instances reporting no port |

## Data types and protocol

| SDK definition or protocol | Rust definition | Notes |
| --- | --- | --- |
| `GoJson`; .NET `JObject` | `Value`: `Nil`, `Bool`, `Int(i64)`, `UInt(u64)`, `Float(f64)`, `Str`, `Bin(Bytes)`, `Array`, `Map`; implements `serde::Serialize` / `Deserialize`, and `&Value` implements `serde::Deserializer` | Value semantics, `Clone` is a deep copy; integers that fit in `i64` are always `Int`, and `UInt` is only for larger values; float32 widens to `f64` on decode; `Nil` maps to serde's unit / `None`, `Bin` to bytes |
| Objects of `GoJson` | `Map = BTreeMap<String, Value>` | Keys in byte order |
| Binary values of `GoJson` | `Value::Bin(Bytes)` | `From<Vec<u8>>` produces `Bin` |
| `GoJsonPointer` | `&str` | RFC 6901; a missing leading `/` is added |
| `GoJsonIterator` | The slice from `as_array()` / `as_map()` and the `BTreeMap` iterators |  |
| `GoJsonError` | `PointerError`, `value::msgpack::DecodeError`, `value::msgpack::EncodeError`, `serde_json::Error`, `serde::de::value::Error` (`T::deserialize(&value)`) | Reads return `Option` and never error |
| Control channel message body (carrier type 0xB000, MessagePack) | `value::msgpack` | Encoding uses the shortest integer formats, float64 for every float, and keys in byte order; decoding accepts any integer width and key order, and treats ext as `Bin` |
| Length-prefixed frames: carrier frames, GDP messages | `frame::FrameReader`, `frame::FrameError`; `control::HEADER_LEN` (14), `gdp::HEADER_LEN` (6) | The little-endian `u32` length includes itself; the length is checked before allocating, and frames over 8 KiB get an exact allocation |
| Carrier frame (the control channel framing: length, type, status and body) | `Request::encode`, `Inbound::decode` | The client sends and receives only type 0xB000 (MessagePack): the server replies in the request's format; `CarrierStatus` is not checked |
| `GoRequestMethod::Method` | `control::Method` |  |
| `GoResponseType` | `control::Inbound`: `Response`, `Notification`, `Stream`, `Other` |  |
| `GoNotificationType` | `control::EventType` | Includes `Other(String)` |
| Stream state strings (`streaming` / `complete` / `abort`) | `control::StreamState` | Includes `Other(String)` |
| `kStatus` (.NET `GoStatus`) | `control::Status(i32)` and all constants (including −975 `OVERHEAT`) | Values outside the table are kept as is |
| `GoCtrlChannel::ReadErrorType` | `frame::FrameError`, `control::InboundError` |  |
| The disconnect reported by `SetDisconnectHandler` | `DisconnectReason`: `Closed`, `Eof`, `Read`, `Write`, `Decode` | All pending requests fail with the same `Arc` reason; stream and GDP connections use it too |
| `GoChannelError`, `GoRequestError` (.NET `GoChannelException`, `GoRequestException`) | `Error`: `Connect`, `Timeout`, `Disconnected`, `Request(Box<RequestError>)`, `Payload`, `Validation`, `InvalidArgument`; `RequestError` carries the method, path and the full `Response` | `Display` includes the first `payload.errors` message; `payload.errors` is not promoted to an error when `status` is OK; `Payload` means a field that `System`, `Resource::get` or `create_child` reads is missing or cannot be deserialized, with `reason` `"missing"` or serde's message; for `Validation` see `GoResourceValidationError` |
| `GoSystem::State` | `RunState`: `Ready`, `Running`, `Conflict`, `Other(i64)` | 0, 1, 2 and values outside the table |
| `GoTransaction` | `control::Transaction` |  |
| Notifications received by `GoNotificationHandler`, `NotificationCallback` | `control::Notice`: `Event(Arc<Notification>)`, `Lagged(u64)` | A notification delivered to several subscriptions is shared; `Lagged` is the number dropped before it |
| `GoStreamResponse` received by `GoStreamHandler` | `control::StreamMessage` (returned by `StreamHandle::recv_timeout`) | End messages are returned as usual: `state` is `Complete` / `Abort` and `status` is the reason; `/dataSources` data may follow right after the end message |
| `GoDataSet` | `Vec<gdp::Message>` | Ends with the `is_last_msg` message |
| Timeout and incompleteness of `ReceiveDataSync` | `gdp::RecvError`: `Timeout`, `Incomplete { messages }`, `Disconnected` |  |
| The server's subscription expressions | `control::SubscriptionExpr`, `control::SubExprError` | Rules are in the `SubscriptionExpr` docs; the regexes are compiled by regex-lite |
| `_schema` (a dialect based on JSON Schema Draft 2019-09) | `Value` | Annotations (`unit`, `visibility`, `extendedType` …) are read directly |
| `GoResourceValidationError::Errors()` | `Vec<String>` | One entry per keyword (`maximum 10`) |
| `GoRelationType` | `resource::rel`: `ITEM`, `SCANNER`, `SUB_TASK`, `CONTENT`, `COMMAND`, `ACTION` | `GoResource::DEFAULT_RELATION_TYPE` is `ITEM`; methods take the rel explicitly |
| `GoResource::DEFAULT_TIMEOUT_MS`, `DEFAULT_EXPAND_LEVEL` | Defaults of `Resource::new`: 5 s, 0 |  |
| `GoResourceError` | `Error::Payload` (a field for `get` is missing or cannot be deserialized; the new path is missing from the `create_child` response) |  |
| `GoResourceValidationError` | `Error::Validation { path, pointer, issues }` | `Errors()` is `issues`; `pointer` locates the invalid value in the patch |
| HAL links (a rel in `_links`: array, single object, `[]`, nil or missing) | `Resource::child_paths`, `children`, `command_names`, `action_names` read them as entry lists | The server writes a single object when there is only one reference |
| `GoGdpMsg`; `MessageType` | `gdp::Message`; the variants of `gdp::Body` | Unknown types are `Body::Unknown(Bytes)`; `GoGdpSignal` is `Body::Signal` |
| `CommonAttributes` | `gdp::Cas` | The common attribute section (CAS) of every GDP message |
| `GoGdpTransform`, `GoGdpBoundingBox` | `Option<[f32; 12]>`, `Option<[f32; 6]>` | Wire order, kept as `f32` |
| Arrays of `kArray1<k16s>`, `kArray2<k16s>`, `kPoint16s`, `kPoint3d16s` (.NET `GoPoint16s`, `GoPoint3d16s`) | `Vec<i16>`, `Vec<[i16; 2]>`, `Vec<[i16; 3]>` | Row-major; invalid values are `i16::MIN` |
| `kPoint3d32f`, `kPoint3d64f`, `kPoint64f` (.NET `GoPoint3d32f`, `GoPoint3d64f`, `GoPoint64f`) | `[f32; 3]`, `[f64; 3]`, `[f64; 2]` | Coordinates of graphics, features, meshes and images; resolutions and offsets of profiles and surfaces are split into fields such as `x_scale`, `z_offset` |
| `kArray1<k8u>`, `kArray2<k8u>` (intensities, image pixels) | `Bytes` | Zero-copy slices of the frame |
| `GoGdpPixelFormat::FormatType` | `gdp::PixelFormat(i32)` and all constants | BayerXX16 uses the GenICam PFNC values starting at 0x0110002E; `bits()` returns the bit count |
| `kCfa` (.NET `GoCfa`) | `Image::color_filter: i32` | 0 none, 1–4 Bayer |
| `GdpSpot` | `gdp::Spot` |  |
| `MeshMsgChannel`, `Channel_ID`, `Channel_Type`, `Channel_State`, `GoFacet32u` | `gdp::MeshChannel` (`id`, `ty`, `state` kept as integers), `gdp::MeshData` (faces are `[u32; 3]`) |  |
| `GoPointSet`, `GoLineSet`, `GoRegion`, `GoPlane`, `GoRay`, `GoLabel`, `GoPosition` of `GoGraphics` | `gdp::PointSet`, `LineSet`, `Region`, `Plane`, `Ray`, `Label`, `Position` |  |
| Missing fields encoded as `k64F_NULL` | `Option` |  |
| `GoInstance` | `discovery::Instance` |  |
| Discovery packets (`DiscoveryBroadcastHeader`, `DiscoveryServerHeader`, `ClassicDiscoveryGetIPCommand`, `ClassicDiscoveryGetIPReply`, `ClassicDiscoveryGetInfoReply`) and the Announce JSON | The `discovery` encode/decode functions and `Instance` fields | Little-endian integers; IPv4 addresses in classic replies are in network byte order |
| Discovery packet parse errors | `discovery::DiscoveryError` | `Command` means a client command was received (including our own loopback) |

## License

MIT. This project references the behavior of the LMI GoPxL SDK, kApi and nlohmann/json (all MIT) and contains no
GoApi code.
