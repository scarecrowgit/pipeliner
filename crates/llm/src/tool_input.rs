//! How the raw JSON a model streams for a tool call becomes the call's input.
//!
//! Clients never fail a whole response because one tool call carries broken JSON (a model
//! cut off by `max_tokens`, a gateway that mangles escapes). The call is still reported as a
//! [`StreamEvent::ToolUse`](crate::StreamEvent::ToolUse) with `{}` as its input and the
//! reason in `error`.

use serde_json::Value;

/// The input to report for a tool call, and the reason it was replaced by `{}` if it was.
/// Empty input counts as `{}` and is fine.
pub(crate) fn parse(raw: &str) -> (Value, Option<String>) {
    if raw.trim().is_empty() {
        return (empty(), None);
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => (value, None),
        Ok(_) => (empty(), Some("the tool input is not a JSON object".to_string())),
        Err(e) => (empty(), Some(format!("the tool input is not valid JSON ({e})"))),
    }
}

fn empty() -> Value {
    Value::Object(Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn objects_pass_and_everything_else_becomes_empty() {
        assert_eq!(parse(r#"{"a":1}"#), (json!({"a": 1}), None));
        assert_eq!(parse("  "), (json!({}), None));
        let (value, err) = parse(r#"{"a":"#);
        assert_eq!(value, json!({}));
        assert!(err.unwrap().contains("not valid JSON"));
        assert!(parse("[1]").1.unwrap().contains("not a JSON object"));
        assert_eq!(parse("{}").1, None);
    }
}
