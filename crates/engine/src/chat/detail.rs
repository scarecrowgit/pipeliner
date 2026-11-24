//! What the chat screen shows around the transcript.

use std::sync::Arc;

use anyhow::Result;
use base::cancel::CancelToken;
use model::{ContentBlock, Role, SessionId};

use crate::api::{SessionDetail, SkillInContext};
use crate::engine::Inner;

pub(crate) fn session_detail(inner: &Arc<Inner>, id: &SessionId) -> Result<Option<SessionDetail>> {
    let Some(session) = inner.store.session(id)? else { return Ok(None) };
    let folder = session.workdir.as_ref().map(|w| w.path.clone());
    let catalog = inner.catalog_for(folder.as_deref());
    let mut children = inner.store.child_sessions(id)?;
    children.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));

    let mut forced: Vec<String> = catalog
        .agent_def(&session.runnable)
        .map(|d| d.skills.forced.clone())
        .unwrap_or_default();
    for skill in &session.forced_skills {
        if !forced.contains(skill) {
            forced.push(skill.clone());
        }
    }
    let loaded = loaded_skills(inner, id)?;
    let mut skills: Vec<SkillInContext> = Vec::new();
    for name in forced.iter().chain(loaded.iter()) {
        if skills.iter().any(|s| &s.name == name) {
            continue;
        }
        let tokens = catalog.skill(name).map_or(0, |s| s.tokens);
        let is_forced = forced.contains(name);
        skills.push(SkillInContext { name: name.clone(), forced: is_forced, loaded: is_forced || loaded.contains(name), tokens });
    }

    let mcp = crate::chat::turn::mcp_peek(inner, folder.as_deref()).servers();
    let repo = folder.as_deref().and_then(|f| vcs::facts(f, &CancelToken::new()));
    Ok(Some(SessionDetail {
        todos: inner.store.todos(id)?,
        files: inner.store.file_changes(id)?,
        children,
        skills,
        mcp,
        repo,
        pending: inner.asks.of(id),
        queued: inner.live.queued(id),
        session,
    }))
}

/// Skills the model loaded with the `skill` tool in the part of the conversation it still sees.
fn loaded_skills(inner: &Inner, id: &SessionId) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for stored in inner.store.model_messages(id)? {
        if stored.message.role != Role::Assistant {
            continue;
        }
        for block in &stored.message.content {
            if let ContentBlock::ToolUse { name, input, .. } = block
                && name == agent::tool_names::SKILL
                && let Some(skill) = input.get("name").and_then(|v| v.as_str())
                && !out.iter().any(|s| s == skill)
            {
                out.push(skill.to_string());
            }
        }
    }
    Ok(out)
}
