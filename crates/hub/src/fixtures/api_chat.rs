//! The fixture's chat and workdir commands and queries.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::api::{
    AgentEvent, Attachment, BlobId, ChatApi, ChatMode, Event, ExportFormat, ExportOptions, FolderInfo, Message,
    McpState, ModelRef, NewSession, PermissionDecision, ProviderInfo, ProviderModels, RepoFacts, RequestId, Role,
    RunnableRef, SearchResult, Session, SessionDetail, SessionFilter, SessionId, SessionOrigin, SessionQuery,
    SessionState, SkillInContext, SlashCommand, StoredMessage, UserMessage, WorkMode, Workdir, WorkdirApi, WorkdirChoice, WorkdirEntry,
};

use super::data_sessions::{FIX_1423, chat, repo_facts};
use super::{FixtureApi, script, search};

impl FixtureApi {
    /// Emits the session's header after a change.
    fn changed(&self, id: &SessionId, f: impl FnOnce(&mut Session)) -> Result<()> {
        let session = {
            let mut world = self.world();
            let now = world.now;
            let session = world.session_mut(id)?;
            f(session);
            session.updated = now;
            session.clone()
        };
        self.emit(Event::SessionChanged { session });
        Ok(())
    }
}

impl ChatApi for FixtureApi {
    fn sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>> {
        Ok(search::sessions(&self.world(), filter))
    }

    fn session_detail(&self, id: &SessionId) -> Result<Option<SessionDetail>> {
        let world = self.world();
        let Some(session) = world.session(id).cloned() else { return Ok(None) };
        let extras = world.extras.get(id).cloned().unwrap_or_default();
        let mut children: Vec<Session> = world.sessions.iter().filter(|s| s.parent.as_ref() == Some(id)).cloned().collect();
        children.sort_by_key(|child| child.created);
        Ok(Some(SessionDetail {
            session,
            todos: extras.todos,
            files: extras.files,
            children,
            skills: extras.skills,
            mcp: extras.mcp,
            repo: extras.repo,
            pending: world.permissions.iter().filter(|request| &request.session == id).cloned().collect(),
            queued: extras.queued,
        }))
    }

    fn messages(&self, id: &SessionId, from: u64, to: u64) -> Result<Vec<StoredMessage>> {
        let world = self.world();
        Ok(world.messages.get(id).map_or_else(Vec::new, |messages| {
            messages.iter().filter(|message| (from..to).contains(&message.seq)).cloned().collect()
        }))
    }

    fn search_sessions(&self, query: &SessionQuery) -> Result<SearchResult> {
        self.fail_if_told("search_sessions")?;
        Ok(search::search(&self.world(), query))
    }

    fn new_session(&self, new: NewSession) -> Result<Session> {
        self.record("new_session", format!("{:?}", new.runnable))?;
        let session = {
            let mut world = self.world();
            let now = world.now;
            let id = world.fresh_id("chat");
            let runnable = new.runnable.clone().unwrap_or_else(|| RunnableRef::agent("build"));
            let mut session = chat(&id, "New chat", runnable, SessionState::Idle, now, now);
            session.mode = new.mode;
            session.message_count = 0;
            session.issue = new.issue.clone();
            session.forced_skills = new.forced_skills.clone();
            if let Some(model) = new.model {
                session.model = model;
            }
            session.workdir = new.workdir.map(|choice| workdir_of(&choice));
            world.sessions.push(session.clone());
            session
        };
        self.emit(Event::SessionChanged { session: session.clone() });
        Ok(session)
    }

    fn send(&self, id: &SessionId, message: UserMessage) -> Result<()> {
        self.record("send", format!("{id} {}", message.text))?;
        let events = {
            let mut world = self.world();
            if world.session_mut(id)?.state == SessionState::Running {
                // A busy session takes the message at its next turn.
                world.extras.entry(id.clone()).or_default().queued.push(message);
                return Ok(());
            }
            let mut content = Vec::new();
            if !message.text.trim().is_empty() {
                content.push(crate::api::ContentBlock::text(message.text));
            }
            content.extend(message.images.into_iter().map(|image| crate::api::ContentBlock::Image {
                blob: image.blob,
                media_type: image.media_type,
                name: image.name,
                bytes: image.bytes,
            }));
            script::reply_to(&mut world, id, &Message { role: Role::User, content })
        };
        self.play_events(events);
        Ok(())
    }

    fn interrupt(&self, id: &SessionId) {
        if self.record("interrupt", id.to_string()).is_err() {
            return;
        }
        let events = {
            let mut world = self.world();
            if world.session(id).is_none() {
                return;
            }
            script::interrupted(&mut world, id)
        };
        self.emit_all(events);
    }

    fn retry(&self, id: &SessionId) -> Result<()> {
        self.record("retry", id.to_string())?;
        self.changed(id, |session| session.state = SessionState::Running)
    }

    fn compact(&self, id: &SessionId) -> Result<()> {
        self.record("compact", id.to_string())?;
        let agent = |event| Event::Agent { session: id.clone(), event };
        self.emit_all([
            agent(AgentEvent::CompactionStarted),
            agent(AgentEvent::Compacted { summarised: 6, context_tokens: 12_400 }),
        ]);
        self.changed(id, |session| session.context_tokens = 12_400)
    }

    fn answer_permission(&self, request: &RequestId, decision: PermissionDecision) -> Result<()> {
        self.record("answer_permission", format!("{request} {decision:?}"))?;
        let events = {
            let mut world = self.world();
            let Some(pending) = world.permissions.iter().find(|p| &p.id == request).cloned() else {
                bail!("the request was already answered");
            };
            if pending.session.as_str() == FIX_1423 && !matches!(decision, PermissionDecision::Reject { .. }) {
                script::after_permission(&mut world, &pending.session)
            } else {
                world.permissions.retain(|p| &p.id != request);
                vec![Event::PermissionResolved { id: request.clone() }]
            }
        };
        self.play_events(events);
        Ok(())
    }

    fn set_session_runnable(&self, id: &SessionId, runnable: &RunnableRef, mode: ChatMode) -> Result<()> {
        self.record("set_session_runnable", format!("{id} {runnable} {mode:?}"))?;
        self.changed(id, |session| {
            session.runnable = runnable.clone();
            session.mode = mode;
        })
    }

    fn set_session_model(&self, id: &SessionId, model: &ModelRef) -> Result<()> {
        self.record("set_session_model", format!("{id} {model}"))?;
        self.changed(id, |session| session.model = model.clone())
    }

    fn set_session_workdir(&self, id: &SessionId, workdir: WorkdirChoice) -> Result<Session> {
        self.record("set_session_workdir", format!("{id} {}", workdir.path.display()))?;
        self.changed(id, |session| session.workdir = Some(workdir_of(&workdir)))?;
        self.world().session(id).cloned().ok_or_else(|| anyhow::anyhow!("no chat {id}"))
    }

    fn set_forced_skills(&self, id: &SessionId, skills: Vec<String>) -> Result<()> {
        self.record("set_forced_skills", format!("{id} {skills:?}"))?;
        {
            let mut world = self.world();
            let extras = world.extras.entry(id.clone()).or_default();
            for skill in &mut extras.skills {
                skill.forced = skills.contains(&skill.name);
            }
            for name in &skills {
                if !extras.skills.iter().any(|skill| &skill.name == name) {
                    extras.skills.push(SkillInContext { name: name.clone(), forced: true, loaded: true, tokens: 0 });
                }
            }
        }
        self.changed(id, |session| session.forced_skills = skills)
    }

    fn rename_session(&self, id: &SessionId, title: &str) -> Result<()> {
        self.record("rename_session", format!("{id} {title}"))?;
        self.changed(id, |session| session.title = title.to_string())
    }

    fn pin_session(&self, id: &SessionId, pinned: bool) -> Result<()> {
        self.record("pin_session", format!("{id} {pinned}"))?;
        self.changed(id, |session| session.pinned = pinned)
    }

    fn archive_session(&self, id: &SessionId, archived: bool) -> Result<()> {
        self.record("archive_session", format!("{id} {archived}"))?;
        self.changed(id, |session| session.archived = archived)
    }

    fn delete_session(&self, id: &SessionId) -> Result<()> {
        self.record("delete_session", id.to_string())?;
        {
            let mut world = self.world();
            world.session_mut(id)?;
            world.sessions.retain(|session| &session.id != id && session.parent.as_ref() != Some(id));
            world.messages.remove(id);
        }
        self.emit(Event::SessionRemoved { session: id.clone() });
        Ok(())
    }

    fn fork_session(&self, id: &SessionId, up_to_seq: Option<u64>) -> Result<Session> {
        self.record("fork_session", format!("{id} {up_to_seq:?}"))?;
        let session = {
            let mut world = self.world();
            let mut session = world.session_mut(id)?.clone();
            let fork = SessionId::new(world.fresh_id("chat"));
            let messages: Vec<StoredMessage> = world
                .messages
                .get(id)
                .map(|messages| messages.iter().filter(|m| up_to_seq.is_none_or(|last| m.seq <= last)).cloned().collect())
                .unwrap_or_default();
            session.id = fork.clone();
            session.title = format!("{} (fork)", session.title);
            session.pinned = false;
            session.state = SessionState::Idle;
            session.origin = SessionOrigin::Me;
            session.message_count = messages.len() as u32;
            world.messages.insert(fork, messages);
            world.sessions.push(session.clone());
            session
        };
        self.emit(Event::SessionChanged { session: session.clone() });
        Ok(session)
    }

    fn export_session(&self, id: &SessionId, format: ExportFormat, _options: ExportOptions, to: &Path) -> Result<PathBuf> {
        self.record("export_session", format!("{id} {format:?} {}", to.display()))?;
        let extension = match format {
            ExportFormat::Markdown => "md",
            ExportFormat::Jsonl => "jsonl",
        };
        Ok(to.join(format!("{id}.{extension}")))
    }

    fn put_image(&self, name: &str, media_type: &str, bytes: &[u8]) -> Result<Attachment> {
        self.record("put_image", name.to_string())?;
        let blob = BlobId::new(self.world().fresh_id("blob"));
        Ok(Attachment { blob, name: name.to_string(), media_type: media_type.to_string(), bytes: bytes.len() as u64 })
    }

    fn blob_path(&self, id: &BlobId) -> PathBuf {
        self.blob_dir().join(id.as_str())
    }

    fn models(&self) -> Vec<ProviderModels> {
        let provider = |id: &str, kind: &str, url: &str, models: &[&str]| ProviderModels {
            provider: ProviderInfo {
                id: id.to_string(),
                kind: kind.to_string(),
                base_url: url.to_string(),
                models: models.iter().map(|m| m.to_string()).collect(),
                has_key: true,
            },
            models: models.iter().map(|m| m.to_string()).collect(),
            error: None,
        };
        vec![
            provider("anthropic", "anthropic", "https://api.anthropic.com", &["opus-5.5", "sonnet-5.5", "haiku-5.5"]),
            provider("openai", "openai", "https://api.openai.com/v1", &["gpt-5.2"]),
        ]
    }

    fn mention_files(&self, _id: &SessionId, prefix: &str, limit: usize) -> Vec<String> {
        const FILES: [&str; 8] = [
            "src/charges/retry.ts",
            "src/charges/retry.test.ts",
            "src/charges/charge.ts",
            "src/charges/keys.ts",
            "src/refunds/webhook.ts",
            "src/refunds/batcher.ts",
            "AGENTS.md",
            "package.json",
        ];
        FILES.iter().filter(|file| file.contains(prefix)).take(limit).map(|file| file.to_string()).collect()
    }

    fn slash_commands(&self, _id: &SessionId) -> Vec<SlashCommand> {
        let command = |name: &str, description: &str, from_project| SlashCommand {
            name: name.to_string(),
            description: description.to_string(),
            from_project,
        };
        vec![
            command("compact", "Summarise older turns now", false),
            command("clear", "Start over in a new chat", false),
            command("release-check", "Run the release checklist", true),
            command("bump-sdk", "Bump the gateway SDK", true),
        ]
    }

    fn reconnect_mcp(&self, id: &SessionId, server: &str) -> Result<()> {
        self.record("reconnect_mcp", format!("{id} {server}"))?;
        let mut world = self.world();
        if let Some(info) = world.extras.get_mut(id).and_then(|extras| extras.mcp.iter_mut().find(|m| m.name == server)) {
            info.state = McpState::Ready;
            info.error = None;
            info.tools = 18;
        }
        Ok(())
    }
}

fn workdir_of(choice: &WorkdirChoice) -> Workdir {
    let repo = choice.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Workdir {
        path: choice.path.clone(),
        repo,
        branch: choice.branch.clone().or_else(|| choice.base.clone()),
        host: crate::api::RepoHost::GitHub,
        mode: choice.mode,
        source: (choice.mode == WorkMode::Worktree).then(|| choice.path.clone()),
        runtime: "mac-mini".to_string(),
    }
}

impl WorkdirApi for FixtureApi {
    fn recent_workdirs(&self) -> Vec<WorkdirEntry> {
        self.world().workdirs.clone()
    }

    fn worktrees(&self, repo: &Path) -> Vec<WorkdirEntry> {
        let name = repo.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        self.world().worktrees.iter().filter(|entry| entry.repo == name).cloned().collect()
    }

    fn repo_facts(&self, path: &Path) -> Option<RepoFacts> {
        path.to_string_lossy().contains("payments-api").then(|| repo_facts("code/acme/payments-api", "main", 0, 0, false))
    }

    fn branches(&self, _repo: &Path) -> Vec<String> {
        ["main", "fix/PAY-1423", "pay-1436-retry-budget", "chat/refund-timeout"].iter().map(|b| b.to_string()).collect()
    }

    fn folder_info(&self, path: &Path) -> FolderInfo {
        FolderInfo {
            agents_md_lines: Some(32),
            skills: vec!["payments-retries".to_string(), "write-failing-test".to_string()],
            commands: vec!["release-check".to_string(), "bump-sdk".to_string()],
            facts: self.repo_facts(path),
        }
    }

    fn suggest_branch(&self, _repo: &Path, hint: &str) -> String {
        format!("chat/{}", base::id::slug(hint, 40))
    }

    fn clone_repo(&self, url: &str, dest: &Path) -> Result<()> {
        self.record("clone_repo", format!("{url} {}", dest.display()))?;
        self.emit(Event::Notice { level: crate::api::NoticeLevel::Success, text: format!("Cloned {url}") });
        Ok(())
    }
}
