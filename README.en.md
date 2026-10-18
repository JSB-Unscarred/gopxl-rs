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

## Examples

`examples/` ports the official C++ samples with the same requests, order and timeouts; sample defects fixed while
porting are listed at the top of each example. The device address comes from the `GOPXL_IP` environment variable
(`IP` or `IP:port`, port 3600 by default); when it is unset, the examples run discovery first and use the only
connectable (not remotely controlled) instance, failing when there is none or more than one:

```bash
GOPXL_IP=192.168.1.10 cargo run --example receive_measurement
```

| C++ sample | Rust example |
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

Based on the public headers of the official C++ SDK (8.5.62.52); a same-named .NET interface shares the row, with
behavioral differences in the Notes column, and .NET-only interfaces get rows of their own.

| SDK interface | Rust interface | Notes |
| --- | --- | --- |
| `GoJson::GoJson()` | `Value::object() -> Value` | `Value::default()` is `Nil` |
| `GoJson::GoJson(const std::string&)`, `GoJson::ParseString` | `str::parse::<Value>()` (`FromStr`, `Err = serde_json::Error`) | Parses JSON text; floats are correctly rounded, bit-identical to GoJson; the integer text `-0` becomes `Float(-0.0)` (GoJson gives the integer 0) |
| `GoJson::ToString` | `impl Display for Value` | Compact JSON, indented with `{:#}`; `Bin` prints as `"<bin N bytes>"` |
| `GoJson::ToBinary` | `value::msgpack::encode(&Value, &mut Vec<u8>) -> Result<(), EncodeError>` | Byte-identical to nlohmann 3.8.0; errors when a length exceeds `u32::MAX` |
| `GoJson::ParseMsgPack` | `value::msgpack::decode(&Bytes) -> Result<Value, DecodeError>` | `Bin` is a zero-copy slice of the input; nesting is limited to 128 levels |
| `GoJson::Get<T>`, `At` | `Value::pointer(&self, &str) -> Option<&Value>`; `Index<&str>`, `Index<usize>` (`&Value::Nil` when missing); `as_bool`, `as_i64`, `as_u64`, `as_f64`, `as_str`, `as_array`, `as_map`; any type via `T::deserialize(&value)` (`&Value` implements `serde::Deserializer`) | Never modifies the document; integer accessors do not truncate floats, and `as_u64` does not wrap negative numbers; deserialization rejects out-of-range integers (nlohmann truncates), `&str` and `&[u8]` borrow without copying |
| `GoJson::GetBinary` | `Value::as_bytes(&self) -> Option<&Bytes>` | No copy |
| `GoJson::Set`, `Replace` | `Value::set(&mut self, &str, impl Into<Value>) -> Result<(), PointerError>` | Creates only objects automatically; arrays take an existing index or `-` to append; the document is unchanged on error |
| `GoJson::Remove` | `Value::remove(&mut self, &str) -> Option<Value>` | Later array elements shift down, leaving no null |
| `GoJson::Merge` | `Value::merge_patch(&mut self, &Value)` | RFC 7396 |
| `GoJson::Contains`, `HasKey`, `Is*`, `Size`, `Empty`, `Begin`, `End`, `Find` | `pointer(..).is_some()`, `is_nil()`, `matches!`, length and iterators of `as_array()` / `as_map()` |  |
| `GoJson(const GoJson&)`, `operator=`, `operator==`, `operator<<` | `Clone`, assignment of `Value::from(..)`, `PartialEq`, `Display` | Deep copy; every number has exactly one representation, so `PartialEq` is derived |
| `GoJson::Add`, `Copy`, `Move`, `Patch`, `Flatten`, `Unflatten`, `GetHandle` | Not provided | Neither the SDK nor the samples call them; `Add`, `Copy` and `Move` are built on the lossy flatten and are covered by combining `set` / `remove`; `GetHandle` exposes the nlohmann object |
| `GoRequest::GoRequest(method, uri, content, args)` | `control::Request::new(Method, impl Into<String>)`, `Request::with_payload(Method, impl Into<String>, impl Into<Value>) -> Request`; the `args` field is set separately | `payload` and `args` default to `{}` (`0x80` on the wire); the path is sent verbatim |
| `GoRequest::Method`, `Uri`, `Content`, `Arguments` | The `method`, `path`, `payload` and `args` fields of `Request` |  |
| `GoRequest::ToByteArray` | `Request::encode(&self, &mut Vec<u8>) -> Result<(), EncodeError>` | Writes the whole frame including the carrier header, byte-identical to the C++ SDK (.NET `ToMsgPack` gives only the body); errors when the length exceeds `u32::MAX`, writing nothing |
| `GoRequest::Id` (.NET `RequestId`) | Not provided | Only a local counter; there is no request id on the wire |
| .NET `IGoCtrlChannel` and its three delegates, `GoRequest(string, string)`, `ToJsonArray`, `Dispose`, `GetHashCode` | Not provided | The channel is an implementation detail (C++ keeps it under `Internal/`); method names are the `Method` enum; only MessagePack goes on the wire; `Dispose` is drop; `System` is not a map key |
| `GoRequestMethod::ToString` | `Method::as_str` | Adds `ListSub`, `ClearSub`, `ListStream`; the server rejects the all-lowercase .NET spellings (`MsgPackMethod`: `unsub`, `cancelstream`), so the exact spelling is used |
| `GoResponse::Type`, `GoResponseType::FromString`; frame reading in `GoCtrlChannelV1` | `control::Inbound::decode(Bytes) -> Result<Inbound, InboundError>` | An unknown `type` becomes `Inbound::Other` (C++ treats it as a response); frames the server never produces are errors (C++ drops them and the FIFO goes out of step) |
| `GoResponse::Status`, `Path`, `Payload` (`GoRequestResponse`) | The `status`, `path` and `payload` fields of `control::Response` | A missing `status` is a malformed frame (C++ treats it as success); a missing `path` is an empty string, a missing `payload` is `Nil` |
| `GoNotificationResponse::NotificationType`; `GoNotificationType::FromString` | The `event` field (`EventType`) of `control::Notification` | Unknown strings become `EventType::Other` (C++ maps them to `Updated`) |
| `GoStreamResponse::StreamIdentifier`, `StreamStatus` | The `stream_id` and `state` (`StreamState`) fields of `control::StreamMessage` |  |
| `GoResponse::Raw` | Not provided | The typed fields hold the whole content |
| `kStatus_Name` | `control::Status::name`, `is_ok`; `impl Display for Status` | Returns the manual's name (`"notFound"`) |
| `GoUri::UrlEncode`, `UrlDecode` | `control::url_encode(&str) -> String`, `control::url_decode(&str) -> Vec<u8>` | No `spaceAsPlus`; decoding never reads out of bounds and the result may not be UTF-8, so it returns bytes |
| `GoRestClient::Connect` | `control::Client::connect(SocketAddr) -> Result<Client>` | 1 s timeout (1000 s in .NET); sets `TCP_NODELAY`, keepalive (probes every 10 s after 30 s idle) and a 10 s write timeout; port 0 is `InvalidArgument` (C++ passes it straight to the socket) |
| `GoRestClient::Disconnect` | `drop(client)` | Pending requests fail with `Disconnected(Closed)`; only shuts the socket down and never joins the reader thread, so it never blocks (C++ joins before shutting down and can hang) |
| `GoRestClient::IsConnected` | `control::Handle::is_connected() -> bool` | `false` once the connection is torn down (C++ stays true after the connection is lost); `Client` reaches the `Handle` methods through `Deref`, and `Client::handle()` returns a handle that can move to other threads |
| `GoRestClient::Read`, `Update`, `Create`, `Delete`, `Call` | `Handle::read(&str)`, `update(&str, impl Into<Value>)`, `create(&str, impl Into<Value>)`, `delete(&str)`, `call(&str, impl Into<Value>)`, all returning `Transaction` | The second argument is the C++ `content` (the wire `payload`); set `args` through `Handle::send`; the slot is registered and the frame written under the write lock (C++ queues before sending, so a failed send leaves an orphan transaction); a failed or timed-out write drops the connection |
| — | `Handle::send(Request) -> Transaction` | Any method and `args`, including `listSub`, `clearSub` and `listStream` (neither SDK has them); `stream` and `cancelStream` are `InvalidArgument`, see `open_stream` for streams |
| `GoTransaction::GetResponse`, `CheckResponse` | `Transaction::timeout(Duration) -> Transaction`, `response() -> Result<Response>` | 3 s by default, counted from `response()`; a `status` other than OK is `Error::Request`; a timeout is `Error::Timeout`, the slot becomes a tombstone and the connection stays usable; dropping a `Transaction` is fire-and-forget |
| `GoTransaction::GetRequest`, `GetResponseFuture` | Not provided | The caller already has the request; the method and path appear in errors |
| `GoRestClient::Sub`, `UnSub`, `SetSubHandler` | `Handle::subscribe(&str) -> Result<Subscription>`; `Subscription::recv_timeout(Duration) -> Result<Option<Notice>>`; dropping it unsubscribes | Pull-based: one queue per subscription (1024 entries; when full the oldest is dropped and a `Notice::Lagged` is yielded, so the reader thread never blocks); only receives the notifications the server delivers for that expression (the C++ global handler receives all of them); subscriptions with the same expression share one server subscription, and the last one dropped writes `unSub` without waiting for the reply; a failed or timed-out `sub` is unregistered the same way (a `sub` the server runs after the timeout is undone by the following `unSub`) |
| `GoRestClient::AddNotificationListener`, `RemoveNotificationListener`, `ReplaceListenerCallback`, `ClearAllListeners` | Not provided | A callback-style subscription is a loop over `Subscription::recv_timeout` on your own thread, and dropping it removes it (C++ starts a thread per notification, out of order, and runs callbacks under a lock, so re-entering deadlocks) |
| `GoRestClient::StartStream`, `StopStream`, `SetStreamHandler` | `Handle::open_stream(&str, impl Into<Value>) -> Result<StreamHandle>`; `StreamHandle::recv_timeout(Duration) -> Result<Option<StreamMessage>>`; dropping it stops the stream | One connection per stream; waits 5 s for the reply, a `status` other than OK is `Error::Request`; data that arrives before the reply comes first; end messages (`Complete` / `Abort`) are yielded like any other; spurious responses after the reply are ignored (C++ takes them as the next request's reply); closing the connection stops the stream, `cancelStream` is never sent; 4 messages are buffered locally, a slow consumer pushes back on the server |
| `GoRestClient::SetDisconnectHandler`, `SetReadErrorHandler` | Not provided | After a teardown `is_connected` is false and every request, subscription and stream returns `Disconnected(reason)`; only on a real teardown (C++ gives no reason and fires on a single bad message) |
| — | `Handle::transfer(Request, Duration) -> Result<Response>` | A temporary connection for file transfers (archive, restore, upgrade, job and replay data) that does not block the main connection; the timeout bounds the time without any received byte; a downloaded `Bin` refers to the response frame |
| `GoSystem::GoSystem(address, port)`, `SetAddress`, `SetControlPort`, `Connect` | `System::connect(SocketAddr) -> Result<System>` | Connected as long as it exists, there is no "not connected" state; port 0 is `InvalidArgument` |
| `GoSystem::Disconnect`, `~GoSystem` (.NET `Dispose`) | `drop(system)` | Same as `GoRestClient::Disconnect` |
| `GoSystem::Client` | `System::client() -> &Client` | `IsConnected` goes through it as `is_connected` |
| `GoSystem::Address`, `ControlPort` | `Handle::addr() -> SocketAddr` |  |
| `GoSystem::Start`, `Stop` | `System::start()`, `stop() -> Result<()>` | `call /system/commands/start` / `stop`, 15 s timeout |
| `GoSystem::RunningState` | `System::running_state() -> Result<RunState>` | Reads `runState` of `/system`, 15 s timeout; values outside the table are `RunState::Other` (C++ casts them to the enum) |
| `GoSystem::EnableQuickEdit`, `DisableQuickEdit`, `QuickEditEnabled` | `System::set_quick_edit(bool) -> Result<()>`, `quick_edit_enabled() -> Result<bool>` | `update` / `read` of `quickEditEnabled` in `/system`, 15 s timeout |
| `GoSystem::GdpPort` | `System::gdp_port() -> Result<u16>` | Reads `serverPort` of `/controls/gocator`, 15 s timeout; a value outside `u16` is `Error::Payload` (C++ truncates it) |
| `GoSystem::SensorPath` | `System::sensor_path(&str) -> Result<Option<String>>` | Reads engines, scanners and sensors level by level like C++ and compares the string `serialNumber`; `None` when not found (C++ returns `""`) |
| `ApiVersion`, `ReleaseVersion` | `API_VERSION` (`"9.0.0"`), `RELEASE_VERSION` (`"1.5"`) |  |
| `BuildVersion`, `BuildVersionNumber`, the numeric `GO_PXL_SDK_*` version constants (.NET `Sdk*Constants`) | Not provided | Cargo provides the crate version |
| `GoGdpClient::Connect`, `Close` (.NET `Dispose`) | `gdp::GdpClient::connect(SocketAddr) -> Result<GdpClient>`; drop | 5 s timeout; a reader thread decodes message by message; never writes to the socket |
| `GoGdpClient::ReceiveDataSync`, `DataSet`, `ReceiveDataAsync` | `GdpClient::receive(&mut self, Duration) -> Result<Vec<Message>, RecvError>` | Groups messages into a set by `cas.is_last_msg`; one deadline (C++ has no limit once a message is half read); distinguishes `Timeout` from `Incomplete`, keeping received messages for the next call; async reception is a loop on your own thread; only 4 messages are buffered locally, so not reading pushes back on the server (the C++ async queue is unbounded); messages with a broken envelope or CAS are dropped |
| `GoGdpClient::ClearData` | `GdpClient::clear_data(&mut self) -> Result<()>` | Reconnects, discarding backlogged data and the unfinished set |
| `GoGdpClient::IsConnected`, `IpAddress`, `Port`; `GoDataSet::Sender`, `SetSender` | Not provided | After a disconnect `receive` returns `RecvError::Disconnected`; the caller supplies the address; a data set is returned by the `GdpClient` whose `receive` was called, so its origin is known |
| `GoDataSet::GdpMsgAt`, `Count`, `Clear`, `Add` | Indexing, length and methods of `Vec<gdp::Message>` |  |
| `GoDiscoveryClient::BlockingDiscover`, `InstanceList` | `discovery::discover(Duration, bool) -> io::Result<Vec<Instance>>` | Sends once from every enabled IPv4 interface (including 169.254.x.x), and one failing interface does not affect the others; resends at 0, 300 and 1000 ms (C++ sends once); deduplicates by (address, web port); GoPxL instances first; every call starts from a fresh state (the C++ classic list is never cleared) |
| `GoDiscoveryClient::GoPxLInstanceList`, `ClassicInstanceList`, `Instance`, `GoPxLInstance`, `ClassicInstance` | Filter the result of `discover` by `Instance::classic`, `address`, `web_port`, `serial_number` |  |
| Matching rules of a `GoRestClient::Sub` expression | `control::SubscriptionExpr::new(&str) -> Result<SubscriptionExpr, SubExprError>`, `delivers(&EventType, &str) -> bool`, `text()` | Server rules: `*` crosses `/`, `/a/*` also matches `/a`, `embeddedUpdated` matches ancestors (C++ uses prefix and ancestor walks that disagree with the server); like the server, matching is byte-wise and `.` does not match `\r` or `\n`; regex metacharacters `\| ( ) [ ] { } ^ \` and NUL (the server truncates the expression at a NUL) are rejected, and so is an expression whose generated regex does not compile |
| `GoSchemaValidator::SchemaForPath` | `schema::schema_for_path(&Value, &str) -> Option<&Value>` | `None` when the path is not in the schema (C++ throws `GoResourceError`); paths use the same syntax as `Value::pointer` (`/` is the key `""`, which C++ treats as the root); also navigates when `type` is a nullable array |
| `GoSchemaValidator::Validate` | `schema::validate(&Value, &Value) -> Vec<String>` | Each entry is the violated keyword and its limit (`maximum 10`); an empty list means valid, as with the C++ `vector<string>`; `binary` requires `Bin`, nullable type arrays are supported, `multipleOf` is correct for negative values, integers in `enum` compare exactly, an empty `enum` imposes no constraint, lengths count characters; `type: "null"` accepts only `Nil` (C++ does not know it and accepts every value) |
| `GoSystem::Resource` | `System::resource(impl Into<String>) -> resource::Resource` | Validation before writes is on (as with `GoSystem`'s resource manager); every call returns an independent `Resource` with its own cache |
| `GoResource::GoResource(client, uri)` | `Resource::new(Handle, impl Into<String>) -> Resource` | Sends nothing; 5 s timeout, expand level 0, no validation |
| `GoResource` copy constructor and assignment | Not provided | The cache and the subscription have one owner; for a second one use `Resource::new(r.handle().clone(), r.path())` |
| `GoResource::Uri` | `Resource::path() -> &str` |  |
| `GoResource::SetTimeout`, `Timeout` | `Resource::set_timeout(Duration)`, `timeout()` | 5 s by default |
| `GoResource::SetExpandLevel`, `ExpandLevel` | `Resource::set_expand_level(i32)`, `expand_level()` | Setting it drops the data cache, so the cache always matches the current level |
| `GoResource::Data` | `Resource::data() -> Result<&Value>` | Reads the server when the cache is empty or stale, with read args `{expandLevel, includeSchema: false}`; the cache goes stale after any `create` / `update` / `delete` / `call` on the same connection (file transfers included), after a notification on the subscription, or after the connection is lost (replacing C++ `SetNonIdempotentRequestHandler` and `ResetConnectionState`); C++ `Cache` (always re-reads) is `invalidate()` followed by `data()` |
| `GoResource::GetBool`, `GetInt`, `GetInt64`, `GetFloat`, `GetDouble`, `GetString`, `GetObject`, `GetProp` | `Resource::get::<T: Deserialize>(&str) -> Result<T>`, e.g. `get::<i64>(ptr)`, `get::<&str>(ptr)`, `get::<Value>(ptr)` or a `#[derive(Deserialize)]` struct | `Error::Payload` with a reason when the field is missing or cannot be deserialized (C++ throws `GoResourceError`); integers do not truncate floats and out-of-range values are errors |
| `GoResource::InvalidateCache` | `Resource::invalidate()` | Drops the data and schema caches |
| `GoResource::HasRemoteChanges` | `Resource::has_remote_changes() -> bool` | Whether the next `data` reads the server; also `true` before the first read |
| `GoResource::SetBool`, `SetInt`, `SetInt64`, `SetFloat`, `SetDouble`, `SetString`, `SetProp` | `Resource::set(&str, impl Into<Value>) -> Result<()>` | An `update` with just this value, sent immediately |
| `GoResource::SetJson`, `Update(payload)`; `ScopedUpdate`, `BeginUpdate`, `EndUpdate`, `Flush`, `GoUpdateScope` | `Resource::update(impl Into<Value>) -> Result<()>` | Put several changes into one patch with `Value::set` and send it once; with validation on, every non-object value in the patch is checked against the schema node at its position (as `Set*` does; C++ `SetJson` does not validate), and a value judged invalid triggers one schema re-read first (the schema changes with device state; C++ only uses the cached schema) |
| `GoResource::CancelUpdate`, `IsUpdateCancelled`, `IsUpdateDeferred`, `DiscardPending`; `GoUpdateScope::Cancel` | Not provided | The patch stays in the caller's hands, so not sending it cancels it; there is no shared pending patch (C++ `Flush` loses the patch on failure, and a scope's exception path clears the outer scope's changes) |
| `GoResource::Schema` | `Resource::schema() -> Result<&Value>` | Read args `{includeSchema: true}`; updates only the schema cache (C++ overwrites an expanded data cache with unexpanded data); `Nil` when there is no `_schema` |
| `GoResource::SchemaFor` | `schema::schema_for_path(resource.schema()?, &str)` | `None` when the path is not in the schema (C++ throws `GoResourceError`) |
| `GoResource::Validate` | `Resource::validate(&str, &Value) -> Result<Vec<String>>` | An empty list means valid; positions outside the schema are unconstrained |
| `GoResource::EnableValidation`, `IsValidationEnabled` | `Resource::set_validation(bool)`, `validation_enabled()` | On by default for resources from `System::resource` |
| `GoResource::Subscribe`, `Unsubscribe`, `IsSubscribed` | `Resource::subscribe() -> Result<()>`, `unsubscribe()`, `is_subscribed()` | Subscribes `<path>/*`, so children's `created` / `deleted` arrive (C++ subscribes the exact path); subscribing drops the data cache; `extId=` paths and non-canonical paths (not starting with `/`, empty segments) receive no notifications (notifications carry canonical paths); unsubscribing drains the queue first; unsubscribing and drop do not wait for the `unSub` reply (the C++ destructor blocks up to 5 s) |
| `GoResource::Links`, `Embedded`, `CallArguments` | `data()?["_links"]`, `data()?["_embedded"]`, `data()?.pointer("/parameters")` |  |
| `GoResource::ChildUris`, `ChildCount` | `Resource::child_paths(&str) -> Result<Vec<String>>` and its length | Relation names are in `resource::rel`; a relation that is a single object is read too (C++ returns nothing) |
| `GoResource::Children` | `Resource::children(&str) -> Result<Vec<Resource>>` | Children inherit the timeout and validation settings; with a non-zero expand level the entries go into the children's caches and the children get the next level (C++ guesses expansion from the key count and caches links that carry `ordIndex` as data) |
| `GoResource::Child` | `Resource::child(&str) -> Resource` | Sends nothing; inherits the timeout and validation settings |
| `GoResource::CreateChild` | `Resource::create_child(impl Into<Value>) -> Result<Resource>` | The new path is the response's `path`, else `_links.self.href`; `Error::Payload` when neither exists |
| `GoResource::Delete`, `DeleteChild` | `Resource::delete(self) -> Result<()>` | Consumes the resource; `notFound` also counts as success; delete children through resources from `children`, or `Resource::new` on an absolute path from `child_paths` |
| `GoResource::DeleteAllChildren` | `delete()` each of `children(rel)?` |  |
| `GoResource::Call`, `CallCommand`, `CallAction` | `Resource::call(impl Into<Value>)`, `command(&str, impl Into<Value>)`, `action(&str, impl Into<Value>)`, all returning `Transaction` | The timeout is the resource's, changeable with `Transaction::timeout`; `notFound` does not mark the resource deleted (C++ marks the parent when a command or action fails) |
| `GoResource::CommandNames`, `ActionNames` | `Resource::command_names()`, `action_names() -> Result<Vec<String>>` | Actions read both `go:action` and `go:actions` (the manual's sensor example uses the plural) |
| `GoResource::Read`, `Read(args)`, `Update(payload, args)` | `send(Request)` on `Resource::handle()` | Requests with arbitrary args do not touch the cache |
| `GoResource::IsDeleted`, `IsManaged`; `SetAutoFetchSchema`, `AutoFetchSchema`, `SetAutoInvalidation`, `AutoInvalidation` | Not provided | A deleted resource has been consumed, and a deletion elsewhere shows up as `notFound` on the next read; there is no manager; the schema is read and cached separately; the resource's own writes always make the cache stale (turning off C++ `AutoInvalidation` only affects subscribed resources, which then return old data) |
| `GoResourceManager::GetOrCreate`, `SetAutoValidation`, `AutoValidation` | `System::resource` (validation on), `Resource::new` (no validation) | No identity map: the write counter and subscriptions make each cache go stale on its own |
| `GoResourceManager::SetAutoSubscribe`, `SubscribeAll`, `UnsubscribeAll`, `EnableValidationAll`, `DisableValidationAll`, `InvalidateAll`, `Remove`, `Cleanup`; `GoSystem::ResourceManager` | Not provided | Resources are owned by the caller and there is no central registry: call `subscribe`, `set_validation` or `invalidate` on the resources you hold, and drop them when no longer needed |
| `GoResourceManager::ResetConnectionState` | Not provided | Caches go stale automatically when the connection is lost; there is no reconnect, so recreate the `System` and the resources |
| `GoResourceManager::SetSubscriptionOptimizedInvalidation`, `SubscriptionOptimizedInvalidation` | Not provided | Subscribed resources also go stale with the write counter (as if C++ turned the optimization off); subscriptions only detect changes by other clients and the device itself, so a write is never followed by data older than its notification |
| `GoResourceManager::Client` | `Resource::handle() -> &Handle` |  |
| `GO_PXL_SDK_DEFAULT_CONTROL_PORT`, `GO_PXL_SDK_DEFAULT_GDP_SERVER_PORT`, `GO_PXL_SDK_DEFAULT_TCP_TIMEOUT_MILLISECONDS` | `control::DEFAULT_PORT`, `gdp::DEFAULT_PORT`; the 1 s timeout for establishing a TCP connection (`Client::connect`, streams and file transfers) | 3600, 3601, 1000 ms |
| `GO_PXL_SDK_DEFAULT_WEB_PORT`, `GO_PXL_SDK_MILLISECONDS_TO_MICROSECONDS_CONVERSION`, discovery constants such as `MAX_MESSAGE_SIZE`; `GoNotificationType::ToString`, `GoResponseType::ToString`; the `Byte`, `ByteArray`, `SerialNum`, `ResourcePath` aliases | Not provided | The protocol never uses the web port (an instance's is `Instance::web_port`); timeouts are `Duration`s; the enums have `Debug`; the aliases are `u8`, `Vec<u8>`, `String` |
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
| `GoPxLDiscoveryProto::ParseReply`, `GoDiscoveryClient::ParseReply` | `discovery::parse_announce(&[u8]) -> Result<Instance, DiscoveryError>` | Parses only the received bytes; an ill-typed or unparsable field takes its default (C++ drops the whole Announce), a missing one takes its default as in C++ |
| The datagrams sent by `ClassicDiscoveryProto::BroadcastGetIP`, `SendGetInfo` | `discovery::classic::encode_get_ip() -> [u8; 32]`, `encode_get_info(u32) -> [u8; 32]` |  |
| `ClassicDiscoveryProto::ParseClassicGetIPReply`, `ParseClassicGetInfoReply` | `discovery::classic::handle_reply(&[u8], &mut Vec<Instance>) -> Result<Option<u32>, DiscoveryError>` | Returns a new serial that needs a `GetInfo`; dispatches by `MsgId` (C++ dispatches by length and drops 99-byte `GetInfo` replies) |
| `ConstructReceiver`, `ConstructSender`, `Instance`, `GoPxlInstances` / `ClassicInstances` of `GoPxLDiscoveryProto` / `ClassicDiscoveryProto` | Not provided | The sockets and result list inside `discover` |
| `GoInstance::Get*`, `Set*` | The fields of `discovery::Instance` | Addresses are `Option<Ipv4Addr>`; `serial_number` and `hmi_status` are `Option`s (C++ defaults to 0 / 1); adds `classic` and `truncated` |
| — | `Instance::control_addr` | The connect check of the SDK sample `Discover.cpp` |

## Data types and protocol

| SDK definition or protocol | Rust definition | Notes |
| --- | --- | --- |
| `GoJson` (nlohmann::json 3.8.0); .NET `JObject` | `Value`: `Nil`, `Bool`, `Int(i64)`, `UInt(u64)`, `Float(f64)`, `Str`, `Bin(Bytes)`, `Array`, `Map`; implements `serde::Serialize` / `Deserialize`, and `&Value` implements `serde::Deserializer` | Value semantics, `Clone` is a deep copy; every integer that fits in `i64` is `Int`, `UInt` is only for larger values; float32 is widened to `f64` when decoding; `Nil` maps to serde's unit / `None`, `Bin` to bytes |
| nlohmann object (`std::map`) | `Map = BTreeMap<String, Value>` | Keys in byte order, so encoding order matches the C++ SDK |
| nlohmann `binary_t` | `Value::Bin(Bytes)` | `From<Vec<u8>>` produces `Bin`, like `GoJson::Set(path, std::vector<k8u>)` |
| `GoJsonPointer` | `&str` | RFC 6901; like `GoJsonPointer`, a missing leading `/` is added |
| `GoJsonIterator` | The slice and `BTreeMap` iterators returned by `as_array()` / `as_map()` |  |
| `GoJsonError` | `PointerError`, `value::msgpack::DecodeError`, `value::msgpack::EncodeError`, `serde_json::Error`, `serde::de::value::Error` (`T::deserialize(&value)`) | Reads return `Option` and never fail |
| Control channel message body (carrier type 0xB000, MessagePack) | `value::msgpack` | Encoding is byte-identical to nlohmann 3.8.0 (shortest integers, float64, keys in byte order); decoding does not depend on integer width or key order, and ext is read as `Bin` |
| Length-prefixed frames: carrier frames, GDP messages | `frame::FrameReader`, `frame::FrameError`; `control::HEADER_LEN` (14), `gdp::HEADER_LEN` (6) | The little-endian `u32` length includes itself; the length is checked before an exact allocation; `WouldBlock` loses no bytes already read |
| Carrier frame (`GoCtrlChannelV1`) | `Request::encode`, `Inbound::decode` | The client sends and receives only 0xB000: the server replies in the request's format, and the C++ SDK also accepts only 0xB000; like C++, `CarrierStatus` is not checked |
| `GoRequestMethod::Method` | `control::Method` |  |
| `GoResponseType` | `control::Inbound`: `Response`, `Notification`, `Stream`, `Other` |  |
| `GoNotificationType` | `control::EventType` | Includes `Other(String)` |
| Stream status strings (`streaming` / `complete` / `abort`) | `control::StreamState` | Includes `Other(String)` |
| `kStatus` (.NET `GoStatus`) | `control::Status(i32)` with every constant (including −975 `OVERHEAT`) | Values outside the table are kept as is; the .NET enum lacks −975 |
| `GoCtrlChannel::ReadErrorType` | `frame::FrameError`, `control::InboundError` |  |
| Disconnects reported through `SetDisconnectHandler` (C++ gives no reason) | `control::DisconnectReason`: `Closed`, `Eof`, `Read`, `Write`, `Decode` | Every pending request fails with the same `Arc` reason; stream and GDP connections report their disconnects with it too |
| `GoChannelError`, `GoRequestError` (.NET `GoChannelException`, `GoRequestException`) | `Error`: `Connect`, `Timeout`, `Disconnected`, `Request(Box<RequestError>)`, `Payload`, `Validation`, `InvalidArgument`; `RequestError` carries the method, path and full `Response` | `Display` includes the description of the first `payload.errors` entry; with an OK `status`, `payload.errors` is not turned into an error (same as the SDK); `Payload` means a field `System`, `Resource::get` or `create_child` reads is missing or cannot be deserialized, with `reason` being `"missing"` or serde's message (C++ throws `GoJsonError` / `GoResourceError`); for `Validation` see `GoResourceValidationError` |
| `GoSystem::State` | `RunState`: `Ready`, `Running`, `Conflict`, `Other(i64)` | 0, 1, 2 and values outside the table |
| `GoTransaction` | `control::Transaction` |  |
| Notifications received by a `GoNotificationHandler` or `NotificationCallback` | `control::Notice`: `Event(Arc<Notification>)`, `Lagged(u64)` | A notification delivered to several subscriptions is shared; `Lagged` is the number dropped before this point |
| `GoStreamResponse` received by a `GoStreamHandler` | `control::StreamMessage` (taken with `StreamHandle::recv_timeout`) | End messages are yielded like any other: `state` is `Complete` / `Abort` and `status` is the end reason; `/dataSources` data may arrive right after the end message (each round the server writes the response queue before the stream queue) |
| `GoDataSet` | `Vec<gdp::Message>` | Ends with the `is_last_msg` message |
| `ReceiveDataSync` timeout and incompleteness | `gdp::RecvError`: `Timeout`, `Incomplete { messages }`, `Disconnected` |  |
| The server's `SubscriptionExpression` | `control::SubscriptionExpr`, `control::SubExprError` | Both regexes (R, E) are ported verbatim and compiled with regex-lite |
| `_schema` (a dialect based on JSON Schema Draft 2019-09) | `Value` | Annotations (`unit`, `visibility`, `extendedType` …) are read directly |
| `GoResourceValidationError::Errors()` | `Vec<String>` | One entry per keyword (`maximum 10`), as with the C++ `vector<string>` |
| `GoRelationType` | `resource::rel`: `ITEM`, `SCANNER`, `SUB_TASK`, `CONTENT`, `COMMAND`, `ACTION` | `GoResource::DEFAULT_RELATION_TYPE` is `ITEM`; Rust methods take the relation explicitly |
| `GoResource::DEFAULT_TIMEOUT_MS`, `DEFAULT_EXPAND_LEVEL` | The defaults of `Resource::new`: 5 s, 0 |  |
| `GoResourceError` | `Error::Payload` (a field `get` reads is missing or cannot be deserialized, or the `create_child` response has no new path) | The other C++ throw sites (deleted resource, missing manager, unbalanced `EndUpdate`) do not exist in Rust |
| `GoResourceValidationError` | `Error::Validation { path, pointer, issues }` | `Errors()` is `issues`; `pointer` is the position of the invalid value in the patch |
| HAL relations (array, single object, `[]`, nil or absent) | `Resource::child_paths`, `children`, `command_names` and `action_names` read them as a list of entries | The server writes a single object when there is exactly one reference |
| `GoGdpMsg`; `MessageType` | `gdp::Message`; the variants of `gdp::Body` | Unknown types become `Body::Unknown(Bytes)`; `GoGdpSignal` has no fields and is `Body::Signal` |
| `CommonAttributes` | `gdp::Cas` |  |
| `GoGdpTransform`, `GoGdpBoundingBox` | `Option<[f32; 12]>`, `Option<[f32; 6]>` | Wire order, kept as `f32` |
| Arrays of `kArray1<k16s>`, `kArray2<k16s>`, `kPoint16s`, `kPoint3d16s` (.NET `GoPoint16s`, `GoPoint3d16s`) | `Vec<i16>`, `Vec<[i16; 2]>`, `Vec<[i16; 3]>` | Row-major; `k16S_NULL` is `i16::MIN` |
| `kPoint3d32f`, `kPoint3d64f`, `kPoint64f` (.NET `GoPoint3d32f`, `GoPoint3d64f`, `GoPoint64f`) | `[f32; 3]`, `[f64; 3]`, `[f64; 2]` | Coordinates of graphics, features, meshes and images; profile and surface resolution / offset are split into fields such as `x_scale`, `z_offset` |
| `kArray1<k8u>`, `kArray2<k8u>` (intensity, image pixels) | `Bytes` | Zero-copy slices of the frame |
| `GoGdpPixelFormat::FormatType` | `gdp::PixelFormat(i32)` with every constant | BayerXX16 uses the PFNC values from 0x0110002E (the .NET values are wrong); `ColorType` and `PixelSize` are only bit fields of the value and are not provided separately, `bits()` reads the size |
| `kCfa` (.NET `GoCfa`) | `Image::color_filter: i32` | 0 none, 1–4 Bayer |
| `GdpSpot` | `gdp::Spot` |  |
| `MeshMsgChannel`, `Channel_ID`, `Channel_Type`, `Channel_State`, `GoFacet32u` | `gdp::MeshChannel` (`id`, `ty`, `state` kept as integers), `gdp::MeshData` (facets are `[u32; 3]`) |  |
| `GoPointSet`, `GoLineSet`, `GoRegion`, `GoPlane`, `GoRay`, `GoLabel`, `GoPosition` of `GoGraphics` | `gdp::PointSet`, `LineSet`, `Region`, `Plane`, `Ray`, `Label`, `Position` |  |
| Missing fields marked with `k64F_NULL` | `Option` |  |
| `GoInstance` | `discovery::Instance` |  |
| Discovery datagrams (`DiscoveryBroadcastHeader`, `DiscoveryServerHeader`, `ClassicDiscoveryGetIPCommand`, `ClassicDiscoveryGetIPReply`, `ClassicDiscoveryGetInfoReply`) and the Announce JSON | The codec functions of `discovery` and the fields of `Instance` | Little-endian integers; IPv4 addresses in classic replies are in network byte order |
| Discovery parse errors | `discovery::DiscoveryError` | `Command` means the datagram is a client command (including our own loopback) |

## License

MIT. This project references the behavior of the LMI GoPxL SDK, kApi and nlohmann/json (all MIT) and contains no
GoApi code.
