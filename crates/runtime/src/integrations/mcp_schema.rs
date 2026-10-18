//! Deliberately bounded JSON Schema subset. Unsupported assertions fail closed.
use serde_json::Value;

const MAX_DEPTH: usize = 32;

pub(crate) fn validate_schema(schema: &Value) -> Result<(), String> {
    schema_at(schema, 0)
}
fn schema_at(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("MCP schema exceeds nesting limit".into());
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or("MCP schema must be an object or boolean")?;
    for key in object.keys() {
        if ![
            "type",
            "properties",
            "required",
            "additionalProperties",
            "items",
            "enum",
            "const",
            "minItems",
            "maxItems",
            "minLength",
            "maxLength",
            "minimum",
            "maximum",
            "title",
            "description",
            "default",
            "examples",
            "$schema",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("unsupported MCP schema keyword: {key}"));
        }
    }
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("MCP schema requires an explicit type")?;
    if ![
        "object", "array", "string", "number", "integer", "boolean", "null",
    ]
    .contains(&kind)
    {
        return Err("unsupported MCP schema type".into());
    }
    if let Some(properties) = object.get("properties") {
        if kind != "object" {
            return Err("MCP properties require object type".into());
        }
        let properties = properties
            .as_object()
            .ok_or("invalid MCP schema properties")?;
        if properties.len() > 128 {
            return Err("MCP schema has too many properties".into());
        }
        for property in properties.values() {
            schema_at(property, depth + 1)?;
        }
    }
    if let Some(required) = object.get("required") {
        if kind != "object"
            || !required
                .as_array()
                .is_some_and(|a| a.len() <= 128 && a.iter().all(Value::is_string))
        {
            return Err("invalid MCP required properties".into());
        }
    }
    if let Some(additional) = object.get("additionalProperties") {
        if kind != "object" {
            return Err("MCP additionalProperties requires object type".into());
        }
        schema_at(additional, depth + 1)?;
    }
    if let Some(items) = object.get("items") {
        if kind != "array" {
            return Err("MCP items requires array type".into());
        }
        schema_at(items, depth + 1)?;
    }
    for (key, expected) in [
        ("minItems", "array"),
        ("maxItems", "array"),
        ("minLength", "string"),
        ("maxLength", "string"),
    ] {
        if let Some(value) = object.get(key) {
            if kind != expected || value.as_u64().is_none() {
                return Err(format!("invalid MCP schema {key}"));
            }
        }
    }
    for key in ["minimum", "maximum"] {
        if let Some(value) = object.get(key) {
            if !matches!(kind, "number" | "integer")
                || !value
                    .as_f64()
                    .is_some_and(|n| n.abs() <= 9_007_199_254_740_991.0)
            {
                return Err(format!("invalid MCP schema {key}"));
            }
        }
    }
    if let Some(values) = object.get("enum") {
        if !values
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.len() <= 128)
        {
            return Err("invalid MCP enum".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_instance(schema: &Value, value: &Value) -> Result<(), String> {
    validate_schema(schema)?;
    instance_at(schema, value, 0)
}
fn instance_at(schema: &Value, value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("MCP arguments exceed nesting limit".into());
    }
    if schema == &Value::Bool(true) {
        return Ok(());
    }
    if schema == &Value::Bool(false) {
        return Err("MCP value rejected by schema".into());
    }
    let kind = schema["type"].as_str().ok_or("invalid MCP schema type")?;
    let valid = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|v| v.fract() == 0.0),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !valid {
        return Err("MCP value does not match schema type".into());
    }
    if let Some(values) = schema.get("enum") {
        if !values
            .as_array()
            .unwrap()
            .iter()
            .any(|expected| schema_equal(expected, value))
        {
            return Err("MCP value not in schema enum".into());
        }
    }
    if let Some(expected) = schema.get("const") {
        if !schema_equal(expected, value) {
            return Err("MCP value does not match schema const".into());
        }
    }
    if let Some(properties) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for key in required {
                if !properties.contains_key(key.as_str().unwrap()) {
                    return Err("MCP arguments missing required property".into());
                }
            }
        }
        for (key, property) in properties {
            if let Some(property_schema) = schema["properties"].get(key) {
                instance_at(property_schema, property, depth + 1)?;
            } else if let Some(additional) = schema.get("additionalProperties") {
                instance_at(additional, property, depth + 1)?;
            }
        }
    }
    if let Some(array) = value.as_array() {
        length(schema, array.len() as u64, "minItems", "maxItems")?;
        if let Some(items) = schema.get("items") {
            for item in array {
                instance_at(items, item, depth + 1)?;
            }
        }
    }
    if let Some(string) = value.as_str() {
        length(
            schema,
            string.chars().count() as u64,
            "minLength",
            "maxLength",
        )?;
    }
    if let Some(number) = value.as_f64() {
        if number.abs() > 9_007_199_254_740_991.0 {
            return Err("MCP numeric value exceeds supported precise range".into());
        }
        if schema["minimum"].as_f64().is_some_and(|min| number < min)
            || schema["maximum"].as_f64().is_some_and(|max| number > max)
        {
            return Err("MCP number outside schema bounds".into());
        }
    }
    Ok(())
}

/// JSON Schema equality treats numeric spellings equally, including in containers.
fn schema_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => match (left.is_f64(), right.is_f64()) {
            (false, false) => left == right,
            (true, true) => left.as_f64() == right.as_f64(),
            (true, false) => float_equals_integer(left.as_f64().unwrap(), right),
            (false, true) => float_equals_integer(right.as_f64().unwrap(), left),
        },
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| schema_equal(a, b))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| schema_equal(value, other))
                })
        }
        _ => left == right,
    }
}

fn float_equals_integer(float: f64, integer: &serde_json::Number) -> bool {
    if float.fract() != 0.0 {
        return false;
    }
    // Check exclusive upper bounds before Rust's saturating float-to-int cast.
    // Converting the integer to f64 instead would equate distinct integers above 2^53.
    if let Some(integer) = integer.as_i64() {
        (i64::MIN as f64..9_223_372_036_854_775_808.0).contains(&float) && float as i64 == integer
    } else {
        (0.0..18_446_744_073_709_551_616.0).contains(&float)
            && Some(float as u64) == integer.as_u64()
    }
}

fn length(schema: &Value, length: u64, min: &str, max: &str) -> Result<(), String> {
    if schema[min].as_u64().is_some_and(|bound| length < bound)
        || schema[max].as_u64().is_some_and(|bound| length > bound)
    {
        return Err("MCP value length outside schema bounds".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enum_and_const_use_numeric_equality_recursively() {
        for keyword in ["enum", "const"] {
            for (kind, expected, actual) in [
                ("number", json!(1.0), json!(1)),
                ("number", json!(1), json!(1.0)),
                ("integer", json!(-0.0), json!(0)),
                (
                    "array",
                    json!([1.0, {"count": -2.0}]),
                    json!([1, {"count": -2}]),
                ),
                (
                    "object",
                    json!({"items": [1.0], "other": 2}),
                    json!({"other": 2.0, "items": [1]}),
                ),
            ] {
                let mut schema = json!({"type": kind});
                schema[keyword] = if keyword == "enum" {
                    json!([expected])
                } else {
                    expected
                };
                assert!(
                    validate_instance(&schema, &actual).is_ok(),
                    "{schema}: {actual}"
                );
            }
            for (expected, actual) in [
                (json!([1]), json!([1.5])),
                (json!([1, 2]), json!([2, 1])),
                (json!([1]), json!(["1"])),
                (json!([{"count": 1}]), json!([{"other": 1}])),
                (json!([{"count": 1}]), json!([{"count": 1, "other": 2}])),
                (
                    json!([9_007_199_254_740_993_u64]),
                    json!([9_007_199_254_740_992.0]),
                ),
                (json!([u64::MAX]), json!([18_446_744_073_709_551_616.0])),
                (json!([i64::MAX]), json!([9_223_372_036_854_775_808.0])),
            ] {
                let mut schema = json!({"type": "array"});
                schema[keyword] = if keyword == "enum" {
                    json!([expected])
                } else {
                    expected
                };
                assert!(
                    validate_instance(&schema, &actual).is_err(),
                    "{schema}: {actual}"
                );
            }
        }
    }

    #[test]
    fn numeric_equality_preserves_exact_integer_boundaries() {
        for (left, right) in [
            (json!(i64::MIN), json!(-9_223_372_036_854_775_808.0)),
            (
                json!(9_223_372_036_854_775_808_u64),
                json!(9_223_372_036_854_775_808.0),
            ),
            (
                json!(9_007_199_254_740_993_u64),
                json!(9_007_199_254_740_993_u64),
            ),
        ] {
            assert!(schema_equal(&left, &right));
            assert!(schema_equal(&right, &left));
        }
        assert!(!schema_equal(
            &json!(9_007_199_254_740_992_u64),
            &json!(9_007_199_254_740_993_u64)
        ));
    }
    #[test]
    fn validates_nested_schema_without_remote_resolution() {
        let schema = json!({"type":"object", "properties":{"count":{"type":"integer","minimum":1,"maximum":3}},"required":["count"],"additionalProperties":false});
        assert!(validate_instance(&schema, &json!({"count":2})).is_ok());
        for value in [
            json!({}),
            json!({"count":4}),
            json!({"count":2,"extra":1}),
            json!({"count":"2"}),
        ] {
            assert!(validate_instance(&schema, &value).is_err());
        }
        for schema in [
            json!({"type":"string","pattern":"(a+)+"}),
            json!({"$ref":"https://evil/schema"}),
            json!({"type":"object","additionalProperties":{"$ref":"file:///secret"}}),
        ] {
            assert!(validate_schema(&schema).is_err());
        }
    }
}
