//! The finish tool of a pipeline node: its spec, the check of its input against the schema,
//! and the reminder for a model that stopped without calling it.

use model::ToolSpec;
use serde_json::Value;

use super::object;
use crate::spec::FinishTool;

/// Reminders sent to a model that stops without calling the finish tool.
pub(crate) const MAX_NUDGES: u32 = 2;

pub(crate) fn spec(finish: &FinishTool) -> ToolSpec {
    ToolSpec {
        name: finish.name.clone(),
        description: finish.description.clone(),
        input_schema: finish.schema.clone(),
    }
}

/// The input as the result of the step, when it has every `required` field and each field
/// the schema describes has the `type` it names. Deeper rules of the schema are left to the
/// model.
pub(crate) fn parse(finish: &FinishTool, input: &Value) -> Result<Value, String> {
    let name = finish.name.as_str();
    let map = object(name, input)?;
    let required = finish.schema.get("required").and_then(Value::as_array);
    let missing: Vec<&str> = required
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| map.get(*key).is_none_or(Value::is_null))
        .collect();
    if !missing.is_empty() {
        return Err(format!("{name}: missing required field(s): {}. Call it again with all of them.", missing.join(", ")));
    }
    let properties = finish.schema.get("properties").and_then(Value::as_object);
    for (key, schema) in properties.into_iter().flatten() {
        let (Some(value), Some(expected)) = (map.get(key), schema.get("type").and_then(Value::as_str)) else {
            continue;
        };
        if !value.is_null() && !has_type(value, expected) {
            return Err(format!("{name}: `{key}` must be of type {expected}. Call it again with a valid value."));
        }
    }
    Ok(Value::Object(map.into_owned()))
}

fn has_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

/// What the model reads back.
pub(crate) const RECORDED: &str = "Recorded. The step is finished.";

/// The reminder for a model that ended its turn without the finish tool.
pub(crate) fn nudge(finish: &FinishTool) -> String {
    format!(
        "You stopped without calling `{name}`. If the work is complete, call `{name}` now with the result. If \
         it is not, continue working and call `{name}` when it is.",
        name = finish.name
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool() -> FinishTool {
        FinishTool::step_done(json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "verified": { "type": "boolean" },
                "files": { "type": "array" },
                "note": {}
            },
            "required": ["summary", "verified"]
        }))
    }

    #[test]
    fn input_that_fits_is_the_payload() {
        let input = json!({"summary": "Done.", "verified": true, "extra": 1});
        assert_eq!(parse(&tool(), &input).expect("fits"), input);
        let encoded = json!(r#"{"summary":"Done.","verified":false}"#);
        assert_eq!(parse(&tool(), &encoded).expect("fits"), json!({"summary": "Done.", "verified": false}));
    }

    #[test]
    fn missing_and_mistyped_fields_are_named() {
        let err = parse(&tool(), &json!({"summary": "Done."})).unwrap_err();
        assert!(err.contains("step_done: missing required field(s): verified"), "{err}");
        let err = parse(&tool(), &json!({"summary": "Done.", "verified": "yes"})).unwrap_err();
        assert!(err.contains("`verified` must be of type boolean"), "{err}");
        let err = parse(&tool(), &json!({"summary": null, "verified": null})).unwrap_err();
        assert!(err.contains("summary, verified"), "{err}");
        assert!(parse(&tool(), &json!([1])).is_err());
        assert!(nudge(&tool()).contains("`step_done`"));
    }
}
