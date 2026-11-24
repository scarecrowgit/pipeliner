//! `todo_write`: the model's task list, shown beside the transcript.

use model::{TodoItem, TodoStatus, ToolSpec};
use serde_json::{Value, json};

use super::names::TODO_WRITE;
use super::object;

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        name: TODO_WRITE.into(),
        description: "Create or replace the task list of the current work. Send the full list every time. Use it \
                      for work with several steps; keep exactly one item in_progress while working and mark \
                      items done as soon as they are finished."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "A short imperative task" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "done"] }
                        },
                        "required": ["text", "status"]
                    }
                }
            },
            "required": ["items"]
        }),
    }
}

/// The list in the input. `content` is accepted for `text` and `completed` for `done`, as
/// models used to other harnesses send them.
pub(crate) fn parse(input: &Value) -> Result<Vec<TodoItem>, String> {
    let map = object(TODO_WRITE, input)?;
    let items = map.get("items").and_then(Value::as_array).ok_or("todo_write: `items` must be an array.")?;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let text = ["text", "content"]
                .iter()
                .find_map(|key| item.get(key).and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("todo_write: items[{i}].text must be a non-empty string."))?;
            let status = match item.get("status").and_then(Value::as_str) {
                Some("pending") | None => TodoStatus::Pending,
                Some("in_progress") => TodoStatus::InProgress,
                Some("done" | "completed") => TodoStatus::Done,
                Some(other) => {
                    return Err(format!(
                        "todo_write: items[{i}].status must be pending, in_progress or done (got `{other}`)."
                    ));
                }
            };
            Ok(TodoItem { text: text.to_string(), status })
        })
        .collect()
}

/// The list as lines: `[x] done`, `[~] in progress`, `[ ] pending`.
pub(crate) fn render(items: &[TodoItem]) -> String {
    let mut out = String::new();
    for item in items {
        let mark = match item.status {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Done => "[x]",
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(mark);
        out.push(' ');
        out.push_str(&item.text);
    }
    out
}

/// What the model reads back after an update.
pub(crate) fn summary(items: &[TodoItem]) -> String {
    let done = items.iter().filter(|t| t.status == TodoStatus::Done).count();
    if items.is_empty() {
        return "Todo list cleared.".to_string();
    }
    format!("Todo list updated ({done} of {} done).\n{}", items.len(), render(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_parse_with_either_spelling() {
        let input = json!({"items": [
            {"text": "Read the code", "status": "done"},
            {"content": " Fix it ", "status": "in_progress"},
            {"text": "Test", "status": "pending"},
            {"text": "Ship", "status": "completed"},
            {"text": "Later"},
        ]});
        let items = parse(&input).expect("valid");
        assert_eq!(items.len(), 5);
        assert_eq!(items[1], TodoItem { text: "Fix it".into(), status: TodoStatus::InProgress });
        assert_eq!(items[3].status, TodoStatus::Done);
        assert_eq!(items[4].status, TodoStatus::Pending);
        assert_eq!(
            summary(&items),
            "Todo list updated (2 of 5 done).\n[x] Read the code\n[~] Fix it\n[ ] Test\n[x] Ship\n[ ] Later"
        );
    }

    #[test]
    fn bad_input_says_what_is_wrong() {
        assert!(parse(&json!({})).unwrap_err().contains("`items`"));
        assert!(parse(&json!({"items": [{"text": "x", "status": "soon"}]})).unwrap_err().contains("soon"));
        assert!(parse(&json!({"items": [{"text": " "}]})).unwrap_err().contains("items[0].text"));
        assert!(parse(&json!(7)).is_err());
        assert_eq!(parse(&json!({"items": []})).expect("empty"), vec![]);
        assert_eq!(summary(&[]), "Todo list cleared.");
        let encoded = json!(r#"{"items":[{"text":"x","status":"pending"}]}"#);
        assert_eq!(parse(&encoded).expect("encoded").len(), 1);
    }
}
