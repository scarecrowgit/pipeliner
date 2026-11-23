//! Reading a tool call's JSON input, with errors that tell the model what to send instead.

use std::borrow::Cow;

use serde_json::{Map, Value};

/// The fields of one tool call. Built from the raw input; every getter explains a bad value.
pub(crate) struct Args<'a> {
    tool: &'static str,
    /// A one-line example of valid input, appended to every error.
    usage: &'static str,
    map: Cow<'a, Map<String, Value>>,
}

impl<'a> Args<'a> {
    /// Accepts a JSON object, or a string holding one (some models send the input encoded).
    pub(crate) fn new(tool: &'static str, usage: &'static str, input: &'a Value) -> Result<Self, String> {
        let map = match input {
            Value::Object(map) => Cow::Borrowed(map),
            Value::String(text) => match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(map)) => Cow::Owned(map),
                _ => return Err(format!("{tool}: the input must be a JSON object, for example {usage}")),
            },
            Value::Null => Cow::Owned(Map::new()),
            _ => return Err(format!("{tool}: the input must be a JSON object, for example {usage}")),
        };
        Ok(Self { tool, usage, map })
    }

    fn fail(&self, what: String) -> String {
        format!("{}: {what}. Example input: {}", self.tool, self.usage)
    }

    /// A required string field.
    pub(crate) fn str(&self, key: &str) -> Result<&str, String> {
        match self.map.get(key) {
            Some(Value::String(s)) => Ok(s),
            None | Some(Value::Null) => Err(self.fail(format!("missing required field `{key}` (a string)"))),
            Some(other) => Err(self.fail(format!("`{key}` must be a string, not {}", kind(other)))),
        }
    }

    /// An optional string field; `null` counts as missing.
    pub(crate) fn opt_str(&self, key: &str) -> Result<Option<&str>, String> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(other) => Err(self.fail(format!("`{key}` must be a string, not {}", kind(other)))),
        }
    }

    /// An optional non-negative whole number; numeric strings and whole floats are accepted.
    pub(crate) fn opt_u64(&self, key: &str) -> Result<Option<u64>, String> {
        let bad = |v: &Value| self.fail(format!("`{key}` must be a non-negative whole number, not {}", kind(v)));
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v @ Value::Number(n)) => match (n.as_u64(), n.as_f64()) {
                (Some(u), _) => Ok(Some(u)),
                (None, Some(f)) if f >= 0.0 && f.fract() == 0.0 && f < 1e15 => Ok(Some(f as u64)),
                _ => Err(bad(v)),
            },
            Some(v @ Value::String(s)) => s.trim().parse().map(Some).map_err(|_| bad(v)),
            Some(v) => Err(bad(v)),
        }
    }

    /// An optional boolean; the strings `"true"` and `"false"` are accepted.
    pub(crate) fn opt_bool(&self, key: &str) -> Result<Option<bool>, String> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(Value::String(s)) if s == "true" => Ok(Some(true)),
            Some(Value::String(s)) if s == "false" => Ok(Some(false)),
            Some(other) => Err(self.fail(format!("`{key}` must be true or false, not {}", kind(other)))),
        }
    }
}

/// The JSON kind of a value, for error messages: `a number (12)`.
fn kind(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => format!("a boolean ({b})"),
        Value::Number(n) => format!("a number ({n})"),
        Value::String(s) => format!("a string ({:?})", base::fmt::truncate_chars(s, 40)),
        Value::Array(_) => "an array".into(),
        Value::Object(_) => "an object".into(),
    }
}

/// The string field `key` of a raw input, for `subject` and `preview`, which must not fail.
pub(crate) fn peek_str<'v>(input: &'v Value, key: &str) -> Option<Cow<'v, str>> {
    match input {
        Value::Object(map) => map.get(key).and_then(Value::as_str).map(Cow::Borrowed),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map.get(key).and_then(Value::as_str).map(|s| Cow::Owned(s.to_string())),
            _ => None,
        },
        _ => None,
    }
}
