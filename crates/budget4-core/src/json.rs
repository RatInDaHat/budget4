//! JSON reading and writing that matches ActionScript/JavaScript `JSON.stringify` byte for byte,
//! which is what YNAB 4 produces. Object key order is preserved (serde_json `preserve_order`).

use serde_json::{Map, Number, Value};

pub type Obj = Map<String, Value>;

/// Parses JSON, ignoring a UTF-8 byte-order mark.
pub fn parse(text: &str) -> serde_json::Result<Value> {
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// A JSON number from an f64, as JavaScript would hold it (written as "50", not "50.0").
pub fn num(x: f64) -> Value {
    Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

/// Numeric value of a JSON number (or 0).
pub fn as_f64(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// Equality that treats 50 and 50.0 as the same number.
pub fn value_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| value_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| value_eq(v, w)))
        }
        _ => a == b,
    }
}

fn write_number(out: &mut String, n: &Number) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else {
        let f = n.as_f64().unwrap_or(0.0);
        if f.is_finite() {
            out.push_str(ryu_js::Buffer::new().format(f));
        } else {
            out.push_str("null");
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `JSON.stringify(value, replacer, indent)` where the replacer drops a property when `omit(key, value)`.
pub fn stringify(value: &Value, indent: &str, omit: &dyn Fn(&str, &Value) -> bool) -> String {
    let mut out = String::with_capacity(1 << 16);
    write(&mut out, value, indent, 0, omit);
    out
}

fn newline(out: &mut String, indent: &str, level: usize) {
    if !indent.is_empty() {
        out.push('\n');
        for _ in 0..level {
            out.push_str(indent);
        }
    }
}

fn write(out: &mut String, v: &Value, indent: &str, level: usize, omit: &dyn Fn(&str, &Value) -> bool) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, level + 1);
                write(out, x, indent, level + 1, omit);
            }
            newline(out, indent, level);
            out.push(']');
        }
        Value::Object(m) => {
            let entries: Vec<(&String, &Value)> = m.iter().filter(|(k, v)| !omit(k, v)).collect();
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, level + 1);
                write_string(out, k);
                out.push(':');
                if !indent.is_empty() {
                    out.push(' ');
                }
                write(out, x, indent, level + 1, omit);
            }
            newline(out, indent, level);
            out.push('}');
        }
    }
}

/// Helpers for reading and writing fields of a JSON object.
pub trait ObjExt {
    fn str_of(&self, k: &str) -> Option<&str>;
    fn f(&self, k: &str) -> f64;
    fn b(&self, k: &str) -> bool;
    fn is_null(&self, k: &str) -> bool;
    fn id(&self) -> &str;
    fn etype(&self) -> &str;
    fn tomb(&self) -> bool;
}

impl ObjExt for Obj {
    fn str_of(&self, k: &str) -> Option<&str> {
        self.get(k).and_then(Value::as_str)
    }
    fn f(&self, k: &str) -> f64 {
        self.get(k).map(as_f64).unwrap_or(0.0)
    }
    fn b(&self, k: &str) -> bool {
        self.get(k).and_then(Value::as_bool).unwrap_or(false)
    }
    fn is_null(&self, k: &str) -> bool {
        self.get(k).is_none_or(Value::is_null)
    }
    fn id(&self) -> &str {
        self.str_of("entityId").unwrap_or("")
    }
    fn etype(&self) -> &str {
        self.str_of("entityType").unwrap_or("")
    }
    fn tomb(&self) -> bool {
        self.b("isTombstone")
    }
}

/// `Some(s)` → string, `None` → null.
pub fn opt_str(s: Option<&str>) -> Value {
    s.map(|x| Value::String(x.to_string())).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_like_javascript() {
        let v = parse(r#"{"a":[],"b":{},"c":-56.55,"d":50,"e":"q\"\u0001","f":[1,{"g":null}]}"#).unwrap();
        let mut obj = v.as_object().unwrap().clone();
        obj.insert("h".into(), num(50.0));
        obj.insert("i".into(), num(0.1 + 0.2));
        let s = stringify(&Value::Object(obj), "\t", &|_, _| false);
        assert_eq!(
            s,
            "{\n\t\"a\": [],\n\t\"b\": {},\n\t\"c\": -56.55,\n\t\"d\": 50,\n\t\"e\": \"q\\\"\\u0001\",\n\t\"f\": [\n\t\t1,\n\t\t{\n\t\t\t\"g\": null\n\t\t}\n\t],\n\t\"h\": 50,\n\t\"i\": 0.30000000000000004\n}"
        );
    }

    #[test]
    fn omits_with_replacer() {
        let v = parse(r#"{"x":null,"y":false,"z":1}"#).unwrap();
        assert_eq!(stringify(&v, "", &|_, v| v.is_null() || v == &Value::Bool(false)), r#"{"z":1}"#);
    }
}
