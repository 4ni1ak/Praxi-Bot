//! Composio's tool schemas and descriptions are untrusted text from a third
//! party.  They are reduced to a safe subset before the model sees them, and
//! arguments are checked against that subset before anything is sent.

use serde_json::{Map, Value, json};

const MAX_DEPTH: usize = 5;
const MAX_PROPS: usize = 60;
const MAX_DESC: usize = 300;
const MAX_ENUM: usize = 50;

/// Strip control characters and cap the length of a description.
#[must_use]
pub fn clean_text(text: &str, max: usize) -> String {
    let s: String = text
        .chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .take(max)
        .collect();
    s.trim().to_owned()
}

/// Reduce supported schemas without dropping validation constraints. Unsupported
/// or malformed schemas become `false` (never executable), not an open object.
#[must_use]
pub fn sanitize(schema: &Value) -> Value {
    sanitize_node(schema, schema, 0)
        .filter(|s| s.get("type").is_some_and(|t| t == "object"))
        .unwrap_or(Value::Bool(false))
}

fn sanitize_node(v: &Value, root: &Value, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let obj = v.as_object()?;
    if let Some(reference) = obj.get("$ref") {
        if obj
            .keys()
            .any(|k| !matches!(k.as_str(), "$ref" | "description" | "title"))
        {
            return None;
        }
        let pointer = reference.as_str()?.strip_prefix('#')?;
        if !pointer.starts_with("/$defs/") && !pointer.starts_with("/definitions/") {
            return None;
        }
        return sanitize_node(root.pointer(pointer)?, root, depth + 1);
    }
    let mut out = Map::new();
    if obj.keys().any(|key| !supported_keyword(key)) {
        return None;
    }
    let ty = obj
        .get("type")
        .cloned()
        .or_else(|| obj.contains_key("properties").then(|| json!("object")))?;
    let valid = |t: &Value| {
        t.as_str().is_some_and(|t| {
            matches!(
                t,
                "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
            )
        })
    };
    if !(valid(&ty)
        || ty
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(valid)))
    {
        return None;
    }
    out.insert("type".into(), ty.clone());
    if let Some(d) = obj.get("description") {
        out.insert(
            "description".into(),
            json!(clean_text(d.as_str()?, MAX_DESC)),
        );
    }
    for key in [
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
    ] {
        if let Some(n) = obj.get(key) {
            if !n.is_number() || (!matches!(key, "minimum" | "maximum") && n.as_u64().is_none()) {
                return None;
            }
            out.insert(key.into(), n.clone());
        }
    }
    if let Some(e) = obj.get("enum") {
        let e = e.as_array()?;
        if e.is_empty() || e.len() > MAX_ENUM {
            return None;
        }
        out.insert("enum".into(), json!(e));
    }
    let has_type = |t| ty == t || ty.as_array().is_some_and(|ts| ts.contains(&json!(t)));
    if has_type("object") {
        out.extend(sanitize_object(obj, root, depth)?);
    }
    if has_type("array") {
        out.insert(
            "items".into(),
            sanitize_node(obj.get("items")?, root, depth + 1)?,
        );
    }
    Some(Value::Object(out))
}

fn supported_keyword(key: &str) -> bool {
    matches!(
        key,
        "type"
            | "properties"
            | "required"
            | "items"
            | "additionalProperties"
            | "enum"
            | "minimum"
            | "maximum"
            | "minLength"
            | "maxLength"
            | "minItems"
            | "maxItems"
            | "description"
            | "title"
            | "default"
            | "examples"
            | "$defs"
            | "definitions"
            | "$schema"
    )
}

fn sanitize_object(
    obj: &Map<String, Value>,
    root: &Value,
    depth: usize,
) -> Option<Map<String, Value>> {
    let mut out = Map::new();
    let empty = Map::new();
    let props = match obj.get("properties") {
        Some(p) => p.as_object()?,
        None => &empty,
    };
    if props.len() > MAX_PROPS {
        return None;
    }
    let mut sanitized = Map::new();
    for (k, v) in props {
        if k.len() > 128 || k.chars().any(char::is_control) {
            return None;
        }
        sanitized.insert(k.clone(), sanitize_node(v, root, depth + 1)?);
    }
    // Conservative closed objects, including schemas with no properties.
    // Explicit dynamic maps are unsupported until they can be validated.
    if obj.get("additionalProperties").is_some_and(|v| v != false) {
        return None;
    }
    out.insert("additionalProperties".into(), json!(false));
    if let Some(required) = obj.get("required") {
        let r = required.as_array()?;
        if r.len() > MAX_PROPS
            || r.iter()
                .any(|k| k.as_str().is_none_or(|k| !props.contains_key(k)))
        {
            return None;
        }
        out.insert("required".into(), required.clone());
    }
    out.insert("properties".into(), Value::Object(sanitized));
    Some(out)
}

fn type_ok(ty: &Value, v: &Value) -> bool {
    let one = |t: &str| match t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "null" => v.is_null(),
        _ => false,
    };
    match ty {
        Value::String(t) => one(t),
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).any(one),
        _ => false,
    }
}

/// Validate `args` against a sanitized schema: an object; no key outside
/// `properties` (including empty objects); every `required` key present; each given
/// value of the declared type, recursively for nested objects and arrays.
///
/// # Errors
/// A message naming the first problem.
pub fn validate(schema: &Value, args: &Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("unsupported action schema; this action cannot run".into());
    }
    if args.to_string().len() > 32 * 1024 {
        return Err("arguments exceed the 32 KiB approval limit".into());
    }
    check(schema, args, "arguments", 0)
}

fn check(schema: &Value, v: &Value, path: &str, depth: usize) -> Result<(), String> {
    if let Some(ty) = schema.get("type")
        && !type_ok(ty, v)
    {
        return Err(format!("`{path}` has the wrong type"));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array)
        && !e.is_empty()
        && !e.contains(v)
    {
        return Err(format!("`{path}` is not one of the allowed values"));
    }
    if depth > MAX_DEPTH {
        return Err("schema depth exceeded".into());
    }
    if let (Some(obj), Some(props)) = (
        v.as_object(),
        schema.get("properties").and_then(Value::as_object),
    ) {
        {
            for key in obj.keys() {
                if !props.contains_key(key) {
                    return Err(format!("unknown argument `{path}.{key}`"));
                }
            }
        }
        if let Some(req) = schema.get("required").and_then(Value::as_array) {
            for r in req.iter().filter_map(Value::as_str) {
                if !obj.contains_key(r) {
                    return Err(format!("missing required argument `{path}.{r}`"));
                }
            }
        }
        for (k, sub) in props {
            if let Some(x) = obj.get(k) {
                check(sub, x, &format!("{path}.{k}"), depth + 1)?;
            }
        }
    }
    if let (Some(arr), Some(items)) = (v.as_array(), schema.get("items")) {
        for (i, x) in arr.iter().enumerate() {
            check(items, x, &format!("{path}[{i}]"), depth + 1)?;
        }
    }
    for (key, lower) in [("minimum", true), ("maximum", false)] {
        if let Some(bound) = schema.get(key).filter(|_| v.is_number()) {
            let integer = |n: &Value| {
                n.as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
            };
            let order = match (integer(v), integer(bound)) {
                (Some(value), Some(bound)) => Some(value.cmp(&bound)),
                _ => v.as_f64().and_then(|n| n.partial_cmp(&bound.as_f64()?)),
            };
            if order.is_some_and(|o| if lower { o.is_lt() } else { o.is_gt() }) {
                return Err(format!("`{path}` violates {key}"));
            }
        }
    }
    for (key, lower, measured) in [
        ("minLength", true, v.as_str().map(|s| s.chars().count())),
        ("maxLength", false, v.as_str().map(|s| s.chars().count())),
        ("minItems", true, v.as_array().map(Vec::len)),
        ("maxItems", false, v.as_array().map(Vec::len)),
    ] {
        if let (Some(bound), Some(n)) = (schema.get(key).and_then(Value::as_u64), measured)
            && (if lower {
                (n as u64) < bound
            } else {
                (n as u64) > bound
            })
        {
            return Err(format!("`{path}` violates {key}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_schemas_fail_closed() {
        for bad in [
            json!(null),
            json!("nope"),
            json!({}),
            json!({"$ref":"https://evil"}),
            json!({"type":"object","allOf":[]}),
            json!({"type":"object","additionalProperties":true}),
            json!({"type":"mystery"}),
        ] {
            assert!(
                validate(&sanitize(&bad), &json!({"anything":1})).is_err(),
                "{bad}"
            );
        }
        let empty = sanitize(&json!({"type":"object","properties":{}}));
        assert!(validate(&empty, &json!({})).is_ok());
        assert!(validate(&empty, &json!({"anything":1})).is_err());
    }

    #[test]
    fn local_refs_nulls_and_bounds_are_checked() {
        let s = sanitize(
            &json!({"type":"object", "$defs":{"n":{"type":"integer","minimum":1,"maximum":3}},
            "properties":{"n":{"$ref":"#/$defs/n"},"s":{"type":"string","minLength":2,"maxLength":4}}}),
        );
        assert!(validate(&s, &json!({"n":2,"s":"ok"})).is_ok());
        for bad in [
            json!({"n":null}),
            json!({"n":0}),
            json!({"n":4}),
            json!({"s":"a"}),
            json!({"s":"abcde"}),
        ] {
            assert!(validate(&s, &bad).is_err(), "{bad}");
        }
        assert_eq!(
            sanitize(&json!({"$defs":{"x":{"$ref":"#/$defs/x"}},"$ref":"#/$defs/x"})),
            false
        );
    }

    #[test]
    fn validation_checks_required_unknown_and_types() {
        let s = sanitize(
            &json!({"properties": {"n": {"type": "integer"}, "t": {"type": "string", "enum": ["a", "b"]}}, "required": ["n"]}),
        );
        assert!(validate(&s, &json!({"n": 1})).is_ok());
        assert!(validate(&s, &json!({})).unwrap_err().contains("missing"));
        assert!(
            validate(&s, &json!({"n": "x"}))
                .unwrap_err()
                .contains("type")
        );
        assert!(
            validate(&s, &json!({"n": 1, "user_id": "x"}))
                .unwrap_err()
                .contains("unknown")
        );
        assert!(validate(&s, &json!({"n": 1, "t": "c"})).is_err());
        assert!(validate(&s, &json!([1])).is_err());
    }
}
