//! `todo_write`: a local tool (handled by the chat, not `agent_tools`) that lets the model
//! keep a task list, shown in the transcript and the right panel.

use serde_json::{Value, json};

pub const TODO_TOOL: &str = "todo_write";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

pub fn spec() -> llm::ToolSpec {
    llm::ToolSpec {
        name: TODO_TOOL.into(),
        description: "Create or replace the task list for the current work. Send the full list \
            every time. Use it for multi-step tasks; keep exactly one item in_progress while \
            working and mark items completed as soon as they are done."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string", "description": "Short imperative task" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["items"]
        }),
    }
}

/// Parse the tool input. Errors are returned to the model as an `is_error` result.
pub fn parse(input: &Value) -> Result<Vec<TodoItem>, String> {
    // Some gateways deliver the arguments as a JSON-encoded string.
    if let Value::String(s) = input {
        let v: Value = serde_json::from_str(s).map_err(|e| format!("invalid JSON input: {e}"))?;
        return parse(&v);
    }
    let items = input
        .get("items")
        .and_then(Value::as_array)
        .ok_or("`items` must be an array")?;
    items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let content = it
                .get("content")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or(format!("items[{i}].content must be a non-empty string"))?;
            let status = match it.get("status").and_then(Value::as_str) {
                Some("pending") => TodoStatus::Pending,
                Some("in_progress") => TodoStatus::InProgress,
                Some("completed") => TodoStatus::Completed,
                other => {
                    return Err(format!(
                        "items[{i}].status must be pending, in_progress or completed (got {})",
                        other.unwrap_or("nothing")
                    ));
                }
            };
            Ok(TodoItem {
                content: content.to_string(),
                status,
            })
        })
        .collect()
}

pub fn done_count(items: &[TodoItem]) -> usize {
    items
        .iter()
        .filter(|t| t.status == TodoStatus::Completed)
        .count()
}

/// Tool result text sent back to the model.
pub fn summary(items: &[TodoItem]) -> String {
    let mut s = format!(
        "Todo list updated ({} of {} completed).",
        done_count(items),
        items.len()
    );
    for t in items {
        let mark = match t.status {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Completed => "[x]",
        };
        s.push_str(&format!("\n{mark} {}", t.content));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_items() {
        let input = json!({"items": [
            {"content": "Read the code", "status": "completed"},
            {"content": " Fix it ", "status": "in_progress"},
            {"content": "Test", "status": "pending"},
        ]});
        let items = parse(&input).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].content, "Fix it");
        assert_eq!(items[1].status, TodoStatus::InProgress);
        assert_eq!(done_count(&items), 1);
        let s = summary(&items);
        assert!(s.starts_with("Todo list updated (1 of 3 completed)."));
        assert!(s.contains("[~] Fix it"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&json!({})).is_err());
        assert!(parse(&json!({"items": [{"content": "x", "status": "done"}]})).is_err());
        assert!(parse(&json!({"items": [{"content": "", "status": "pending"}]})).is_err());
        assert_eq!(parse(&json!({"items": []})).unwrap(), vec![]);
        let s = json!(r#"{"items":[{"content":"x","status":"pending"}]}"#);
        assert_eq!(parse(&s).unwrap().len(), 1);
    }

    #[test]
    fn spec_is_named() {
        let s = spec();
        assert_eq!(s.name, TODO_TOOL);
        assert_eq!(s.input_schema["required"][0], "items");
    }
}
