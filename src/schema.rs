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

use std::fmt;

use crate::value::{Value, array_index, tokens};

/// 浮点比较的容差，与 C++ 相同。
const EPSILON: f64 = 1e-9;

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

/// 用一个 schema 节点校验 `value`，对应 `GoSchemaValidator::Validate`。每条问题是被违反的关键字与界限
/// （`"maximum 10"`、`"type \"integer\""`、`"readOnly"`），空表即通过；C++ 的 `Errors()` 同样是字符串列表。
///
/// 与 C++ 相同，只校验这一层（不递归进 `properties` / `items`），`readOnly` 与类型不符时立即返回，其余问题
/// 全部收集。修正 C++ 的缺陷：`binary` 要求 `Bin`（C++ 要求字符串）；`type` 可以是
/// 数组（可空类型，C++ 跳过类型检查）；`multipleOf` 对负数不误报；`enum` 中的整数精确比较（C++ 截断为 `int`）；
/// 空 `enum` 不约束（C++ 拒绝一切值）；字符串长度按字符计（C++ 按字节）。
pub fn validate(value: &Value, node: &Value) -> Vec<String> {
    if node["readOnly"].as_bool() == Some(true) {
        return vec!["readOnly".to_owned()];
    }
    let mut types = type_names(node).peekable();
    if types.peek().is_some() && !types.any(|ty| type_matches(value, ty)) {
        return vec![format!("type {}", node["type"])];
    }

    let mut issues = Vec::new();
    if let Some(v) = value.as_f64() {
        issues.extend(violated(node, "minimum", Value::as_f64, |l| v < l));
        issues.extend(violated(node, "maximum", Value::as_f64, |l| v > l));
        issues.extend(violated(node, "exclusiveMinimum", Value::as_f64, |l| {
            v <= l
        }));
        issues.extend(violated(node, "exclusiveMaximum", Value::as_f64, |l| {
            v >= l
        }));
        // `%` 的余数与被除数同号；比较绝对值，负数在 −m 附近的余数不再误报。
        issues.extend(violated(node, "multipleOf", Value::as_f64, |d| {
            let rem = (v % d).abs();
            d.abs() > 0.0 && rem > EPSILON && d.abs() - rem > EPSILON
        }));
    }
    match value {
        Value::Str(text) => {
            let n = text.chars().count() as u64;
            issues.extend(violated(node, "minLength", Value::as_u64, |l| n < l));
            issues.extend(violated(node, "maxLength", Value::as_u64, |l| n > l));
        }
        Value::Array(items) => {
            let n = items.len() as u64;
            issues.extend(violated(node, "minItems", Value::as_u64, |l| n < l));
            issues.extend(violated(node, "maxItems", Value::as_u64, |l| n > l));
        }
        Value::Map(map) => {
            let required = node["required"].as_array().unwrap_or_default();
            for name in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    issues.push(format!("required {name:?}"));
                }
            }
        }
        _ => {}
    }

    let options = node["enum"].as_array().unwrap_or_default();
    let scalar = !matches!(value, Value::Nil | Value::Array(_) | Value::Map(_));
    if scalar && !options.is_empty() && !options.iter().any(|option| enum_eq(value, option)) {
        issues.push("enum".to_owned());
    }
    issues
}

/// schema 节点中 `keyword` 的界限（经 `limit` 读出）被 `broken` 判为违反时，返回“关键字 界限”。
fn violated<T: Copy + fmt::Display>(
    node: &Value,
    keyword: &str,
    limit: fn(&Value) -> Option<T>,
    broken: impl Fn(T) -> bool,
) -> Option<String> {
    limit(&node[keyword])
        .filter(|&l| broken(l))
        .map(|l| format!("{keyword} {l}"))
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

// 校验与导航是刻意不同于 C++ 的自有算法，无法与官方实现对照。
#[cfg(test)]
mod tests {
    use super::{schema_for_path, validate};
    use crate::value::Value;

    // 约定：binary 要求 Bin；type 数组表示可空；multipleOf 对负数不误报；enum 中的整数精确比较；空 enum 不约束；
    // 长度按字符计。schema_for_path：路径与 Value::pointer 同一语法（"/" 是键 ""），可空数组同样按 items 导航。
    #[test]
    fn validation_follows_own_conventions() {
        let json = |text: &str| text.parse::<Value>().unwrap();
        let nullable = r#"{"type":["number","null"],"minimum":0}"#;
        let cases = [
            (Value::from(vec![1_u8]), r#"{"type":"binary"}"#, vec![]),
            (
                json(r#""AQ==""#),
                r#"{"type":"binary"}"#,
                vec!["type \"binary\""],
            ),
            (Value::Nil, nullable, vec![]),
            (json("-1"), nullable, vec!["minimum 0"]),
            (json("-4"), r#"{"multipleOf":2}"#, vec![]),
            (json("-3.0"), r#"{"multipleOf":2}"#, vec!["multipleOf 2"]),
            (json("4294967297"), r#"{"enum":[1]}"#, vec!["enum"]),
            (json(r#""x""#), r#"{"enum":[]}"#, vec![]),
            (json(r#""毫米""#), r#"{"maxLength":2}"#, vec![]),
        ];
        for (value, schema, expected) in cases {
            assert_eq!(
                validate(&value, &json(schema)),
                expected,
                "{schema} {value}"
            );
        }

        let schema = json(
            r#"{"properties":{"":{"type":"integer"},"l":{"type":["array","null"],"items":{"type":"number"}}}}"#,
        );
        assert_eq!(
            schema_for_path(&schema, "/"),
            schema.pointer("/properties/")
        );
        assert_eq!(
            schema_for_path(&schema, "/l/3"),
            schema.pointer("/properties/l/items")
        );
    }
}
