//! Shared pieces of the protocol tests: a blob store and one conversation that uses tools,
//! images and thinking.

use std::collections::HashMap;

use model::{BlobId, ContentBlock, Message, Role, ToolSpec};
use serde_json::json;

use crate::{BlobSource, ChatRequest, LlmError, StreamEvent, sse};

/// Blobs held in memory.
pub(crate) struct MapBlobs(pub HashMap<BlobId, Vec<u8>>);

impl BlobSource for MapBlobs {
    fn read(&self, id: &BlobId) -> std::io::Result<Vec<u8>> {
        self.0.get(id).cloned().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "gone"))
    }
}

/// One stored image, `png-1`, holding the bytes `PNG!` (base64 `UE5HIQ==`).
pub(crate) fn blobs() -> MapBlobs {
    MapBlobs(HashMap::from([(BlobId::new("png-1"), b"PNG!".to_vec())]))
}

/// A user turn with an image (stored) and one whose blob is gone, a signed and an unsigned
/// thinking block, two parallel tool calls and their results, and an empty assistant turn.
pub(crate) fn conversation() -> ChatRequest {
    let image = |blob: &str, name: &str| ContentBlock::Image {
        blob: BlobId::new(blob),
        media_type: "image/png".into(),
        name: name.into(),
        bytes: 4,
    };
    ChatRequest {
        system: "Be brief.".into(),
        messages: vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::text("What is in these?"), image("png-1", "a.png"), image("lost", "b.png")],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking { text: "Look closer.".into(), signature: Some("sig-a".into()) },
                    ContentBlock::Thinking { text: "From elsewhere.".into(), signature: None },
                    ContentBlock::text("Checking."),
                    ContentBlock::ToolUse { id: "call_a".into(), name: "zoom".into(), input: json!({"x": 1}) },
                    ContentBlock::ToolUse { id: "call_b".into(), name: "now".into(), input: json!({}) },
                ],
            },
            Message {
                role: Role::User,
                content: vec![
                    ContentBlock::text("hurry"),
                    ContentBlock::ToolResult { tool_use_id: "call_a".into(), content: "a cat".into(), is_error: false },
                    ContentBlock::ToolResult { tool_use_id: "call_b".into(), content: "clock broken".into(), is_error: true },
                ],
            },
            Message { role: Role::Assistant, content: vec![ContentBlock::text("")] },
        ],
        tools: vec![
            ToolSpec { name: "zoom".into(), description: "Zoom in".into(), input_schema: json!({"type": "object"}) },
            ToolSpec { name: "now".into(), description: "Time".into(), input_schema: json!({"type": "object"}) },
        ],
        max_tokens: 1024,
        temperature: Some(0.5),
        reasoning: Default::default(),
    }
}

/// Decodes a transcript given as SSE lines joined with `\n`.
pub(crate) fn decode(decoder: &mut dyn sse::Decoder, lines: &[&str]) -> Result<Vec<StreamEvent>, LlmError> {
    sse::decode_all(lines.join("\n").as_bytes(), decoder)
}
