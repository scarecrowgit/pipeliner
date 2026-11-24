//! Transcript exports: Markdown for reading, JSON Lines for tools. Secrets are redacted in
//! both.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use model::{ContentBlock, Role, Session, SessionId, StoredMessage};
use serde_json::json;

use super::TO_END;
use crate::api::{ExportFormat, ExportOptions};
use crate::engine::Inner;

/// Prefixes of tokens that are secrets whatever surrounds them.
const SECRET_PREFIXES: [&str; 10] =
    ["sk-ant-", "sk-or-", "sk-", "ghp_", "gho_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "AIza"];
/// Characters of a token after its prefix for it to count as a secret.
const SECRET_MIN: usize = 16;
const REDACTED: &str = "[redacted]";

/// Writes the transcript of `id` to `to` (a file, or a folder to put it in) and returns the
/// file's path.
pub(crate) fn export(inner: &Inner, id: &SessionId, format: ExportFormat, options: ExportOptions, to: &Path) -> Result<PathBuf> {
    let session = inner.session(id)?;
    let ext = match format {
        ExportFormat::Markdown => "md",
        ExportFormat::Jsonl => "jsonl",
    };
    let path = if to.is_dir() || to.extension().is_none() {
        to.join(format!("{}-{}.{ext}", base::id::slug(&session.title, 40), id))
    } else {
        to.to_path_buf()
    };
    let mut sessions = vec![session];
    if options.subagents {
        let mut at = 0;
        while at < sessions.len() {
            let mut children = inner.store.child_sessions(&sessions[at].id)?;
            children.sort_by_key(|c| c.created);
            sessions.extend(children);
            at += 1;
        }
    }
    let mut text = String::new();
    for (n, session) in sessions.iter().enumerate() {
        let messages = inner.store.messages(&session.id, 1..TO_END)?;
        let name = inner.catalog().get(&session.runnable).map_or_else(|| session.title.clone(), |r| r.name.clone());
        match format {
            ExportFormat::Markdown => markdown(&mut text, inner, session, &name, &messages, options, n > 0),
            ExportFormat::Jsonl => jsonl(&mut text, inner, session, &messages, options)?,
        }
    }
    let text = redact(&text);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Absolute paths of the workdir shown relative to it, unless local paths are kept.
fn localise(text: &str, session: &Session, options: ExportOptions) -> String {
    match (&session.workdir, options.local_paths) {
        (Some(workdir), false) => {
            let root = workdir.path.to_string_lossy();
            text.replace(&format!("{root}/"), "").replace(root.as_ref(), ".")
        }
        _ => text.to_string(),
    }
}

fn markdown(
    out: &mut String,
    inner: &Inner,
    session: &Session,
    name: &str,
    messages: &[StoredMessage],
    options: ExportOptions,
    sub: bool,
) {
    let offset = base::time::utc_offset(session.created);
    let heading = if sub { "##" } else { "#" };
    let _ = writeln!(out, "{heading} {}{}\n", if sub { "Sub-agent: " } else { "" }, session.title);
    let _ = writeln!(out, "- Agent: {name} · {}", session.model);
    if let Some(workdir) = &session.workdir {
        let folder = if options.local_paths { inner.paths.display(&workdir.path) } else { workdir.repo.clone() };
        let branch = workdir.branch.as_deref().map(|b| format!(" · {b}")).unwrap_or_default();
        let _ = writeln!(out, "- Folder: {folder}{branch}");
    }
    let _ = writeln!(out, "- Started: {}\n", base::time::long_date(session.created, offset));
    let level = if sub { "###" } else { "##" };
    for stored in messages {
        let results_only = stored.message.content.iter().all(|b| matches!(b, ContentBlock::ToolResult { .. }));
        let who = match stored.message.role {
            Role::User if results_only => "Tool results",
            Role::User => "You",
            Role::Assistant => name,
        };
        let mut body = String::new();
        for block in &stored.message.content {
            match block {
                ContentBlock::Text { text } if agent::notes::is_note(text) => {}
                ContentBlock::Text { text } => {
                    let _ = writeln!(body, "{}\n", text.trim_end());
                }
                ContentBlock::Thinking { .. } => {}
                ContentBlock::Image { name, blob, .. } => {
                    if options.images {
                        let _ = writeln!(body, "![{name}]({})\n", inner.store.blob_path(blob).unwrap_or_default().display());
                    } else {
                        let _ = writeln!(body, "*[image: {name}]*\n");
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let _ = writeln!(body, "> **{name}** `{}`\n", base::fmt::truncate_chars(&input.to_string(), 200));
                }
                ContentBlock::ToolResult { content, is_error, .. } => {
                    if options.tool_output {
                        let mark = if *is_error { " (error)" } else { "" };
                        let _ = writeln!(body, "<details><summary>Result{mark}</summary>\n\n```\n{}\n```\n</details>\n", content.trim_end());
                    }
                }
            }
        }
        if body.trim().is_empty() {
            continue;
        }
        let time = base::time::clock_time(stored.at, offset);
        let _ = writeln!(out, "{level} {who} · {time}\n");
        out.push_str(&localise(&body, session, options));
    }
}

fn jsonl(out: &mut String, inner: &Inner, session: &Session, messages: &[StoredMessage], options: ExportOptions) -> Result<()> {
    for stored in messages {
        let content: Vec<serde_json::Value> = stored
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Thinking { .. } => None,
                ContentBlock::Image { name, blob, media_type, bytes } => Some(if options.images {
                    let path = inner.store.blob_path(blob).unwrap_or_default();
                    json!({"type": "image", "name": name, "media_type": media_type, "bytes": bytes, "path": path})
                } else {
                    json!({"type": "image", "name": name})
                }),
                ContentBlock::ToolResult { tool_use_id, content, is_error } => Some(if options.tool_output {
                    json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error})
                } else {
                    json!({"type": "tool_result", "tool_use_id": tool_use_id, "is_error": is_error})
                }),
                other => serde_json::to_value(other).ok(),
            })
            .collect();
        let line = json!({
            "session": session.id,
            "parent": session.parent,
            "seq": stored.seq,
            "at": stored.at,
            "role": stored.message.role,
            "text": stored.message.plain_text(),
            "content": content,
            "compacted": stored.compacted,
            "model": stored.model,
            "usage": stored.usage,
            "cost_usd": stored.cost_usd,
        });
        out.push_str(&localise(&serde_json::to_string(&line)?, session, options));
        out.push('\n');
    }
    Ok(())
}

/// `text` with every token that looks like an API key or access token replaced.
pub(crate) fn redact(text: &str) -> String {
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let start = rest.find(|c: char| is_token_char(c)).unwrap_or(rest.len());
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_token_char(c)).unwrap_or(rest.len());
        let token = &rest[..end];
        let secret = SECRET_PREFIXES
            .iter()
            .any(|p| token.strip_prefix(p).is_some_and(|tail| tail.len() >= SECRET_MIN))
            || (token.starts_with("AKIA") && token.len() == 20 && token[4..].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
        out.push_str(if secret { REDACTED } else { token });
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn secrets_are_redacted_and_the_rest_kept() {
        let text = "key sk-ant-api03-abcdefghijklmnopqrstuv and ghp_0123456789abcdefABCDEF, not sk-short or task-runner.";
        assert_eq!(redact(text), "key [redacted] and [redacted], not sk-short or task-runner.");
        assert_eq!(redact("AKIAABCDEFGHIJKLMNOP x"), "[redacted] x");
        assert_eq!(redact("Привет, мир"), "Привет, мир");
    }
}
