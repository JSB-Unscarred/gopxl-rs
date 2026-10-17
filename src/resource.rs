//! 资源层：[`Resource`] 是一个资源路径加它的读缓存，对应 C++ 的 `GoResource`。在原始动词之上缓存读结果、写入前按
//! schema 校验、沿 HAL 链接取子资源与命令；所有权与缓存何时过期见 [`docs::architecture`](crate::docs::architecture)。
//!
//! 写入就是发送一个补丁：[`Resource::update`] 发送调用方构建的补丁，[`Resource::set`] 是只含一个值的补丁。没有
//! C++ 的延迟写与 `GoUpdateScope`：它们共享一个待发补丁，`Flush` 失败时补丁丢失，作用域的异常路径清掉外层作用域
//! 的改动（重写方案 §5.4 第 35 条）。

use std::fmt;
use std::slice;
use std::time::Duration;

use crate::control::{Handle, Method, Request, Status, Subscription, Transaction};
use crate::error::{Error, Result};
use crate::schema::{self, Issue};
use crate::value::{Map, Value};

/// HAL 关系名，对应 C++ 的 `GoRelationType`。
pub mod rel {
    /// 集合的条目。
    pub const ITEM: &str = "item";
    /// 引擎下的扫描器。
    pub const SCANNER: &str = "go:scanner";
    /// 子任务。
    pub const SUB_TASK: &str = "go:subTask";
    /// 内容。
    pub const CONTENT: &str = "go:content";
    /// 命令（`_links` 中）。
    pub const COMMAND: &str = "go:command";
    /// 动作（`_links` 中）。
    pub const ACTION: &str = "go:action";
}

/// 请求的默认超时（C++ 的 `DEFAULT_TIMEOUT_MS`）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// 一个资源路径及其缓存，对应 C++ 的 `GoResource`。由 [`System::resource`](crate::System::resource)、
/// [`Resource::new`] 或父资源的 `child`、`children`、`create_child` 得到。
///
/// owned、`Send`，唯一拥有自己的缓存与订阅。Drop 时订阅随之释放，写出 `unSub`、不等应答（C++ 的析构等待应答，
/// 最多阻塞 5 s，§5.4 第 44 条）。不让连接存活：`System` Drop 之后，请求以 `Disconnected` 失败。
pub struct Resource {
    handle: Handle,
    path: String,
    timeout: Duration,
    expand_level: i32,
    validation: bool,
    /// 读缓存；`None` 时下一次访问读服务端。
    data: Option<Value>,
    /// 发出读取 `data` 的请求之前连接的写计数（`Handle::write_count`）。
    epoch: u64,
    /// `_schema` 缓存，与 `data` 分开（C++ 读 schema 时用不展开的数据覆盖数据缓存，§5.4 第 36 条）。
    schema: Option<Value>,
    subscription: Option<Subscription>,
}

impl Resource {
    /// 路径 `path` 上的资源，不发请求，对应 C++ 的 `GoResource(client, uri)`：超时 5 s、展开层级 0、不校验。
    /// 路径原样发送；订阅要求规范路径（以 `/` 开头、没有空段），见 [`subscribe`](Self::subscribe)。
    pub fn new(handle: Handle, path: impl Into<String>) -> Self {
        Self {
            handle,
            path: path.into(),
            timeout: DEFAULT_TIMEOUT,
            expand_level: 0,
            validation: false,
            data: None,
            epoch: 0,
            schema: None,
            subscription: None,
        }
    }

    /// 资源路径，对应 C++ 的 `Uri`。
    pub fn path(&self) -> &str {
        &self.path
    }

    /// 所在的连接，对应 C++ 的 `GoResourceManager::Client`；原始动词与读参数经它发送。
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// 本资源各请求的超时，对应 C++ 的 `SetTimeout`。
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// 本资源各请求的超时，对应 C++ 的 `Timeout`。
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// 读取时的 `expandLevel`（0 只有子项的链接，1 展开一层，−1 展开全部），对应 C++ 的 `SetExpandLevel`。缓存随之
    /// 失效，所以缓存总是按当前层级读到的，[`children`](Self::children) 据此判断条目是否已展开。
    pub fn set_expand_level(&mut self, level: i32) {
        self.expand_level = level;
        self.data = None;
    }

    /// 读取时的 `expandLevel`，对应 C++ 的 `ExpandLevel`。
    pub fn expand_level(&self) -> i32 {
        self.expand_level
    }

    /// 开启或关闭写入前的 schema 校验，对应 C++ 的 `EnableValidation`。
    pub fn set_validation(&mut self, enabled: bool) {
        self.validation = enabled;
    }

    /// 是否在写入前校验，对应 C++ 的 `IsValidationEnabled`。
    pub fn validation_enabled(&self) -> bool {
        self.validation
    }

    /// 资源的数据，对应 C++ 的 `Data` 与各 `Get*`。缓存为空或过期时读服务端，读参数为
    /// `{"expandLevel": n, "includeSchema": false}`；字段经 [`Value::pointer`] 与 `as_*` 读取。C++ 的 `Cache`（总是
    /// 重读）即 [`invalidate`](Self::invalidate) 之后 `data`。
    pub fn data(&mut self) -> Result<&Value> {
        self.refresh();
        let data = if let Some(data) = self.data.take() {
            data
        } else {
            // 先取计数再发请求，见 `Handle::write_count`。
            self.epoch = self.handle.write_count();
            self.read(Map::from([
                ("expandLevel".to_owned(), Value::from(self.expand_level)),
                ("includeSchema".to_owned(), Value::Bool(false)),
            ]))?
        };
        Ok(self.data.insert(data))
    }

    /// `pointer` 处的字段经 `convert` 转换的结果，对应 C++ 的 `GetBool`、`GetInt`、`GetDouble`、`GetString`、`GetProp`
    /// 等：例如 `get("/parameters/x", Value::as_i64)`，原样取值用 `Some`。字段缺失或 `convert` 返回 `None`（类型不对）
    /// 时报 `Payload`（C++ 抛 `GoResourceError`）。
    pub fn get<'a, T>(
        &'a mut self,
        pointer: &str,
        convert: impl FnOnce(&'a Value) -> Option<T>,
    ) -> Result<T> {
        let path = self.path.clone();
        let value = self.data()?.pointer(pointer).and_then(convert);
        value.ok_or_else(|| Error::Payload {
            path,
            field: pointer.to_owned(),
        })
    }

    /// 下一次 [`data`](Self::data) 是否会读服务端（缓存为空，或因变更请求、通知而过期），对应 C++ 的
    /// `HasRemoteChanges`；从未读取时也为 `true`。
    pub fn has_remote_changes(&mut self) -> bool {
        self.refresh();
        self.data.is_none()
    }

    /// 丢弃数据与 schema 缓存，对应 C++ 的 `InvalidateCache`。
    pub fn invalidate(&mut self) {
        self.data = None;
        self.schema = None;
    }

    /// 发送补丁 `patch`（`update`），对应 C++ 的 `SetJson`、`Update` 与延迟写的 `Flush`；多个修改用 [`Value::set`]
    /// 写进同一个补丁，一次发出。服务端整体替换数组，修改数组元素时写入整个数组。
    ///
    /// 开启校验时，补丁中对象以外的每个值按所在位置的 schema 节点校验（与 C++ 的 `Set*` 相同；C++ 的 `SetJson` 不校验），
    /// 第一个不合格的值报 `Validation`，请求不发出。补丁只放要修改的字段：读到的整份数据含只读字段，原样写回会被拒绝。
    pub fn update(&mut self, patch: impl Into<Value>) -> Result<()> {
        let patch = patch.into();
        if self.validation {
            let cached = self.schema.is_some();
            let mut invalid = first_invalid(self.schema()?, &patch, "");
            // schema 随设备状态变化（例如 materialType 决定哪些参数只读），缓存的 schema 判为不合格时重读一次再判，
            // 免得按过期的约束拒绝服务端接受的写入；合格或刚读到时不多发请求。
            if invalid.is_some() && cached {
                self.schema = None;
                invalid = first_invalid(self.schema()?, &patch, "");
            }
            if let Some((pointer, issues)) = invalid {
                return Err(Error::Validation {
                    path: self.path.clone(),
                    pointer,
                    issues,
                });
            }
        }
        self.handle
            .update(&self.path, patch)
            .timeout(self.timeout)
            .response()?;
        Ok(())
    }

    /// 把 `pointer` 处设为 `value` 并发送，对应 C++ 的各 `Set*`；即只含这一个值的 [`update`](Self::update)，没有延迟写。
    pub fn set(&mut self, pointer: &str, value: impl Into<Value>) -> Result<()> {
        let mut patch = Value::object();
        // 从空对象出发只会创建对象，不会失败。
        let _ = patch.set(pointer, value);
        self.update(patch)
    }

    /// 资源的 `_schema`，对应 C++ 的 `Schema`。缓存为空时读一次（读参数 `{"includeSchema": true}`），只更新 schema
    /// 缓存；响应没有 `_schema` 时为 `Nil`。子节点经 [`schema::schema_for_path`] 取得（C++ 的 `SchemaFor`）。
    pub fn schema(&mut self) -> Result<&Value> {
        let schema = if let Some(schema) = self.schema.take() {
            schema
        } else {
            let args = Map::from([("includeSchema".to_owned(), Value::Bool(true))]);
            self.read(args)?.remove("/_schema").unwrap_or_default()
        };
        Ok(self.schema.insert(schema))
    }

    /// 用 `pointer` 处的 schema 节点校验 `value`，不发送，对应 C++ 的 `Validate`；空表即通过，schema 中没有这个
    /// 位置时不约束。用的是缓存的 schema，设备状态改变之后可能过期，需要时先 [`invalidate`](Self::invalidate)。
    pub fn validate(&mut self, pointer: &str, value: &Value) -> Result<Vec<Issue>> {
        let node = schema::schema_for_path(self.schema()?, pointer);
        Ok(node
            .map(|node| schema::validate(value, node))
            .unwrap_or_default())
    }

    /// 相对路径 `path` 处的子资源，不发请求，对应 C++ 的 `Child`：去掉本路径末尾与 `path` 开头的 `/` 后以 `/`
    /// 连接。子资源继承超时与校验设置。
    #[must_use]
    pub fn child(&self, path: &str) -> Resource {
        let base = self.path.strip_suffix('/').unwrap_or(&self.path);
        let path = path.strip_prefix('/').unwrap_or(path);
        self.derive(format!("{base}/{path}"))
    }

    /// `_embedded` 中关系 `rel`（见 [`rel`]）各条目的 `_links.self.href`，对应 C++ 的 `ChildUris`；`ChildCount` 即
    /// 其长度。缺少 href 的条目跳过。
    pub fn child_paths(&mut self, rel: &str) -> Result<Vec<String>> {
        let data = self.data()?;
        let paths = entries(&data["_embedded"][rel]).iter().filter_map(href);
        Ok(paths.map(str::to_owned).collect())
    }

    /// `_embedded` 中关系 `rel` 的各个子资源，对应 C++ 的 `Children`；子资源继承超时与校验设置。
    ///
    /// 展开层级不为 0 时条目就是子资源的数据（服务端按请求的 `expandLevel` 展开），直接放进子资源的缓存，子资源的
    /// 展开层级取服务端渲染条目用的下一层（−1 不变，n > 0 为 n − 1，其他为 0，重写方案 §1.4.10），缓存与层级一致。C++ 按条目的键数猜测是否已
    /// 展开，把带 `ordIndex` 的链接也当作数据放进缓存（§5.4 第 33 条）。
    pub fn children(&mut self, rel: &str) -> Result<Vec<Resource>> {
        let expanded = self.expand_level != 0;
        let expand_level = match self.expand_level {
            -1 => -1,
            n if n > 0 => n - 1,
            _ => 0,
        };
        let found: Vec<_> = entries(&self.data()?["_embedded"][rel])
            .iter()
            .filter_map(|item| Some((href(item)?.to_owned(), expanded.then(|| item.clone()))))
            .collect();
        let children = found.into_iter().map(|(path, data)| Resource {
            expand_level,
            data,
            epoch: self.epoch,
            ..self.derive(path)
        });
        Ok(children.collect())
    }

    /// 在本集合中创建子资源（`create`），对应 C++ 的 `CreateChild`。新资源的路径取响应载荷的 `path`，没有时取
    /// `_links.self.href`，都没有时报 `Payload`。
    pub fn create_child(&self, args: impl Into<Value>) -> Result<Resource> {
        let response = self.handle.create(&self.path, args);
        let payload = response.timeout(self.timeout).response()?.payload;
        let path = ["/path", "/_links/self/href"]
            .into_iter()
            .filter_map(|pointer| payload.pointer(pointer)?.as_str())
            .find(|path| !path.is_empty())
            .ok_or_else(|| Error::Payload {
                path: self.path.clone(),
                field: "path".to_owned(),
            })?;
        Ok(self.derive(path.to_owned()))
    }

    /// 删除本资源，对应 C++ 的 `Delete`、`DeleteChild`；已经不存在（`notFound`）也算成功。消耗 `self`，删除之后
    /// 不能再用它。删除子项用 `children` 得到的资源，或对 `child_paths` 返回的绝对路径 `Resource::new`；`child` 只接受
    /// 相对路径。
    pub fn delete(self) -> Result<()> {
        match self
            .handle
            .delete(&self.path)
            .timeout(self.timeout)
            .response()
        {
            Err(Error::Request(e)) if e.response.status == Status::NOT_FOUND => Ok(()),
            result => result.map(|_| ()),
        }
    }

    /// `call` 本资源（例如 `/controls/gocator/outputs/commands/add`），对应 C++ 的 `Call`。超时是资源的超时，可以再用
    /// [`Transaction::timeout`] 修改。
    pub fn call(&self, args: impl Into<Value>) -> Transaction {
        self.handle.call(&self.path, args).timeout(self.timeout)
    }

    /// 调用命令 `<路径>/commands/<name>`，对应 C++ 的 `CallCommand`；超时同 [`call`](Self::call)。
    pub fn command(&self, name: &str, args: impl Into<Value>) -> Transaction {
        let path = format!("{}/commands/{name}", self.path);
        self.handle.call(&path, args).timeout(self.timeout)
    }

    /// 调用动作 `<路径>/actions/<name>`，对应 C++ 的 `CallAction`；超时同 [`call`](Self::call)。
    pub fn action(&self, name: &str, args: impl Into<Value>) -> Transaction {
        let path = format!("{}/actions/{name}", self.path);
        self.handle.call(&path, args).timeout(self.timeout)
    }

    /// `_links` 中 `go:command` 各链接 href 的最后一段，对应 C++ 的 `CommandNames`。
    pub fn command_names(&mut self) -> Result<Vec<String>> {
        self.link_names(&[rel::COMMAND])
    }

    /// 动作名，对应 C++ 的 `ActionNames`；同时读 `go:action` 与 `go:actions`（手册中传感器的例子用复数，C++ 只认
    /// 单数）。
    pub fn action_names(&mut self) -> Result<Vec<String>> {
        self.link_names(&[rel::ACTION, "go:actions"])
    }

    /// 订阅本资源及其后代的变更（表达式 `<路径>/*`），对应 C++ 的 `Subscribe`；已订阅时什么也不做。之后其他客户端与
    /// 设备自身的修改也会使缓存过期。
    ///
    /// C++ 订阅精确路径，收不到子项的 `created` / `deleted`（§5.4 第 40 条）。通知总是带规范路径，所以以 `extId=`
    /// 访问的资源、不是规范路径（不以 `/` 开头或有空段）的资源都收不到通知：订阅 `create_child`、`children` 得到的
    /// 资源。订阅之前的修改没有通知，所以订阅时丢弃数据缓存。
    pub fn subscribe(&mut self) -> Result<()> {
        if self.subscription.is_none() {
            let expr = format!("{}/*", self.path.trim_end_matches('/'));
            self.subscription = Some(self.handle.subscribe(&expr)?);
            self.data = None;
        }
        Ok(())
    }

    /// 取消订阅，对应 C++ 的 `Unsubscribe`：丢弃订阅，写出 `unSub`、不等应答。先取空队列，已到达的通知照样使缓存过期。
    pub fn unsubscribe(&mut self) {
        self.refresh();
        self.subscription = None;
    }

    /// 是否已订阅，对应 C++ 的 `IsSubscribed`。
    pub fn is_subscribed(&self) -> bool {
        self.subscription.is_some()
    }

    /// 同一连接上路径为 `path` 的资源，继承超时与校验设置（C++ 经管理器得到的资源都带 `GoSystem` 开启的校验）。
    fn derive(&self, path: String) -> Resource {
        Resource {
            timeout: self.timeout,
            validation: self.validation,
            ..Resource::new(self.handle.clone(), path)
        }
    }

    /// 连接的写计数变了、连接已断开（订阅队列随之丢弃未取走的通知；C++ 的 `ResetConnectionState`），或订阅队列里
    /// 有通知时，丢弃数据缓存。
    fn refresh(&mut self) {
        if self.epoch != self.handle.write_count() || !self.handle.is_connected() {
            self.data = None;
        }
        if let Some(subscription) = &self.subscription {
            // 通知与 `Lagged` 都说明资源可能变了。
            while let Ok(Some(_)) = subscription.recv_timeout(Duration::ZERO) {
                self.data = None;
            }
        }
    }

    /// 以读参数 `args` 读本资源，返回载荷。
    fn read(&self, args: Map) -> Result<Value> {
        let mut request = Request::new(Method::Read, self.path.clone());
        request.args = args.into();
        let response = self.handle.send(request).timeout(self.timeout).response()?;
        Ok(response.payload)
    }

    /// `_links` 中各关系 `rels` 的链接 href 的最后一段；没有 `/` 或最后一段为空的 href 跳过（同 C++）。
    fn link_names(&mut self, rels: &[&str]) -> Result<Vec<String>> {
        let links = &self.data()?["_links"];
        let names = rels
            .iter()
            .flat_map(|rel| entries(&links[*rel]))
            .filter_map(|link| link["href"].as_str()?.rsplit_once('/'))
            .map(|(_, name)| name)
            .filter(|name| !name.is_empty());
        Ok(names.map(str::to_owned).collect())
    }
}

/// 只输出路径与状态：缓存可能是几 MB 的文档或二进制。
impl fmt::Debug for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resource")
            .field("path", &self.path)
            .field("expand_level", &self.expand_level)
            .field("validation", &self.validation)
            .field("cached", &self.data.is_some())
            .field("subscribed", &self.subscription.is_some())
            .finish_non_exhaustive()
    }
}

/// 一个 rel 的条目：数组的元素，或单个对象本身；`[]`、nil 与缺失都没有条目。服务端只有一个引用时写出单个对象
/// （重写方案 §1.4.10），C++ 当作没有条目（§5.4 第 34 条）。
fn entries(rel: &Value) -> &[Value] {
    match rel {
        Value::Array(items) => items,
        Value::Map(_) => slice::from_ref(rel),
        _ => &[],
    }
}

/// HAL 条目的 `_links.self.href`。
fn href(item: &Value) -> Option<&str> {
    item.pointer("/_links/self/href")?.as_str()
}

/// 补丁中第一个不符合 schema 的值，返回它的 JSON Pointer 与问题；`pointer` 是 `value` 在补丁中的位置。
///
/// 对象逐键深入，其余的值（包括数组：服务端整体替换数组）按所在位置的 schema 节点校验，与 C++ 逐个 `Set*` 校验
/// 的值相同；对象本身不校验，补丁只含要修改的属性，`required` 不适用。schema 中没有的位置不约束。
fn first_invalid(schema: &Value, value: &Value, pointer: &str) -> Option<(String, Vec<Issue>)> {
    let Value::Map(map) = value else {
        let issues = schema::validate(value, schema::schema_for_path(schema, pointer)?);
        return (!issues.is_empty()).then(|| (pointer.to_owned(), issues));
    };
    map.iter().find_map(|(key, value)| {
        let key = key.replace('~', "~0").replace('/', "~1");
        first_invalid(schema, value, &format!("{pointer}/{key}"))
    })
}

// 保留原因：补丁逐值校验是自有算法（C++ 只校验单个 Set 的值、不校验 SetJson），无法与官方实现对照。
#[cfg(test)]
mod tests {
    use super::first_invalid;
    use crate::schema::Issue;
    use crate::value::Value;

    // 约定：对象逐键深入，键按 RFC 6901 转义；对象以外的值（数组整体）按所在位置校验，报告第一个不合格的值及其
    // JSON Pointer；对象本身不校验（补丁不受 required 约束）；schema 中没有的位置不约束。
    #[test]
    fn patch_values_are_checked_where_they_land() {
        let schema: Value = r#"{"type":"object","properties":{
            "id":{"type":"string","readOnly":true},
            "a/b~":{"type":"integer"},
            "obj":{"type":"object","required":["q"],"properties":{"r":{"type":"string"}}},
            "list":{"type":"array","maxItems":2},
            "n":{"type":"integer","maximum":10}}}"#
            .parse()
            .unwrap();
        let check = |patch: &str| first_invalid(&schema, &patch.parse().unwrap(), "");
        assert_eq!(check(r#"{"obj":{"r":"s"},"n":10}"#), None);
        assert_eq!(check(r#"{"unknown":{"deep":1}}"#), None);
        assert_eq!(
            check(r#"{"a/b~":"x"}"#),
            Some(("/a~1b~0".to_owned(), vec![Issue::Type("integer".into())]))
        );
        assert_eq!(
            check(r#"{"id":"x"}"#),
            Some(("/id".to_owned(), vec![Issue::ReadOnly]))
        );
        assert_eq!(
            check(r#"{"list":[1,2,3],"n":11}"#),
            Some(("/list".to_owned(), vec![Issue::MaxItems(2)]))
        );
        assert_eq!(
            check(r#"{"obj":5}"#),
            Some(("/obj".to_owned(), vec![Issue::Type("object".into())]))
        );
    }
}
