//! From the stored conversation to the messages of a request, and the places where the
//! conversation may be cut.
//!
//! The loop stores a valid conversation: every tool call is answered by the next message.
//! [`request_messages`] still repairs what a crash or another writer could have left behind,
//! because a single unanswered tool call makes every later request fail.

use model::{ContentBlock, Message, Role, StoredMessage};

use crate::notes::note;

/// The result of a tool call that was cut off: by a cancel, a crash, or the app closing.
pub(crate) const INTERRUPTED: &str = "Interrupted before the tool finished. It may not have run; check before relying on it.";

/// A tool result saying the call was interrupted.
pub(crate) fn interrupted(tool_use_id: &str) -> ContentBlock {
    ContentBlock::ToolResult { tool_use_id: tool_use_id.to_string(), content: INTERRUPTED.to_string(), is_error: true }
}

/// The summary of the compacted part, as the first message of a request.
pub(crate) fn summary_note(summary: &str) -> String {
    note(&format!(
        "The earlier part of this conversation was replaced by the summary below to save context. \
         Continue from it; do not repeat finished work.\n\n{summary}"
    ))
}

/// The messages to send: the summary (if any) and the stored messages, with adjacent messages
/// of one role merged, empty text dropped, every tool call answered right after it (an
/// unanswered one gets an "interrupted" result) and results without a call left out.
pub(crate) fn request_messages(summary: Option<&str>, stored: &[StoredMessage]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    if let Some(summary) = summary {
        out.push(Message::user(summary_note(summary)));
    }
    // Tool calls of the latest assistant message that no result has answered yet.
    let mut open: Vec<String> = Vec::new();
    for stored in stored {
        let message = &stored.message;
        match message.role {
            Role::Assistant => {
                close_open(&mut out, &mut open);
                let content: Vec<ContentBlock> = message.content.iter().filter(|b| !is_blank(b)).cloned().collect();
                open = message.tool_uses().map(|(id, _, _)| id.to_string()).collect();
                push(&mut out, Role::Assistant, content);
            }
            Role::User => {
                let (mut results, mut rest) = (Vec::new(), Vec::new());
                for block in &message.content {
                    match block {
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            if let Some(at) = open.iter().position(|id| id == tool_use_id) {
                                open.remove(at);
                                results.push(block.clone());
                            }
                        }
                        other if is_blank(other) => {}
                        other => rest.push(other.clone()),
                    }
                }
                results.extend(open.drain(..).map(|id| interrupted(&id)));
                results.extend(rest);
                push(&mut out, Role::User, results);
            }
        }
    }
    close_open(&mut out, &mut open);
    out
}

fn close_open(out: &mut Vec<Message>, open: &mut Vec<String>) {
    let results: Vec<ContentBlock> = open.drain(..).map(|id| interrupted(&id)).collect();
    push(out, Role::User, results);
}

fn push(out: &mut Vec<Message>, role: Role, content: Vec<ContentBlock>) {
    if content.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.role == role => last.content.extend(content),
        _ => out.push(Message { role, content }),
    }
}

fn is_blank(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Text { text } if text.trim().is_empty())
}

/// Ids of the tool calls the last stored message makes, when nothing has answered them.
pub(crate) fn dangling(stored: &[StoredMessage]) -> Vec<String> {
    match stored.last() {
        Some(last) if last.message.role == Role::Assistant => {
            last.message.tool_uses().map(|(id, _, _)| id.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

fn has_tool_result(message: &Message) -> bool {
    message.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// How many of the oldest messages a compaction replaces when it keeps about `keep` recent
/// ones. The kept part never starts with tool results, so a call and its result stay together.
/// 0 means there is nothing to replace.
pub(crate) fn cut_index(stored: &[StoredMessage], keep: usize) -> usize {
    let mut cut = stored.len().saturating_sub(keep);
    while cut > 0 && cut < stored.len() && has_tool_result(&stored[cut].message) {
        cut -= 1;
    }
    cut
}

/// The calls of the tool `name` that succeeded, as their inputs, oldest first.
pub(crate) fn successful_calls<'a>(stored: &'a [StoredMessage], name: &str) -> Vec<&'a serde_json::Value> {
    let mut out = Vec::new();
    for (index, stored_message) in stored.iter().enumerate() {
        let Some(next) = stored.get(index + 1) else { break };
        for (id, tool, input) in stored_message.message.tool_uses() {
            let ok = next.message.content.iter().any(
                |b| matches!(b, ContentBlock::ToolResult { tool_use_id, is_error, .. } if tool_use_id == id && !is_error),
            );
            if tool == name && ok {
                out.push(input);
            }
        }
    }
    out
}

/// A rough token count of messages, for showing how full the context is after a compaction.
pub(crate) fn estimate_tokens(summary: Option<&str>, stored: &[StoredMessage]) -> u64 {
    let mut bytes = summary.map_or(0, str::len);
    for stored in stored {
        bytes += stored.message.content.iter().map(block_bytes).sum::<usize>();
    }
    (bytes / 4) as u64
}

pub(crate) fn block_bytes(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => text.len(),
        // An image costs roughly this many tokens times four, whatever its size on disk.
        ContentBlock::Image { .. } => 6_000,
        ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
        ContentBlock::ToolResult { content, .. } => content.len(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn stored(messages: Vec<Message>) -> Vec<StoredMessage> {
        messages
            .into_iter()
            .enumerate()
            .map(|(i, message)| StoredMessage {
                seq: i as u64 + 1,
                at: 0,
                message,
                compacted: false,
                model: None,
                usage: Default::default(),
                cost_usd: 0.0,
            })
            .collect()
    }

    fn call(id: &str) -> ContentBlock {
        ContentBlock::ToolUse { id: id.into(), name: "read_file".into(), input: json!({"path": "a"}) }
    }

    fn result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult { tool_use_id: id.into(), content: "ok".into(), is_error: false }
    }

    fn assistant(content: Vec<ContentBlock>) -> Message {
        Message { role: Role::Assistant, content }
    }

    fn user(content: Vec<ContentBlock>) -> Message {
        Message { role: Role::User, content }
    }

    #[test]
    fn a_valid_conversation_passes_unchanged_and_roles_merge() {
        let history = stored(vec![
            Message::user("hi"),
            assistant(vec![ContentBlock::text("Reading."), call("t1")]),
            user(vec![result("t1")]),
            Message::user("and hurry"),
            Message::assistant("Done."),
        ]);
        let messages = request_messages(None, &history);
        assert_eq!(
            messages,
            vec![
                Message::user("hi"),
                assistant(vec![ContentBlock::text("Reading."), call("t1")]),
                user(vec![result("t1"), ContentBlock::text("and hurry")]),
                Message::assistant("Done."),
            ]
        );
    }

    #[test]
    fn unanswered_calls_get_an_interrupted_result_right_after_them() {
        // Before a user message, before another assistant message, and at the very end.
        let history = stored(vec![
            Message::user("go"),
            assistant(vec![call("t1"), call("t2")]),
            user(vec![result("t2"), ContentBlock::text("what happened?")]),
            assistant(vec![call("t3")]),
            assistant(vec![call("t4")]),
        ]);
        let messages = request_messages(None, &history);
        assert_eq!(
            messages,
            vec![
                Message::user("go"),
                assistant(vec![call("t1"), call("t2")]),
                user(vec![result("t2"), interrupted("t1"), ContentBlock::text("what happened?")]),
                assistant(vec![call("t3")]),
                user(vec![interrupted("t3")]),
                assistant(vec![call("t4")]),
                user(vec![interrupted("t4")]),
            ]
        );
    }

    #[test]
    fn results_without_a_call_and_blank_text_are_dropped() {
        let history = stored(vec![
            user(vec![result("ghost"), ContentBlock::text("hello")]),
            assistant(vec![ContentBlock::text("  "), ContentBlock::text("Hi.")]),
            user(vec![result("t9")]),
        ]);
        assert_eq!(request_messages(None, &history), vec![Message::user("hello"), Message::assistant("Hi.")]);
    }

    #[test]
    fn the_summary_opens_the_request_and_merges_into_a_user_message() {
        let history = stored(vec![Message::user("next"), Message::assistant("ok")]);
        let messages = request_messages(Some("S"), &history);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, vec![ContentBlock::text(summary_note("S")), ContentBlock::text("next")]);

        let history = stored(vec![assistant(vec![call("t1")]), user(vec![result("t1")])]);
        let messages = request_messages(Some("S"), &history);
        assert_eq!(messages[0], Message::user(summary_note("S")));
        assert_eq!(messages[1].role, Role::Assistant);
    }

    #[test]
    fn cuts_never_split_a_call_from_its_result() {
        let history = stored(vec![
            Message::user("one"),
            assistant(vec![call("t1")]),
            user(vec![result("t1")]),
            assistant(vec![call("t2")]),
            user(vec![result("t2")]),
            Message::assistant("done"),
        ]);
        assert_eq!(cut_index(&history, 6), 0);
        assert_eq!(cut_index(&history, 9), 0);
        assert_eq!(cut_index(&history, 1), 5);
        // Keeping two would start with t2's result: keep its call as well.
        assert_eq!(cut_index(&history, 2), 3);
        assert_eq!(cut_index(&history, 3), 3);
        assert_eq!(cut_index(&history, 4), 1);
        assert_eq!(cut_index(&history, 0), 6);
    }

    #[test]
    fn dangling_calls_are_those_of_a_last_assistant_message() {
        let mut history = stored(vec![Message::user("go"), assistant(vec![call("t1"), call("t2")])]);
        assert_eq!(dangling(&history), ["t1", "t2"]);
        history.extend(stored(vec![user(vec![result("t1"), result("t2")])]));
        assert!(dangling(&history).is_empty());
        assert!(dangling(&[]).is_empty());
    }

    #[test]
    fn successful_calls_skip_failed_and_unanswered_ones() {
        let failed = ContentBlock::ToolResult { tool_use_id: "t2".into(), content: "no".into(), is_error: true };
        let history = stored(vec![
            assistant(vec![call("t1"), call("t2")]),
            user(vec![result("t1"), failed]),
            assistant(vec![call("t3")]),
        ]);
        assert_eq!(successful_calls(&history, "read_file").len(), 1);
        assert!(successful_calls(&history, "bash").is_empty());
    }
}
