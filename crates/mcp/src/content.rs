//! Turning a `tools/call` result into the text the model reads.

use serde_json::Value;

use crate::client::CallResult;

/// The most result text handed on; the head is kept.
pub const MAX_RESULT_BYTES: usize = 100 * 1024;

/// Text blocks joined with blank lines; images, audio and binary resources become a short
/// placeholder such as `[image: image/png, 12 KB]`. `structuredContent` is shown only when
/// there is no text. Capped at [`MAX_RESULT_BYTES`].
pub(crate) fn format_call_result(v: &Value) -> CallResult {
    let mut parts: Vec<String> = Vec::new();
    let mut has_text = false;
    for block in v.get("content").and_then(Value::as_array).into_iter().flatten() {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("unknown");
        let field = |k: &str| block.get(k).and_then(Value::as_str);
        match kind {
            "text" => {
                has_text = true;
                parts.push(field("text").unwrap_or_default().to_string());
            }
            "image" | "audio" => parts.push(format!(
                "[{kind}: {}, {}]",
                field("mimeType").unwrap_or("unknown type"),
                size(field("data").unwrap_or_default())
            )),
            "resource" => {
                let res = |k: &str| block.get("resource").and_then(|r| r.get(k)).and_then(Value::as_str);
                let uri = res("uri").unwrap_or("?");
                match res("text") {
                    Some(text) => {
                        has_text = true;
                        parts.push(format!("[resource: {uri}]\n{text}"));
                    }
                    None => parts.push(format!(
                        "[resource: {uri}, {}, {}]",
                        res("mimeType").unwrap_or("unknown type"),
                        size(res("blob").unwrap_or_default())
                    )),
                }
            }
            "resource_link" => {
                let uri = field("uri").unwrap_or("?");
                match field("name") {
                    Some(name) if name != uri => parts.push(format!("[resource link: {name} ({uri})]")),
                    _ => parts.push(format!("[resource link: {uri}]")),
                }
            }
            other => parts.push(format!("[{other} content]")),
        }
    }
    if !has_text && let Some(structured) = v.get("structuredContent").filter(|s| !s.is_null()) {
        parts.push(serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string()));
    }
    let mut text = parts.join("\n\n");
    if text.trim().is_empty() {
        text = "(no output)".to_string();
    }
    CallResult { text: cap(text), is_error: v.get("isError").and_then(Value::as_bool).unwrap_or(false) }
}

/// The first [`MAX_RESULT_BYTES`] of `text` (on a character boundary) and a note saying how
/// much there was.
pub(crate) fn cap(mut text: String) -> String {
    if text.len() <= MAX_RESULT_BYTES {
        return text;
    }
    let total = text.len();
    let mut cut = MAX_RESULT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push_str(&format!(
        "\n\n[output truncated: showing the first {} of {}]",
        base::fmt::bytes(cut as u64),
        base::fmt::bytes(total as u64)
    ));
    text
}

/// Decoded size of base64 `data`.
fn size(data: &str) -> String {
    base::fmt::bytes((data.trim_end_matches('=').len() * 3 / 4) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_blocks() {
        let r = format_call_result(&json!({
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "image", "data": "A".repeat(4096), "mimeType": "image/png"},
                {"type": "audio", "data": "AAAA", "mimeType": "audio/wav"},
                {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "file body"}},
                {"type": "resource", "resource": {"uri": "file:///b.bin", "blob": "AAAA", "mimeType": "application/octet-stream"}},
                {"type": "resource_link", "uri": "file:///c", "name": "c"},
                {"type": "hologram"},
            ],
            "structuredContent": {"ignored": true},
        }));
        assert!(!r.is_error);
        let parts: Vec<&str> = r.text.split("\n\n").collect();
        assert_eq!(parts[0], "hello");
        assert!(parts[1].starts_with("[image: image/png, 3"), "{}", parts[1]);
        assert_eq!(parts[2], "[audio: audio/wav, 3 B]");
        assert_eq!(parts[3], "[resource: file:///a.txt]\nfile body");
        assert_eq!(parts[4], "[resource: file:///b.bin, application/octet-stream, 3 B]");
        assert_eq!(parts[5], "[resource link: c (file:///c)]");
        assert_eq!(parts[6], "[hologram content]");
        assert!(!r.text.contains("ignored"));
    }

    #[test]
    fn structured_content_errors_and_empty() {
        let r = format_call_result(&json!({"content": [], "structuredContent": {"answer": 42}, "isError": true}));
        assert!(r.is_error);
        assert_eq!(r.text, "{\n  \"answer\": 42\n}");
        assert_eq!(format_call_result(&json!({})).text, "(no output)");
        assert_eq!(format_call_result(&json!(null)).text, "(no output)");
    }

    #[test]
    fn huge_results_are_cut_on_a_char_boundary() {
        let big = "é".repeat(80 * 1024); // 160 KB
        let r = format_call_result(&json!({"content": [{"type": "text", "text": big}]}));
        assert!(r.text.len() < MAX_RESULT_BYTES + 200);
        assert!(r.text.contains("[output truncated: showing the first"), "{}", &r.text[r.text.len() - 80..]);
        assert_eq!(cap("short".into()), "short");
    }
}
