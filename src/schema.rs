//! 资源 schema（读取时 `includeSchema` 得到的 `_schema`）的导航与校验，对应 C++ 的 `GoSchemaValidator`。
//!
//! GoPxL 的 schema 是基于 JSON Schema Draft 2019-09 的自定义方言：不支持 `$ref` 与组合关键字，另有 `binary`
//! 类型和 `unit`、`visibility`、`extendedType` 等注解。这里只实现 `GoSchemaValidator` 的导航与单节点校验，
//! 注解经 [`Value`] 直接读取。校验只是建议性的，服务端才是裁判。

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use crate::value::{Value, array_index, tokens};

/// 浮点比较的容差，与 C++ 相同。
const EPSILON: f64 = 1e-9;

/// 校验发现的一个问题，对应 `GoResourceValidationError::Errors()` 中的一条。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Issue {
    /// `readOnly` 为真。
    #[error("property is read-only")]
    ReadOnly,
    /// 类型不符，携带 schema 的 `type`。
    #[error("type mismatch: expected {0}")]
    Type(Value),
    /// 小于 `minimum`。
    #[error("value is less than minimum {0}")]
    Minimum(f64),
    /// 大于 `maximum`。
    #[error("value exceeds maximum {0}")]
    Maximum(f64),
    /// 不大于 `exclusiveMinimum`。
    #[error("value is not greater than exclusiveMinimum {0}")]
    ExclusiveMinimum(f64),
    /// 不小于 `exclusiveMaximum`。
    #[error("value is not less than exclusiveMaximum {0}")]
    ExclusiveMaximum(f64),
    /// 不是 `multipleOf` 的整数倍。
    #[error("value is not a multiple of {0}")]
    MultipleOf(f64),
    /// 字符数小于 `minLength`。
    #[error("string is shorter than minLength {0}")]
    MinLength(u64),
    /// 字符数大于 `maxLength`。
    #[error("string is longer than maxLength {0}")]
    MaxLength(u64),
    /// 元素数小于 `minItems`。
    #[error("array has fewer items than minItems {0}")]
    MinItems(u64),
    /// 元素数大于 `maxItems`。
    #[error("array has more items than maxItems {0}")]
    MaxItems(u64),
    /// 缺少 `required` 中的属性。
    #[error("missing required property {0:?}")]
    Required(String),
    /// 不在 `enum` 中。
    #[error("value is not in the allowed enum set")]
    Enum,
}

/// 找出 JSON Pointer `path` 处数据对应的子 schema，对应 `GoSchemaValidator::SchemaForPath`。
///
/// 路径与 [`Value::pointer`] 同一语法，校验与写入因此指向同一节点：`""` 为根，`"/"` 是键 `""`，末尾的 `/` 是
/// 一个空段，数组下标不带前导零（C++ 的 `SplitPath` 把 `"/"` 当作根、忽略末尾的 `/`，与写入补丁的 nlohmann
/// pointer 不一致）。对象节点（或不带 `type` 的节点）进入 `properties/<段>`；数组节点在 `items` 是数组（元组
/// 形式）时取对应元素，否则所有元素共用 `items`。可空类型写作 `["array", "null"]` 时同样按数组导航（C++ 只认
/// 字符串形式的 `type`）。路径不在 schema 中时为 `None`（C++ 抛 `GoResourceError`），调用方按“不约束”处理。
pub fn schema_for_path<'a>(schema: &'a Value, path: &str) -> Option<&'a Value> {
    tokens(path).try_fold(schema, |node, token| {
        if has_type(node, "array") {
            let index = array_index(&token)?;
            match &node["items"] {
                Value::Array(items) => items.get(index),
                Value::Nil => None,
                items => Some(items),
            }
        } else if node["type"].is_nil() || has_type(node, "object") {
            node["properties"].as_map()?.get(&*token)
        } else {
            None
        }
    })
}

/// 用一个 schema 节点校验 `value`，对应 `GoSchemaValidator::Validate`；返回空表即通过。
///
/// 与 C++ 相同，只校验这一层（不递归进 `properties` / `items`），`readOnly` 与类型不符时立即返回，其余问题
/// 全部收集。修正 C++ 的缺陷（重写方案 §5.4 第 37 条）：`binary` 要求 `Bin`（C++ 要求字符串）；`type` 可以是
/// 数组（可空类型，C++ 跳过类型检查）；`multipleOf` 对负数不误报；`enum` 中的整数精确比较（C++ 截断为 `int`）；
/// 空 `enum` 不约束（C++ 拒绝一切值）；字符串长度按字符计（C++ 按字节）。
pub fn validate(value: &Value, node: &Value) -> Vec<Issue> {
    if node["readOnly"].as_bool() == Some(true) {
        return vec![Issue::ReadOnly];
    }
    let mut types = type_names(node).peekable();
    if types.peek().is_some() && !types.any(|ty| type_matches(value, ty)) {
        return vec![Issue::Type(node["type"].clone())];
    }

    let mut issues = Vec::new();
    let number = |key: &str| node[key].as_f64();
    let count = |key: &str| node[key].as_u64();

    if let Some(v) = value.as_f64() {
        issues.extend(number("minimum").filter(|&l| v < l).map(Issue::Minimum));
        issues.extend(number("maximum").filter(|&l| v > l).map(Issue::Maximum));
        issues.extend(
            number("exclusiveMinimum")
                .filter(|&l| v <= l)
                .map(Issue::ExclusiveMinimum),
        );
        issues.extend(
            number("exclusiveMaximum")
                .filter(|&l| v >= l)
                .map(Issue::ExclusiveMaximum),
        );
        // `%` 的余数与被除数同号；比较绝对值，负数在 −m 附近的余数不再误报。
        let not_multiple = |d: f64| {
            let rem = (v % d).abs();
            d.abs() > 0.0 && rem > EPSILON && d.abs() - rem > EPSILON
        };
        issues.extend(
            number("multipleOf")
                .filter(|&d| not_multiple(d))
                .map(Issue::MultipleOf),
        );
    }

    match value {
        Value::Str(text) => {
            let n = text.chars().count() as u64;
            issues.extend(count("minLength").filter(|&l| n < l).map(Issue::MinLength));
            issues.extend(count("maxLength").filter(|&l| n > l).map(Issue::MaxLength));
        }
        Value::Array(items) => {
            let n = items.len() as u64;
            issues.extend(count("minItems").filter(|&l| n < l).map(Issue::MinItems));
            issues.extend(count("maxItems").filter(|&l| n > l).map(Issue::MaxItems));
        }
        Value::Map(map) => {
            let required = node["required"].as_array().unwrap_or_default();
            for name in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    issues.push(Issue::Required(name.to_owned()));
                }
            }
        }
        _ => {}
    }

    let options = node["enum"].as_array().unwrap_or_default();
    let scalar = !matches!(value, Value::Nil | Value::Array(_) | Value::Map(_));
    if scalar && !options.is_empty() && !options.iter().any(|option| enum_eq(value, option)) {
        issues.push(Issue::Enum);
    }
    issues
}

/// `type` 中的类型名：字符串，或字符串数组（可空类型）。
fn type_names(node: &Value) -> impl Iterator<Item = &str> {
    let names = match &node["type"] {
        Value::Array(names) => names.as_slice(),
        name @ Value::Str(_) => std::slice::from_ref(name),
        _ => &[],
    };
    names.iter().filter_map(Value::as_str)
}

/// `type` 是否含类型名 `name`。
fn has_type(node: &Value, name: &str) -> bool {
    type_names(node).any(|ty| ty == name)
}

/// 值是否属于类型名 `ty`；`integer` 也是 `number`，`null` 只接受 `Nil`（C++ 不认识 `null`，一律通过）。不认识的
/// 类型名不约束（与 C++ 相同）。
fn type_matches(value: &Value, ty: &str) -> bool {
    match ty {
        "null" => value.is_nil(),
        "boolean" => matches!(value, Value::Bool(_)),
        "integer" => matches!(value, Value::Int(_) | Value::UInt(_)),
        "number" => matches!(value, Value::Int(_) | Value::UInt(_) | Value::Float(_)),
        "string" => matches!(value, Value::Str(_)),
        "binary" => matches!(value, Value::Bin(_)),
        "object" => matches!(value, Value::Map(_)),
        "array" => matches!(value, Value::Array(_)),
        _ => true,
    }
}

/// `enum` 的相等：有浮点数时按容差比较，其余按值相等（整数归一化后精确比较）。
fn enum_eq(value: &Value, option: &Value) -> bool {
    match (value, option) {
        (Value::Float(_), _) | (_, Value::Float(_)) => value
            .as_f64()
            .zip(option.as_f64())
            .is_some_and(|(a, b)| (a - b).abs() < EPSILON),
        _ => value == option,
    }
}
