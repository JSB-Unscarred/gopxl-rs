//! 动态值：控制通道消息体的内存表示。
//!
//! - 值语义：`Clone` 是深拷贝，副本之间互不影响；读取（[`Value::pointer`]、`Index`、`as_*`）不修改文档；
//! - [`Value::set`] / [`Value::remove`] 直接操作树：空容器仍是空容器，数组仍是数组，删除数组元素时后面的元素前移。
//!
//! 对象用 `BTreeMap`：键按字节序排列，编码时不用另外排序，同一个值总是编码成同样的字节。
//!
//! serde：`Value` 实现 `Serialize` 与 `Deserialize`，`&Value` 实现 `Deserializer`，载荷按需反序列化成调用方自己的
//! 类型（`T::deserialize(&value)`），`Value` 也能作为别的类型的字段。线上的 MessagePack 由 [`msgpack`] 手写，
//! 编码规则见该模块。

pub mod msgpack;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Index;
use std::str::FromStr;

use bytes::Bytes;
use serde::de::value::{
    BorrowedStrDeserializer, MapAccessDeserializer, MapDeserializer, SeqDeserializer,
};
use serde::de::{self, Deserializer, IntoDeserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer, forward_to_deserialize_any};

/// 一个 MessagePack / JSON 值。
///
/// 同一个数只有一种表示：能放进 `i64` 的整数一律是 [`Value::Int`]，[`Value::UInt`] 只用于大于 `i64::MAX` 的值
/// （metrics 的计数器），所以 `PartialEq` 可以直接 derive。解码、JSON 解析与 `From<u64>` 都遵守这条规则。
///
/// 载荷反序列化成自己的类型（`&Value` 实现 [`serde::Deserializer`]，字符串与字节可以借用）：
///
/// ```
/// use gopxl::Value;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// #[serde(rename_all = "camelCase")]
/// struct Sensor<'a> {
///     display_name: &'a str,
///     serial_number: Option<u64>,
/// }
///
/// let value: Value = r#"{"displayName":"Main","serialNumber":null,"extra":1}"#.parse()?;
/// let sensor = Sensor::deserialize(&value)?;
/// assert_eq!(sensor.display_name, "Main");
/// assert_eq!(sensor.serial_number, None);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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
    /// 浮点数；服务端写出的 float32 在解码时拓宽为 `f64`。
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

/// 对象，键按字节序排列。
pub type Map = BTreeMap<String, Value>;

/// [`Value::set`] 的错误：指针穿过标量，或数组下标不存在。携带出错的指针。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot set JSON pointer {0:?}: it crosses a scalar or a missing array element")]
pub struct PointerError(pub String);

impl Value {
    /// 空对象 `{}`，请求参数与补丁的起点。
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

    /// 能放进 `i64` 的整数；浮点数返回 `None`，不截断。
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Int(v) => Some(v),
            Self::UInt(v) => i64::try_from(v).ok(),
            _ => None,
        }
    }

    /// 非负整数；负数与浮点数返回 `None`，不回绕。
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::Int(v) => u64::try_from(v).ok(),
            Self::UInt(v) => Some(v),
            _ => None,
        }
    }

    /// 任意数值转为 `f64`；绝对值超过 2⁵³ 的整数会舍入。
    #[allow(clippy::cast_precision_loss, reason = "整数转为浮点数时舍入是预期行为")]
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

    /// 二进制；克隆返回的 `Bytes` 不复制数据。
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

    /// 按 JSON Pointer（RFC 6901）读取，不存在时返回 `None`，不修改文档。
    ///
    /// 缺少前导 `/` 时自动补上：`"a/b"` 等同于 `"/a/b"`；`""` 指向根。
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        tokens(pointer).try_fold(self, |node, token| match node {
            Self::Map(map) => map.get(&*token),
            Self::Array(items) => items.get(array_index(&token)?),
            _ => None,
        })
    }

    /// 把 `pointer` 处设为 `value`。
    ///
    /// 缺失的对象键与 `Nil` 节点自动创建为对象，所以可以从 `Nil` 或 `{}`
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

    /// 删除 `pointer` 处的值并返回它；删除数组元素时后面的元素前移。根不能删除。
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

    /// 应用 RFC 7396 merge patch：补丁中的 `Nil` 删除对应的键，对象递归合并，其他值整体替换。
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

/// 把 JSON Pointer 拆成解除转义的 token；缺少前导 `/` 时视为有，`""` 产生零个 token（指向根）。
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

/// 数组下标：十进制数字，除 `0` 外不带前导零（RFC 6901）。`str::parse` 只比它多接受前导 `+`。
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
    // `Vec<u8>` 是二进制；不提供 `Vec<T>` 到数组的泛型转换，免得字节缓冲被悄悄编码成整数数组。
    Bytes => Bin, Vec<u8> => Bin,
}

/// 能放进 `i64` 时为 `Int`，否则为 `UInt`，确保每个数只有一种表示。
impl From<u64> for Value {
    fn from(v: u64) -> Self {
        i64::try_from(v).map_or(Self::UInt(v), Self::Int)
    }
}

/// JSON 文本；`{:#}` 输出缩进格式。经 `serde_json` 输出，只用于日志与调试。
///
/// JSON 无法表示的值：`Bin` 输出为字符串 `"<bin N bytes>"`（[`Serialize`] 则输出字节），免得打印归档时输出几百 MB；
/// NaN 与无穷输出为 `null`。
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = if f.alternate() {
            serde_json::to_string_pretty(&Text(self))
        } else {
            serde_json::to_string(&Text(self))
        };
        // 键都是字符串，serde_json 不会拒绝。
        f.write_str(&text.map_err(|_| fmt::Error)?)
    }
}

/// `Display` 用的视图：`Bin` 换成占位字符串，容器逐层套上视图，其余与 `Value` 的 `Serialize` 相同。
struct Text<'a>(&'a Value);

impl Serialize for Text<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Bin(v) => serializer.collect_str(&format_args!("<bin {} bytes>", v.len())),
            Value::Array(items) => serializer.collect_seq(items.iter().map(Text)),
            Value::Map(map) => serializer.collect_map(map.iter().map(|(k, v)| (k, Text(v)))),
            scalar => scalar.serialize(serializer),
        }
    }
}

/// 解析 JSON 文本，例如 `r#"{"enabled":true}"#.parse::<Value>()`。嵌套深度受 `serde_json` 的 128 层上限约束。
/// 浮点数正确舍入（`serde_json` 的 `float_roundtrip`）；整数文本 `-0` 是 `Float(-0.0)`，负零的符号得以保留。
impl FromStr for Value {
    type Err = serde_json::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(text)
    }
}

/// `Nil` 为 unit（JSON 的 `null`），`Bin` 为字节，其余按 serde 数据模型的同名类型。
impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Nil => serializer.serialize_unit(),
            Value::Bool(v) => serializer.serialize_bool(*v),
            Value::Int(v) => serializer.serialize_i64(*v),
            Value::UInt(v) => serializer.serialize_u64(*v),
            Value::Float(v) => serializer.serialize_f64(*v),
            Value::Str(v) => serializer.serialize_str(v),
            Value::Bin(v) => serializer.serialize_bytes(v),
            Value::Array(items) => items.serialize(serializer),
            Value::Map(map) => map.serialize(serializer),
        }
    }
}

/// 从任意 serde 数据模型构造，包括 JSON 文本（[`FromStr`]）与 `serde_json::Value`（`serde_json::json!` 的结果）：
/// 整数按 [`Value`] 的规则归一化，字节为 `Bin`，`None` 与 unit 为 `Nil`。
impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON / MessagePack value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Int(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Float(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::Str(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::Str(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Value, E> {
        Ok(Value::Bin(Bytes::copy_from_slice(v)))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Value, E> {
        Ok(Value::Bin(v.into()))
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Nil)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Nil)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut entries = Map::new();
        while let Some((key, value)) = map.next_entry()? {
            entries.insert(key, value);
        }
        Ok(Value::Map(entries))
    }
}

impl<'de> IntoDeserializer<'de, de::value::Error> for &'de Value {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// 把 `Value` 反序列化成任意类型：`Nil` 是 unit 与 `None`；整数经 serde 的范围检查，
/// 越界报错而不截断；`Str`、`Bin` 与对象的键按借用交给访问者，`&str`、`&[u8]` 零拷贝；枚举接受字符串（单元变体）
/// 与单键对象；元组与结构体要求数组的元素数正好。错误是 [`serde::de::value::Error`]，只带一句说明。
impl<'de> Deserializer<'de> for &'de Value {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Nil => visitor.visit_unit(),
            Value::Bool(v) => visitor.visit_bool(*v),
            Value::Int(v) => visitor.visit_i64(*v),
            Value::UInt(v) => visitor.visit_u64(*v),
            Value::Float(v) => visitor.visit_f64(*v),
            Value::Str(v) => visitor.visit_borrowed_str(v),
            Value::Bin(v) => visitor.visit_borrowed_bytes(v),
            Value::Array(items) => {
                let mut seq = SeqDeserializer::new(items.iter());
                let value = visitor.visit_seq(&mut seq)?;
                seq.end()?;
                Ok(value)
            }
            Value::Map(map) => {
                let mut entries = MapDeserializer::new(borrowed_entries(map));
                let value = visitor.visit_map(&mut entries)?;
                entries.end()?;
                Ok(value)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if self.is_nil() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Value::Str(v) => visitor.visit_enum(BorrowedStrDeserializer::new(v)),
            Value::Map(map) => visitor.visit_enum(MapAccessDeserializer::new(
                MapDeserializer::new(borrowed_entries(map)),
            )),
            // 访问者自己报“类型不对、期望枚举”。
            _ => self.deserialize_any(visitor),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf unit
        unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

/// 对象的条目，键按借用交给 serde。
fn borrowed_entries(
    map: &Map,
) -> impl Iterator<Item = (BorrowedStrDeserializer<'_, de::value::Error>, &Value)> {
    map.iter()
        .map(|(key, value)| (BorrowedStrDeserializer::new(key), value))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::Value;

    // 约定：JSON 文本与 serde 数据模型按 Value 的规则归一化（能放进 i64 的为 Int，更大的为 UInt，-0 为 Float）；
    // 反序列化时 Nil 是 None，整数越界、类型不符、元素数不对都报错而不截断，字符串与字节按借用读出；Display 把 Bin
    // 换成占位、其余与 Serialize 经 serde_json 的输出相同。
    #[test]
    fn serde_follows_value_conventions() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Fields<'a> {
            a: u8,
            b: Option<u64>,
            d: Option<i32>,
            l: (i64, i64),
            s: &'a str,
        }

        let text = r#"{"a":1,"b":18446744073709551615,"c":-0,"d":null,"l":[1,2],"s":"x"}"#;
        let value: Value = text.parse().unwrap();
        assert_eq!(value["a"], Value::Int(1));
        assert_eq!(value["b"], Value::UInt(u64::MAX));
        assert!(matches!(value["c"], Value::Float(f) if f.is_sign_negative()));

        let fields = Fields::deserialize(&value).unwrap();
        assert_eq!(
            fields,
            Fields {
                a: 1,
                b: Some(u64::MAX),
                d: None,
                l: (1, 2),
                s: "x",
            }
        );
        assert!(i8::deserialize(&value["b"]).is_err());
        assert!(bool::deserialize(&value["a"]).is_err());
        assert!(<(i64,)>::deserialize(&value["l"]).is_err());
        assert_eq!(Value::deserialize(&value).unwrap(), value);

        let bin = Value::from(vec![1_u8, 2, 3]);
        assert_eq!(<&[u8]>::deserialize(&bin).unwrap(), [1, 2, 3]);
        assert_eq!(bin.to_string(), r#""<bin 3 bytes>""#);
        assert_eq!(serde_json::to_string(&value).unwrap(), value.to_string());
    }
}
