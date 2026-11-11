//! JSON output that is pleasant to read and to diff.
//!
//! `serde_json`'s pretty printer puts every number on its own line, which turns a
//! room's 576-cell tile layer into a 600-line column. This printer keeps any array
//! of plain values on one line, so a tile layer reads as 18 rows and a position
//! as `[7.5, 4.25]`, while objects and arrays of objects still nest normally.

use serde_json::Value;

pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write(value, 0, &mut out);
    out.push('\n');
    out
}

fn is_leaf(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

fn write(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) if items.iter().all(is_leaf) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&item.to_string());
            }
            out.push(']');
        }
        Value::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                indent(depth + 1, out);
                write(item, depth + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(depth, out);
            out.push(']');
        }
        Value::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Value::Object(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                indent(depth + 1, out);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write(item, depth + 1, out);
                if index + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(depth, out);
            out.push('}');
        }
        leaf => out.push_str(&leaf.to_string()),
    }
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// A number rounded to four decimals, so positions do not carry float noise such
/// as `7.4659729999`.
pub fn number(value: f32) -> Value {
    let rounded = (value as f64 * 10_000.0).round() / 10_000.0;
    // Whole numbers print as `8.0`, which reads as a position, not a count.
    serde_json::Number::from_f64(if rounded == 0.0 { 0.0 } else { rounded })
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

pub fn pair(x: f32, y: f32) -> Value {
    Value::Array(vec![number(x), number(y)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_arrays_stay_on_one_line_and_nested_ones_break() {
        let value = json!({"size": [32, 18], "rows": [[1, 2], [3, 4]], "none": []});
        let text = to_string(&value);
        assert!(text.contains("\"size\": [32, 18]"), "{text}");
        assert!(text.contains("\"rows\": [\n    [1, 2],\n    [3, 4]\n  ]"), "{text}");
        assert!(text.contains("\"none\": []"), "{text}");
        // And it is still JSON.
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
    }

    #[test]
    fn numbers_lose_float_noise() {
        assert_eq!(number(7.465973).to_string(), "7.466");
        assert_eq!(number(-0.0).to_string(), "0.0");
        assert_eq!(pair(8.0, 0.25).to_string(), "[8.0,0.25]");
    }
}
