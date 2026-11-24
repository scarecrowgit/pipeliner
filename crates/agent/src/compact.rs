//! Compaction: the older part of a conversation replaced by a summary the model writes.
//!
//! The most recent messages stay as they are; the cut never falls between a tool call and its
//! result. Forced skills live in the system prompt and are untouched. The task list and the
//! names of skills loaded so far are written under the summary, so they survive it.

use anyhow::{Result, bail};
use llm::{ChatRequest, Reasoning, pricing};
use model::{AgentEvent, ContentBlock, Message, Role, StoredMessage};

use crate::history::{cut_index, estimate_tokens};
use crate::session_tools::todo;
use crate::turn::{Called, Session, loaded_skills};

/// Output cap of the summary call.
const SUMMARY_MAX_TOKENS: u32 = 8_000;
/// Share of the context window the text to summarise may fill, counted at three bytes a token.
const TRANSCRIPT_SHARE: f64 = 0.6;
const TEXT_MAX_CHARS: usize = 6_000;
const TOOL_INPUT_MAX_CHARS: usize = 600;
const TOOL_RESULT_MAX_CHARS: usize = 1_500;

const SYSTEM: &str = "You compress the conversation of a coding agent so that the agent can go on working with \
                      less context. You write only the summary, in the sections you are asked for.";

const INSTRUCTIONS: &str = "Write a summary of the conversation above that lets the agent continue the work \
without the original messages. Use exactly these sections:

## Goal
What the person asked for, in their words where the wording matters, with the constraints and preferences they stated.

## Decisions
Decisions made and why; instructions and corrections from the person that still apply.

## Done so far
What was done, in order: files read, created or changed (with paths), commands run and their results, what was verified.

## Current state
What is in progress right now, what works, what is broken, open errors with their exact text.

## Next steps
What remains, the most immediate first.

## Facts to keep
Paths, names, identifiers, URLs, commands and numbers that will be needed again.

Be specific and complete rather than short. Leave out pleasantries and anything a later message superseded. \
Do not call tools.";

/// How a compaction ended.
pub(crate) enum Compaction {
    Done,
    /// The conversation is too short to cut.
    Nothing,
    Cancelled,
}

impl Session<'_> {
    /// Summarises the older messages with one model call and marks them compacted. `forced`
    /// (an explicit request, or a request that did not fit) cuts deeper: at most half of the
    /// messages stay.
    pub(crate) fn compact(&mut self, forced: bool) -> Result<Compaction> {
        let limits = &self.spec.limits;
        let keep = if forced { limits.keep_recent.min(self.messages.len() / 2) } else { limits.keep_recent };
        let cut = cut_index(&self.messages, keep);
        if cut == 0 {
            return Ok(Compaction::Nothing);
        }
        self.emit(AgentEvent::CompactionStarted);

        let model = &self.spec.model;
        let budget = (pricing::context_window(model) as f64 * TRANSCRIPT_SHARE) as usize * 3;
        let transcript = transcript(self.summary.as_deref(), &self.messages[..cut], budget);
        let request = ChatRequest {
            system: SYSTEM.to_string(),
            messages: vec![Message::user(format!("{transcript}\n\n{INSTRUCTIONS}"))],
            tools: Vec::new(),
            max_tokens: SUMMARY_MAX_TOKENS.min(pricing::max_output(model)),
            temperature: None,
            reasoning: Reasoning::Low,
        };
        let reply = match self.call_model(&request, false)? {
            Called::Reply(reply) => reply,
            Called::Cancelled(_) => return Ok(Compaction::Cancelled),
            Called::TooLong(message) => bail!("The conversation is too long to summarise: {message}"),
        };
        let mut summary = reply.text().trim().to_string();
        if summary.is_empty() {
            bail!("The model returned an empty summary; the conversation was left as it is.");
        }
        if !self.todos.is_empty() {
            summary.push_str(&format!("\n\n## Task list at this point\n{}", todo::render(&self.todos)));
        }
        let kept_skills = loaded_skills(&self.messages[cut..]);
        let lost_skills: Vec<&str> =
            self.loaded_skills.iter().filter(|name| !kept_skills.contains(name)).map(String::as_str).collect();
        if !lost_skills.is_empty() {
            summary.push_str(&format!(
                "\n\n## Skills loaded before\n{}. Their instructions are no longer in the conversation: load a \
                 skill again before relying on it.",
                lost_skills.join(", ")
            ));
        }

        let up_to_seq = self.messages[cut - 1].seq;
        self.deps.host.compact(&self.env.session, up_to_seq, &summary)?;
        self.messages.drain(..cut);
        self.summary = Some(summary);
        self.loaded_skills = kept_skills;
        self.context_tokens = estimate_tokens(self.summary.as_deref(), &self.messages);
        self.emit(AgentEvent::Compacted {
            summarised: u32::try_from(cut).unwrap_or(u32::MAX),
            context_tokens: self.context_tokens,
        });
        Ok(Compaction::Done)
    }
}

/// The messages to summarise as plain text, with long tool output cut. When even that is
/// more than `budget` bytes, the oldest messages are left out and a line says how many.
fn transcript(previous: Option<&str>, messages: &[StoredMessage], budget: usize) -> String {
    let mut entries: Vec<String> = messages.iter().map(|m| entry(&m.message)).filter(|e| !e.is_empty()).collect();
    let mut total: usize = entries.iter().map(String::len).sum();
    let mut dropped = 0;
    while total > budget && entries.len() > 1 {
        total -= entries.remove(0).len();
        dropped += 1;
    }
    let mut out = String::from("<conversation>\n");
    if let Some(previous) = previous {
        out.push_str(&format!("### Summary of what came before\n{previous}\n\n"));
    }
    if dropped > 0 {
        out.push_str(&format!("[{} left out here to fit.]\n\n", base::fmt::plural(dropped, "message", "messages")));
    }
    out.push_str(&entries.join("\n\n"));
    out.push_str("\n</conversation>");
    out
}

fn entry(message: &Message) -> String {
    let who = match message.role {
        Role::User => "### User",
        Role::Assistant => "### Assistant",
    };
    let clip = base::fmt::truncate_chars;
    let mut lines: Vec<String> = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text } if text.trim().is_empty() => {}
            ContentBlock::Text { text } => lines.push(clip(text.trim(), TEXT_MAX_CHARS)),
            ContentBlock::Thinking { .. } => {}
            ContentBlock::Image { name, .. } => lines.push(format!("[image: {name}]")),
            ContentBlock::ToolUse { name, input, .. } => {
                lines.push(format!("[tool call {name}: {}]", clip(&input.to_string(), TOOL_INPUT_MAX_CHARS)))
            }
            ContentBlock::ToolResult { content, is_error, .. } => {
                let kind = if *is_error { "tool error" } else { "tool result" };
                lines.push(format!("[{kind}: {}]", clip(content.trim(), TOOL_RESULT_MAX_CHARS)))
            }
        }
    }
    if lines.is_empty() { String::new() } else { format!("{who}\n{}", lines.join("\n")) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn stored(message: Message) -> StoredMessage {
        StoredMessage { seq: 1, at: 0, message, compacted: false, model: None, usage: Default::default(), cost_usd: 0.0 }
    }

    #[test]
    fn the_transcript_names_roles_and_cuts_long_tool_output() {
        let messages = vec![
            stored(Message::user("Fix the retry bug")),
            stored(Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking { text: "secret".into(), signature: None },
                    ContentBlock::text("Reading."),
                    ContentBlock::ToolUse { id: "t1".into(), name: "read_file".into(), input: json!({"path": "a.rs"}) },
                ],
            }),
            stored(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult { tool_use_id: "t1".into(), content: "x".repeat(5_000), is_error: false }],
            }),
        ];
        let text = transcript(Some("Earlier: nothing."), &messages, 1_000_000);
        assert!(text.starts_with("<conversation>\n### Summary of what came before\nEarlier: nothing.\n\n### User\nFix the retry bug"));
        assert!(text.contains("### Assistant\nReading.\n[tool call read_file: {\"path\":\"a.rs\"}]"));
        assert!(!text.contains("secret"));
        assert!(text.contains("[tool result: xxx"));
        assert!(text.len() < 2_500, "{}", text.len());
        assert!(text.ends_with("\n</conversation>"));
    }

    #[test]
    fn the_oldest_messages_go_when_the_budget_is_small() {
        let messages: Vec<StoredMessage> = (0..10).map(|i| stored(Message::user(format!("message {i} {}", "y".repeat(100))))).collect();
        let text = transcript(None, &messages, 400);
        assert!(text.contains("[7 messages left out here to fit.]"), "{text}");
        assert!(!text.contains("message 6 "));
        assert!(text.contains("message 7 ") && text.contains("message 9 "));
    }
}
