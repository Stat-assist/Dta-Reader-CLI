//! A tiny, dependency-free JSON value type and serializer.
//!
//! We only need serialization (the CLI never parses JSON), so this is intentionally minimal.
//! Objects preserve insertion order (a `Vec` of pairs, not a map) so output field order is stable
//! and readable. The `Num` variant carries an already-formatted numeric *string* rather than an
//! `f64`, which lets callers control exact precision (e.g. Stata's float-vs-double significant-digit
//! rules) and emit it unquoted; the string must be a valid JSON number.

use std::fmt::Write as _;

#[derive(Debug, Clone)]
pub enum Json {
    Null,
    Bool(bool),
    /// A pre-formatted, valid JSON number emitted without quotes (e.g. "3.5799999", "42", "1e-9").
    Num(String),
    Str(String),
    Array(Vec<Json>),
    /// Insertion-ordered object.
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn int(v: i64) -> Json {
        Json::Num(v.to_string())
    }

    pub fn str<S: Into<String>>(s: S) -> Json {
        Json::Str(s.into())
    }

    /// Serialize with 2-space indentation (pretty) or as a single compact line.
    pub fn to_string(&self, pretty: bool) -> String {
        let mut out = String::new();
        self.write(&mut out, pretty, 0);
        out
    }

    fn write(&self, out: &mut String, pretty: bool, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(n),
            Json::Str(s) => write_json_string(out, s),
            Json::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline_indent(out, pretty, depth + 1);
                    item.write(out, pretty, depth + 1);
                }
                newline_indent(out, pretty, depth);
                out.push(']');
            }
            Json::Object(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline_indent(out, pretty, depth + 1);
                    write_json_string(out, key);
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    value.write(out, pretty, depth + 1);
                }
                newline_indent(out, pretty, depth);
                out.push('}');
            }
        }
    }
}

fn newline_indent(out: &mut String, pretty: bool, depth: usize) {
    if pretty {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    }
}

fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
