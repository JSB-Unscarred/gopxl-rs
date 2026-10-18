//! 动态值：控制通道消息体的内存表示，取代 C++ SDK 的 `GoJson`（`nlohmann::json` 3.8.0 的包装）。
//!
//! 与 `GoJson` 的差异都用来修掉它的缺陷：
//! - 值语义：`Clone` 是深拷贝，不存在赋值后共享存储；读取（[`Value::pointer`]、`Index`、`as_*`）不修改文档；
//! - [`Value::set`] / [`Value::remove`] 直接操作树，不经过 flatten，不会把空容器变成 null、把数组变成对象，
//!   删除数组元素时后面的元素前移而不是留下 null。
//!
//! 对象用 `BTreeMap`：键按字节序排列，与 nlohmann 的 `std::map` 相同，编码时不用排序就得到 C++ SDK 的字节。

pub mod msgpack;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Index;
use std::str::FromStr;

use bytes::Bytes;

/// 一个 MessagePack / JSON 值。
///
/// 同一个数只有一种表示：能放进 `i64` 的整数一律是 [`Value::Int`]，[`Value::UInt`] 只用于大于 `i64::MAX` 的值
/// （metrics 的计数器），所以 `PartialEq` 可以直接 derive。解码、JSON 解析与 `From<u64>` 都遵守这条规则。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    /// `null`（MessagePack 的 nil）。
    #[default]
    Nil,
    /// 布尔值。
    Bool(bool),
    /// 能放进 `i64` 的整数。
    Int(i64),
    /// 大于 `i64::MAX` 的整数；用 `Value::from(u64)` 构造即自动归一化。
    UInt(u64),
    /// 浮点数；服务端写出的 float32 在解码时拓宽为 `f64`（与 nlohmann 相同）。
    Float(f64),
    /// 字符串。
    Str(String),
    /// 二进制（文件、归档、GDP 流载荷）。解码得到的是所在帧的零拷贝切片，克隆只增加引用计数。
    Bin(Bytes),
    /// 数组。
    Array(Vec<Value>),
    /// 对象。
    Map(Map),
}

/// 对象，键按字节序排列（与 nlohmann 的 `std::map` 相同）。
pub type Map = BTreeMap<String, Value>;

/// [`Value::set`] 的错误：指针穿过标量，或数组下标不存在。携带出错的指针。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot set JSON pointer {0:?}: it crosses a scalar or a missing array element")]
pub struct PointerError(pub String);

impl Value {
    /// 空对象 `{}`，即 `GoJson` 的默认值。
    pub fn object() -> Self {
        Self::Map(Map::new())
    }

    /// 是否为 `null`。
    pub fn is_nil(&self) -> bool {
        matches!(self, Self::Nil)
    }

    /// 布尔值。
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(v) => Some(v),
            _ => None,
        }
    }

    /// 能放进 `i64` 的整数；浮点数返回 `None`，不按 nlohmann 的规则截断。
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Int(v) => Some(v),
            Self::UInt(v) => i64::try_from(v).ok(),
            _ => None,
        }
    }

    /// 非负整数；负数与浮点数返回 `None`（nlohmann 会把负数回绕成大正数）。
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::Int(v) => u64::try_from(v).ok(),
            Self::UInt(v) => Some(v),
            _ => None,
        }
    }

    /// 任意数值转为 `f64`；绝对值超过 2⁵³ 的整数会舍入。
    #[allow(
        clippy::cast_precision_loss,
        reason = "与 nlohmann 的 get<double>() 相同，舍入是预期行为"
    )]
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Self::Int(v) => Some(v as f64),
            Self::UInt(v) => Some(v as f64),
            Self::Float(v) => Some(v),
            _ => None,
        }
    }

    /// 字符串。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(v) => Some(v),
            _ => None,
        }
    }

    /// 二进制，对应 `GoJson::GetBinary`；克隆返回的 `Bytes` 不复制数据。
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Self::Bin(v) => Some(v),
            _ => None,
        }
    }

    /// 数组。
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(v) => Some(v),
            _ => None,
        }
    }

    /// 对象。
    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Self::Map(v) => Some(v),
            _ => None,
        }
    }

    /// 按 JSON Pointer（RFC 6901）读取，对应 `GoJson::At` / `Get`，不存在时返回 `None`，不修改文档。
    ///
    /// 与 `GoJsonPointer` 相同，缺少前导 `/` 时自动补上：`"a/b"` 等同于 `"/a/b"`；`""` 指向根。
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        tokens(pointer).try_fold(self, |node, token| match node {
            Self::Map(map) => map.get(&*token),
            Self::Array(items) => items.get(array_index(&token)?),
            _ => None,
        })
    }

    /// 把 `pointer` 处设为 `value`，对应 `GoJson::Set` / `Replace`。
    ///
    /// 缺失的对象键与 `Nil` 节点自动创建为对象（与 nlohmann 的 `operator[]` 相同），所以可以从 `Nil` 或 `{}`
    /// 开始构建嵌套的请求或补丁；`value` 为 `Nil` 时保存显式的 null。数组只能写已有下标或用 `-` 追加，
    /// 不会被自动创建。只有穿过已有节点时才会出错，所以出错时文档不变。
    pub fn set(&mut self, pointer: &str, value: impl Into<Value>) -> Result<(), PointerError> {
        let error = || PointerError(pointer.to_owned());
        let mut node = self;
        for token in tokens(pointer) {
            if node.is_nil() {
                *node = Self::object();
            }
            node = match node {
                Self::Map(map) => map.entry(token.into_owned()).or_default(),
                Self::Array(items) => {
                    let slot = if token == "-" {
                        items.push(Self::Nil);
                        items.last_mut()
                    } else {
                        array_index(&token).and_then(|index| items.get_mut(index))
                    };
                    slot.ok_or_else(error)?
                }
                _ => return Err(error()),
            };
        }
        *node = value.into();
        Ok(())
    }

    /// 删除 `pointer` 处的值并返回它，对应 `GoJson::Remove`；删除数组元素时后面的元素前移。根不能删除。
    pub fn remove(&mut self, pointer: &str) -> Option<Value> {
        let mut tokens: Vec<_> = tokens(pointer).collect();
        let last = tokens.pop()?;
        let parent = tokens
            .into_iter()
            .try_fold(self, |node, token| match node {
                Self::Map(map) => map.get_mut(&*token),
                Self::Array(items) => items.get_mut(array_index(&token)?),
                _ => None,
            })?;
        match parent {
            Self::Map(map) => map.remove(&*last),
            Self::Array(items) => array_index(&last)
                .filter(|&index| index < items.len())
                .map(|index| items.remove(index)),
            _ => None,
        }
    }

    /// 应用 RFC 7396 merge patch，对应 `GoJson::Merge`：补丁中的 `Nil` 删除对应的键，对象递归合并，
    /// 其他值整体替换。
    pub fn merge_patch(&mut self, patch: &Value) {
        let Self::Map(entries) = patch else {
            *self = patch.clone();
            return;
        };
        let Self::Map(target) = self else {
            *self = Self::object();
            return self.merge_patch(patch);
        };
        for (key, value) in entries {
            if value.is_nil() {
                target.remove(key);
            } else {
                target.entry(key.clone()).or_default().merge_patch(value);
            }
        }
    }
}

/// 把 JSON Pointer 拆成解除转义的 token；缺少前导 `/` 时视为有，`""` 没有 token（指向根）。
/// 不含 `~` 的 token 直接借用，读取响应时不分配。
pub(crate) fn tokens(pointer: &str) -> impl Iterator<Item = Cow<'_, str>> {
    let rest = pointer.strip_prefix('/').unwrap_or(pointer);
    (!pointer.is_empty())
        .then(|| rest.split('/'))
        .into_iter()
        .flatten()
        .map(|token| {
            if token.contains('~') {
                // RFC 6901 §4：先把 `~1` 换成 `/`，再把 `~0` 换成 `~`，`~01` 才会得到 `~1`。
                Cow::Owned(token.replace("~1", "/").replace("~0", "~"))
            } else {
                Cow::Borrowed(token)
            }
        })
}

/// 数组下标：十进制数字，除 `0` 外没有前导零（RFC 6901）。`str::parse` 只比它多接受前导 `+`。
pub(crate) fn array_index(token: &str) -> Option<usize> {
    if token.starts_with('+') || (token.len() > 1 && token.starts_with('0')) {
        None
    } else {
        token.parse().ok()
    }
}

/// 找不到时返回 `&Value::Nil`，便于连续下标：`response["payload"]["state"]`。
impl Index<&str> for Value {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        self.as_map()
            .and_then(|map| map.get(key))
            .unwrap_or(&Value::Nil)
    }
}

/// 找不到时返回 `&Value::Nil`。
impl Index<usize> for Value {
    type Output = Value;

    fn index(&self, index: usize) -> &Value {
        self.as_array()
            .and_then(|items| items.get(index))
            .unwrap_or(&Value::Nil)
    }
}

macro_rules! from_lossless {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl From<$ty> for Value {
            fn from(v: $ty) -> Self {
                Self::$variant(v.into())
            }
        }
    )*};
}

from_lossless! {
    bool => Bool,
    i8 => Int, i16 => Int, i32 => Int, i64 => Int,
    u8 => Int, u16 => Int, u32 => Int,
    f32 => Float, f64 => Float,
    &str => Str, String => Str,
    Map => Map, Vec<Value> => Array,
    // `Vec<u8>` 是二进制，与 `GoJson::Set(path, std::vector<k8u>)` 相同；不提供 `Vec<T>` 到数组的泛型转换，
    // 免得字节缓冲被悄悄编码成整数数组。
    Bytes => Bin, Vec<u8> => Bin,
}

/// 能放进 `i64` 时为 `Int`，否则为 `UInt`，保持每个数只有一种表示。
impl From<u64> for Value {
    fn from(v: u64) -> Self {
        i64::try_from(v).map_or(Self::UInt(v), Self::Int)
    }
}

/// JSON 文本，对应 `GoJson::ToString`；`{:#}` 输出缩进格式。经 `serde_json` 输出，每次复制一遍树（`Bin` 除外），
/// 只用于日志与调试。
///
/// JSON 无法表示的值：`Bin` 输出为字符串 `"<bin N bytes>"`，免得打印归档时输出几百 MB；NaN 与无穷输出为 `null`
/// （与 nlohmann 相同）。
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        to_json(self).fmt(f)
    }
}

/// 转换为 `serde_json` 的值，规则见 `Display`。
fn to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Nil => serde_json::Value::Null,
        Value::Bool(v) => (*v).into(),
        Value::Int(v) => (*v).into(),
        Value::UInt(v) => (*v).into(),
        Value::Float(v) => (*v).into(),
        Value::Str(v) => v.as_str().into(),
        Value::Bin(v) => format!("<bin {} bytes>", v.len()).into(),
        Value::Array(items) => items.iter().map(to_json).collect(),
        Value::Map(map) => map.iter().map(|(k, v)| (k.clone(), to_json(v))).collect(),
    }
}

/// 解析 JSON 文本，对应 `GoJson(const std::string&)` 与 `GoJson::ParseString`，例如
/// `r#"{"enabled":true}"#.parse::<Value>()`。嵌套深度受 `serde_json` 的 128 层上限约束。浮点数正确舍入，与 `GoJson`
/// 逐位相同；整数文本 `-0` 是 `Float(-0.0)`（`GoJson` 为整数 0）。
impl FromStr for Value {
    type Err = serde_json::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(text).map(from_json)
    }
}

/// 把 `serde_json` 的值转换过来，整数按 [`Value`] 的规则归一化。
fn from_json(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(v) => Value::Bool(v),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(v), _) => Value::Int(v),
            (None, Some(v)) => Value::UInt(v),
            (None, None) => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(v) => Value::Str(v),
        serde_json::Value::Array(items) => Value::Array(items.into_iter().map(from_json).collect()),
        serde_json::Value::Object(map) => {
            Value::Map(map.into_iter().map(|(k, v)| (k, from_json(v))).collect())
        }
    }
}
