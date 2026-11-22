//! The agent loop: send → stream → tools (auto / ask) → continue, plus interrupts,
//! retries, `!shell` commands, `/` commands, config + model loading and persistence.
//!
//! Blocking backend calls run in `cx.background_spawn`; stream events come back over a
//! `smol::channel` to a foreground task that updates the entity. The current foreground
//! task lives in [`Run::task`] — dropping it (interrupt) stops event delivery, and the
//! shared cancel flag stops the stream / running tool on the background thread.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_tools::skills::{self, SKILL_TOOL};
use agent_tools::{Permission, ToolDisplay, ToolOutput};
use gpui::{AppContext as _, Context, Task, Window, px};
use llm::{ChatRequest, StopReason, StreamEvent, Usage};
use serde_json::{Value, json};

use crate::data::Mode;
use crate::deferred;
use crate::extensions::{self, McpTarget};
use crate::models::{self, ModelRef, ProviderModels};
use crate::picker::skill_name;
use crate::session::{
    self, Attachment, Item, Saved, ShellRun, ToolResult, ToolStatus, Turn, TurnState, UserMsg,
};
use crate::todo::{self, TODO_TOOL};
use crate::workdir::WorkMode;
use crate::{ChatScreen, ConfigState};

/// Model calls per send before the loop stops.
pub(crate) const MAX_STEPS: usize = 50;

const SYSTEM_BASE: &str = "You are Build, a coding agent inside the Harness app. You work in the \
user's repository through tools; all paths are relative to the workspace root.

- Read the relevant code before changing it. Prefer small, exact edit_file replacements over \
rewriting whole files, and match the surrounding style.
- Use bash for builds, tests and git inspection; the user approves edits and commands, and may \
reject them with a note — adapt instead of retrying the same call.
- For multi-step work keep a todo list with todo_write (one item in_progress at a time).
- Be concise. Use Markdown; refer to code as `path:line`. When done, summarise what changed and \
how it was verified.";

const SYSTEM_MCP: &str = "\n- Tools named `mcp__<server>__<tool>` come from external MCP servers the \
user configured; the user approves them unless they are auto-approved.";

const SYSTEM_READ_ONLY: &str = "\n\nThis chat works in its folder read-only: only read-only \
tools are available. Answer questions and propose changes, but do not try to edit files or run \
commands.";

const SYSTEM_PLAN: &str = "\n\nYou are in Plan mode: only read-only tools are available. \
Investigate the code and reply with a concise, numbered implementation plan (files, changes, \
risks, how to verify). Do not attempt to edit files or run commands.";

pub(crate) struct Run {
    pub cancel: Arc<AtomicBool>,
    pub steps: usize,
    /// Tool names offered in the current request.
    pub offered: Vec<String>,
    /// MCP tools offered in the current request: id → (server, tool).
    pub mcp_tools: BTreeMap<String, McpTarget>,
    /// Model of the current request (the picker may change mid-run).
    pub model: Option<ModelRef>,
    task: Option<Task<()>>,
}

impl Run {
    fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            steps: 0,
            offered: Vec::new(),
            mcp_tools: BTreeMap::new(),
            model: None,
            task: None,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Once,
    Always,
    Reject,
}

fn tool_result(content: impl Into<String>, is_error: bool) -> ToolStatus {
    ToolStatus::Done(ToolResult {
        content: content.into(),
        is_error,
        display: None,
    })
}

impl ChatScreen {
    fn set_task(&mut self, task: Task<()>) {
        if let Some(run) = self.run.as_mut() {
            run.task = Some(task);
        }
    }

    fn last_turn_mut(&mut self) -> Option<&mut Turn> {
        match self.items.last_mut() {
            Some(Item::Turn(t)) => Some(t),
            _ => None,
        }
    }

    pub(crate) fn asking_tool_id(&self) -> Option<String> {
        match self.items.last() {
            Some(Item::Turn(t)) => t
                .tools()
                .find(|c| c.status == ToolStatus::Asking)
                .map(|c| c.id.clone()),
            _ => None,
        }
    }

    pub(crate) fn waiting_permission(&self) -> bool {
        self.asking_tool_id().is_some()
    }

    /// Scrolled to (near) the end — keep following streamed output.
    fn at_bottom(&self) -> bool {
        let max = self.scroll.max_offset().height;
        -self.scroll.offset().y >= max - px(48.)
    }

    fn notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.items.push(Item::Notice(text.into()));
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    // ── Sending ─────────────────────────────────────────────────────────

    pub(crate) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.draft.trim().to_string();
        if text.is_empty() || self.busy() {
            return;
        }
        if let Some(cmd) = text.strip_prefix('!') {
            let cmd = cmd.trim().to_string();
            if !cmd.is_empty() {
                self.set_draft("", cx);
                self.run_shell(cmd, cx);
            }
            return;
        }
        if let Some(cmd) = text.strip_prefix('/') {
            self.set_draft("", cx);
            self.run_command(cmd.trim(), window, cx);
            return;
        }
        if self.model.is_none() {
            let msg = if self.has_providers() {
                "Pick a model first (the model menu below the prompt)."
            } else {
                "No LLM provider is configured yet. See the setup steps above."
            };
            self.notice(msg, cx);
            return;
        }
        let attachments = session::mentions(&text)
            .into_iter()
            .filter(|p| {
                self.files.binary_search(p).is_ok() || self.workspace.root.join(p).is_file()
            })
            .map(|path| Attachment {
                path,
                content: None,
                lines: 0,
                error: None,
            })
            .collect();
        self.items.push(Item::User(UserMsg { text, attachments }));
        self.set_draft("", cx);
        self.scroll.scroll_to_bottom();
        self.begin_run(cx);
    }

    /// Start a run for the last user item: read @-mentioned files, then the first turn.
    fn begin_run(&mut self, cx: &mut Context<Self>) {
        let run = Run::new();
        let cancel = run.cancel.clone();
        self.run = Some(run);
        let pending: Vec<String> = self
            .items
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::User(u) => Some(u),
                _ => None,
            })
            .map(|u| {
                u.attachments
                    .iter()
                    .filter(|a| a.content.is_none() && a.error.is_none())
                    .map(|a| a.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if pending.is_empty() {
            self.start_turn(cx);
            return;
        }
        let ws = self.workspace.clone();
        let read = cx.background_spawn(async move {
            pending
                .into_iter()
                .map(|p| {
                    let out = ws.execute("read_file", &json!({ "path": p }), cancel.clone());
                    (p, out)
                })
                .collect::<Vec<_>>()
        });
        let task = cx.spawn(async move |this, cx| {
            let results = read.await;
            this.update(cx, |this, cx| this.on_attachments(results, cx))
                .ok();
        });
        self.set_task(task);
        cx.notify();
    }

    fn on_attachments(&mut self, results: Vec<(String, ToolOutput)>, cx: &mut Context<Self>) {
        let user = self.items.iter_mut().rev().find_map(|i| match i {
            Item::User(u) => Some(u),
            _ => None,
        });
        if let Some(u) = user {
            for (path, out) in results {
                let Some(a) = u.attachments.iter_mut().find(|a| a.path == path) else {
                    continue;
                };
                if out.is_error {
                    a.error = Some(out.content.lines().next().unwrap_or("error").to_string());
                } else {
                    a.lines = match out.display {
                        ToolDisplay::Read { lines, .. } => lines,
                        _ => out.content.lines().count(),
                    };
                    a.content = Some(out.content);
                }
            }
        }
        self.start_turn(cx);
    }

    /// MCP schemas are loaded on demand (too many tools to send them all every request).
    /// An agent's `tools:` list names MCP tools explicitly: those are offered directly.
    pub(crate) fn mcp_deferred(&self) -> bool {
        self.mode == Mode::Build
            && !self.read_only()
            && self.mcp.index.len() > deferred::EAGER_LIMIT
            && self.agent_grants().is_none()
    }

    /// The session's folder is read-only (enforced whatever the agent / Build-Plan mode).
    pub(crate) fn read_only(&self) -> bool {
        self.work_mode == WorkMode::ReadOnly
    }

    /// Tools for the next request (by the selected agent / mode) and the MCP targets.
    /// A read-only folder offers only the workspace's read tools, todo and skills.
    fn request_tools(&self) -> (Vec<llm::ToolSpec>, BTreeMap<String, McpTarget>) {
        let (mut tools, mcp_tools) = self.mode_tools();
        if !self.read_only() {
            return (tools, mcp_tools);
        }
        tools.retain(|t| {
            t.name == TODO_TOOL
                || t.name == SKILL_TOOL
                || self.workspace.permission(&t.name) == Permission::Auto
                    && !t.name.starts_with("mcp__")
                    && t.name != deferred::TOOL_SEARCH
        });
        (tools, BTreeMap::new())
    }

    fn mode_tools(&self) -> (Vec<llm::ToolSpec>, BTreeMap<String, McpTarget>) {
        let grants = self.agent_grants();
        let allows = |tool: &str| grants.as_ref().is_none_or(|g| g.has(tool));
        let mut tools = self.workspace.tool_specs();
        match &grants {
            Some(g) => tools.retain(|t| g.has(&t.name)),
            None if self.mode == Mode::Plan => {
                tools.retain(|t| self.workspace.permission(&t.name) == Permission::Auto)
            }
            None => {}
        }
        if allows(TODO_TOOL) {
            tools.push(todo::spec());
        }
        let auto = self.auto_skills();
        if !auto.is_empty() && allows(SKILL_TOOL) {
            tools.push(skills::tool_spec(&auto));
        }
        let mcp_tools = match &grants {
            Some(g) => {
                let granted: Vec<_> = self
                    .mcp
                    .tool_specs()
                    .into_iter()
                    .filter(|s| g.grants_mcp(&s.name))
                    .collect();
                tools.extend(granted);
                self.mcp.index.clone()
            }
            None if self.mode == Mode::Build => {
                let all = self.mcp.tool_specs();
                if self.mcp_deferred() {
                    // Names only (system prompt) + tool_search; loaded schemas are offered.
                    tools.push(deferred::spec());
                    tools.extend(
                        all.into_iter()
                            .filter(|s| self.loaded_mcp.contains(&s.name)),
                    );
                } else {
                    tools.extend(all);
                }
                self.mcp.index.clone()
            }
            None => BTreeMap::new(),
        };
        (tools, mcp_tools)
    }

    pub(crate) fn system_prompt(&self) -> String {
        let agent = self.active_agent();
        let grants = self.agent_grants();
        let mut s = match agent {
            Some(a) => SYSTEM_BASE.replacen("You are Build", &format!("You are {}", a.name), 1),
            None => String::from(SYSTEM_BASE),
        };
        if self.mode == Mode::Plan {
            s.push_str(SYSTEM_PLAN);
        }
        if self.read_only() {
            s.push_str(SYSTEM_READ_ONLY);
        }
        let has_mcp = !self.read_only() && match &grants {
            Some(g) => self.mcp.index.keys().any(|k| g.grants_mcp(k)),
            None => self.mode == Mode::Build && self.mcp.has_tools(),
        };
        if has_mcp {
            s.push_str(SYSTEM_MCP);
        }
        if self.mcp_deferred() {
            s.push_str(&deferred::prompt_section(&self.mcp.index, &self.loaded_mcp));
        }
        s.push_str("\n\n# Workspace\n\n");
        match &self.context {
            Some(c) => s.push_str(c),
            None => s.push_str(&format!("Root: {}", self.workspace.root.display())),
        }
        if let Some(a) = agent
            && !a.prompt.is_empty()
        {
            s.push_str(&format!(
                "\n\n# Agent: {}\n\n{}",
                a.name,
                a.prompt.trim_end()
            ));
        }
        if grants.as_ref().is_none_or(|g| g.has(SKILL_TOOL)) {
            let section = skills::prompt_section(&self.auto_skills());
            if !section.is_empty() {
                s.push_str("\n\n");
                s.push_str(section.trim_end());
            }
        }
        let forced = self.forced_section();
        if !forced.is_empty() {
            s.push_str("\n\n");
            s.push_str(forced.trim_end());
        }
        s
    }

    /// One model call with the current history.
    fn start_turn(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.cancelled() {
            return;
        }
        run.steps += 1;
        if run.steps > MAX_STEPS {
            self.notice(
                format!("Stopped after {MAX_STEPS} steps. Send a message to let it continue."),
                cx,
            );
            self.finish_run(cx);
            return;
        }
        let Some(model) = self.model.clone() else {
            self.fail("No model selected.".into(), cx);
            return;
        };
        let provider = match &self.config {
            ConfigState::Ready(c) => c.provider(&model.provider).cloned(),
            _ => None,
        };
        let Some(provider) = provider else {
            self.fail(
                format!(
                    "Provider “{}” is not configured (see {}).",
                    model.provider,
                    llm::Config::path().display()
                ),
                cx,
            );
            return;
        };

        let (tools, mcp_tools) = self.request_tools();
        let request = ChatRequest {
            model: model.model.clone(),
            system: Some(self.system_prompt()),
            messages: session::build_history(&self.items),
            tools,
            max_tokens: models::max_output(&model.model),
            temperature: None,
        };
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.offered = request.tools.iter().map(|t| t.name.clone()).collect();
        run.mcp_tools = mcp_tools;
        run.model = Some(model.clone());
        let cancel = run.cancel.clone();
        let mut turn = Turn::new(model.label(), self.mode);
        turn.agent = self.active_agent().map(|a| a.name.clone());
        self.items.push(Item::Turn(turn));

        let (tx, rx) = smol::channel::unbounded::<StreamEvent>();
        let stream = cx.background_spawn(async move {
            let mut send = |ev: StreamEvent| {
                let _ = tx.send_blocking(ev);
            };
            llm::stream_chat(&provider, &request, cancel, &mut send).map_err(|e| format!("{e:#}"))
        });
        let task = cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                let mut batch = vec![ev];
                while let Ok(ev) = rx.try_recv() {
                    batch.push(ev);
                }
                if this
                    .update(cx, |this, cx| this.on_stream_events(batch, cx))
                    .is_err()
                {
                    return;
                }
            }
            let result = stream.await;
            this.update(cx, |this, cx| this.on_stream_done(result, cx))
                .ok();
        });
        self.set_task(task);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn on_stream_events(&mut self, batch: Vec<StreamEvent>, cx: &mut Context<Self>) {
        let follow = self.at_bottom();
        if let Some(t) = self.last_turn_mut() {
            for ev in batch {
                t.apply(ev);
            }
            t.reparse();
        }
        if follow {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn on_stream_done(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_ref() else {
            return;
        };
        let cancelled = run.cancelled();
        let model = run.model.clone();
        let Some(turn) = self.last_turn_mut() else {
            self.finish_run(cx);
            return;
        };
        for t in turn.tools_mut() {
            if t.status == ToolStatus::Streaming {
                t.status = ToolStatus::Cancelled;
            }
        }
        let usage = turn.usage;
        let stop = turn.stop.clone();
        let interrupted = cancelled || stop == Some(StopReason::Cancelled);
        let failed = result.is_err() && !interrupted;
        let has_tools =
            !interrupted && !failed && turn.tools().any(|t| t.status == ToolStatus::Queued);
        if interrupted {
            turn.interrupt();
        } else if failed {
            turn.state = TurnState::Failed;
        } else {
            turn.state = TurnState::Done;
        }
        let empty = turn.is_empty();
        self.account(usage, model.as_ref());
        if let Err(e) = result
            && failed
        {
            if empty {
                self.items.pop();
            }
            self.fail(e, cx);
            return;
        }
        if has_tools {
            self.advance_tools(cx);
            return;
        }
        if !interrupted {
            match stop {
                Some(StopReason::MaxTokens) => {
                    self.notice("Stopped: the response hit the output token limit.", cx)
                }
                Some(StopReason::Other(r)) => self.notice(format!("Stopped: {r}"), cx),
                _ => {}
            }
        }
        self.finish_run(cx);
    }

    fn account(&mut self, usage: Usage, model: Option<&ModelRef>) {
        let Some(model) = model else { return };
        if usage == Usage::default() {
            return;
        }
        self.last_usage = Some((usage, model.model.clone()));
        let t = &mut self.total_usage;
        t.input_tokens += usage.input_tokens;
        t.output_tokens += usage.output_tokens;
        t.cache_read_tokens += usage.cache_read_tokens;
        t.cache_write_tokens += usage.cache_write_tokens;
        match models::price(&model.provider, &model.model) {
            Some(p) => self.cost_usd += models::cost(p, &usage),
            None => self.unpriced = true,
        }
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.items.push(Item::Error(message));
        self.scroll.scroll_to_bottom();
        self.finish_run(cx);
    }

    fn finish_run(&mut self, cx: &mut Context<Self>) {
        self.run = None;
        self.save_session(cx);
        cx.notify();
    }

    // ── Tools ───────────────────────────────────────────────────────────

    /// Handle the next queued tool call of the last turn; when none is left, continue
    /// the loop with the results.
    fn advance_tools(&mut self, cx: &mut Context<Self>) {
        loop {
            let Some(run) = self.run.as_ref() else {
                return;
            };
            let offered = run.offered.clone();
            let mcp_target = |name: &str| run.mcp_tools.get(name).cloned();
            let mode = self.mode;
            let read_only = self.read_only();
            let custom = self.active_agent().map(|a| a.name.clone());
            let Some(Item::Turn(turn)) = self.items.last_mut() else {
                self.finish_run(cx);
                return;
            };
            let Some(call) = turn.tools_mut().find(|t| t.status == ToolStatus::Queued) else {
                self.save_session(cx);
                self.start_turn(cx);
                return;
            };
            let name = call.name.clone();
            let id = call.id.clone();
            let input = call.input.clone().unwrap_or(Value::Null);
            let target = mcp_target(&name);
            if !offered.contains(&name) {
                let why = if read_only {
                    format!(
                        "Tool `{name}` is not available: this chat works in its folder read-only. Describe the change instead."
                    )
                } else if let Some(agent) = &custom {
                    format!("Tool `{name}` is not available to the agent `{agent}`.")
                } else if mode == Mode::Plan {
                    format!(
                        "Tool `{name}` is not available in Plan mode (read-only). Describe the change in your plan instead."
                    )
                } else {
                    format!("Unknown tool `{name}`.")
                };
                call.status = tool_result(why, true);
                continue;
            }
            if name == TODO_TOOL {
                match todo::parse(&input) {
                    Ok(items) => {
                        call.status = tool_result(todo::summary(&items), false);
                        self.todos = items;
                    }
                    Err(e) => call.status = tool_result(e, true),
                }
                continue;
            }
            if name == deferred::TOOL_SEARCH {
                call.status = match deferred::parse_input(&input) {
                    Ok((query, max)) => {
                        let specs = self.mcp.tool_specs();
                        let found = deferred::search(&specs, &query, max);
                        self.loaded_mcp.extend(found.iter().map(|s| s.name.clone()));
                        tool_result(deferred::render(&found), false)
                    }
                    Err(e) => tool_result(e, true),
                };
                continue;
            }
            let auto = match &target {
                Some(t) => self.mcp.auto_approves(t),
                None => name == SKILL_TOOL || self.workspace.permission(&name) == Permission::Auto,
            };
            if auto || self.always_allow.contains(&name) {
                self.execute_tool(id, cx);
                return;
            }
            // MCP calls show their server, tool and arguments instead of a preview.
            call.preview = target
                .is_none()
                .then(|| self.workspace.preview(&name, &input));
            call.status = ToolStatus::Asking;
            call.open = true;
            self.note_input.update(cx, |i, cx| i.set_text("", cx));
            self.scroll.scroll_to_bottom();
            cx.notify();
            return;
        }
    }

    fn execute_tool(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(cancel) = self.run.as_ref().map(|r| r.cancel.clone()) else {
            return;
        };
        let Some(call) = self.last_turn_mut().and_then(|t| t.tool_mut(&id)) else {
            return;
        };
        call.status = ToolStatus::Running;
        let name = call.name.clone();
        let input = call.input.clone().unwrap_or(Value::Null);
        let target = self
            .run
            .as_ref()
            .and_then(|r| r.mcp_tools.get(&name).cloned());
        let job: Task<ToolResult> = if let Some(target) = target {
            self.call_mcp(target, &input, cancel, cx)
        } else if name == SKILL_TOOL {
            let forced = skill_name(&input).filter(|n| self.forced.contains(n));
            let auto = self.auto_skills();
            cx.background_spawn(async move {
                match forced {
                    Some(n) => ToolResult {
                        content: format!(
                            "Skill `{n}` is forced: its instructions are already in the system prompt."
                        ),
                        is_error: false,
                        display: None,
                    },
                    None => extensions::run_skill(&auto, &input),
                }
            })
        } else {
            let ws = self.workspace.clone();
            cx.background_spawn(async move {
                let out = ws.execute(&name, &input, cancel);
                ToolResult {
                    content: out.content,
                    is_error: out.is_error,
                    display: Some(out.display),
                }
            })
        };
        let task = cx.spawn(async move |this, cx| {
            let out = job.await;
            this.update(cx, |this, cx| this.on_tool_done(id, out, cx))
                .ok();
        });
        self.set_task(task);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn on_tool_done(&mut self, id: String, out: ToolResult, cx: &mut Context<Self>) {
        if self.run.is_none() {
            return;
        }
        let Some(call) = self.last_turn_mut().and_then(|t| t.tool_mut(&id)) else {
            return;
        };
        if call.status != ToolStatus::Running {
            return;
        }
        let diff = match (&out.display, out.is_error) {
            (
                Some(ToolDisplay::Diff {
                    path,
                    added,
                    removed,
                    ..
                }),
                false,
            ) => Some((path.clone(), *added, *removed)),
            _ => None,
        };
        if out.display.is_none() {
            // MCP / skill results: collapsed rows, expandable.
            call.open = false;
        }
        call.status = ToolStatus::Done(out);
        if let Some((path, added, removed)) = diff {
            match self.modified.iter_mut().find(|(p, _, _)| *p == path) {
                Some(entry) => {
                    entry.1 += added;
                    entry.2 += removed;
                }
                None => self.modified.push((path, added, removed)),
            }
        }
        self.advance_tools(cx);
    }

    /// The user's answer to a permission prompt.
    pub(crate) fn answer(&mut self, id: &str, decision: Decision, cx: &mut Context<Self>) {
        let note = self.note_input.read(cx).text().trim().to_string();
        let Some(call) = self.last_turn_mut().and_then(|t| t.tool_mut(id)) else {
            return;
        };
        if call.status != ToolStatus::Asking {
            return;
        }
        let name = call.name.clone();
        match decision {
            Decision::Once => self.execute_tool(id.to_string(), cx),
            Decision::Always => {
                self.always_allow.insert(name);
                self.execute_tool(id.to_string(), cx);
            }
            Decision::Reject => {
                call.status = ToolStatus::Rejected(note);
                self.note_input.update(cx, |i, cx| i.set_text("", cx));
                self.advance_tools(cx);
            }
        }
    }

    // ── Interrupt / retry ───────────────────────────────────────────────

    /// Esc / Stop: cancel the stream or running tool, keep partial output.
    pub(crate) fn interrupt(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.run.take() else {
            return;
        };
        run.cancel.store(true, Ordering::Relaxed);
        let model = run.model.clone();
        drop(run);
        match self.items.last_mut() {
            Some(Item::Turn(t)) => {
                let usage = t.usage;
                let streaming = t.state == TurnState::Streaming;
                t.interrupt();
                if streaming {
                    self.account(usage, model.as_ref());
                }
            }
            Some(Item::Shell(s)) if s.output.is_none() => {
                s.output = Some(ToolResult {
                    content: "Interrupted by the user.".into(),
                    is_error: true,
                    display: None,
                });
            }
            _ => {}
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Re-run the last turn after an error card.
    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        loop {
            match self.items.last() {
                Some(Item::Error(_)) => {}
                Some(Item::Turn(t)) if t.state == TurnState::Failed => {}
                _ => break,
            }
            self.items.pop();
        }
        if self.items.is_empty() {
            cx.notify();
            return;
        }
        self.begin_run(cx);
    }

    // ── Shell / commands ────────────────────────────────────────────────

    fn run_shell(&mut self, command: String, cx: &mut Context<Self>) {
        let run = Run::new();
        let cancel = run.cancel.clone();
        self.run = Some(run);
        self.items.push(Item::Shell(ShellRun {
            command: command.clone(),
            output: None,
        }));
        let ws = self.workspace.clone();
        let job = cx.background_spawn(async move {
            ws.execute("bash", &json!({ "command": command }), cancel)
        });
        let task = cx.spawn(async move |this, cx| {
            let out = job.await;
            this.update(cx, |this, cx| {
                if let Some(Item::Shell(s)) = this.items.last_mut()
                    && s.output.is_none()
                {
                    s.output = Some(ToolResult {
                        content: out.content,
                        is_error: out.is_error,
                        display: Some(out.display),
                    });
                }
                this.scroll.scroll_to_bottom();
                this.finish_run(cx);
            })
            .ok();
        });
        self.set_task(task);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn run_command(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = line.split_whitespace().next().unwrap_or("");
        match name {
            "new" => self.open_new_chat(cx),
            "clear" => self.clear_session(cx),
            "model" | "models" => self.open_model_menu(window, cx),
            "plan" => self.set_mode(Mode::Plan, cx),
            "build" => self.set_mode(Mode::Build, cx),
            "skills" => self.notice(self.skills_notice(), cx),
            "mcp" => self.notice(self.mcp_notice(), cx),
            "" => {}
            other => self.notice(
                format!(
                    "Unknown command /{other}. Try /new, /clear, /model, /plan, /build, /skills or /mcp."
                ),
                cx,
            ),
        }
    }

    pub(crate) fn reset_session_state(&mut self) {
        self.items.clear();
        self.todos.clear();
        self.modified.clear();
        self.last_usage = None;
        self.total_usage = Usage::default();
        self.cost_usd = 0.;
        self.unpriced = false;
        self.always_allow.clear();
        self.loaded_mcp.clear();
        self.scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
    }

    /// Start a fresh session (the old one stays on disk).
    pub(crate) fn new_session(&mut self, cx: &mut Context<Self>) {
        self.interrupt(cx);
        self.reset_session_state();
        self.session_id = session::new_id();
        self.persisted = false;
        cx.notify();
    }

    /// Clear this session's messages (same id).
    fn clear_session(&mut self, cx: &mut Context<Self>) {
        self.interrupt(cx);
        self.reset_session_state();
        self.save_session(cx);
        cx.notify();
    }

    // ── Persistence ─────────────────────────────────────────────────────

    pub(crate) fn save_session(&mut self, cx: &mut Context<Self>) {
        let messages = session::build_history(&self.items);
        if messages.is_empty() && !self.persisted {
            return;
        }
        self.persisted = true;
        let saved = Saved {
            id: self.session_id.clone(),
            model: self.model.as_ref().map(ModelRef::label),
            messages,
            cost_usd: self.cost_usd,
            last_usage: self.last_usage.clone(),
            usage: self.total_usage,
            loaded_tools: self.loaded_mcp.iter().cloned().collect(),
            mode: self.mode,
            agent: self.custom_agent.clone(),
            forced_skills: self.forced.iter().cloned().collect(),
            workdir: Some(self.workspace.root.clone()),
            work_mode: self.work_mode,
        };
        let title = session::title(&self.items);
        self.touch_index(title.clone());
        cx.background_spawn(async move {
            if let Err(e) = session::save(&session::sessions_dir(), &saved, title.as_deref()) {
                eprintln!("harness: could not save session {}: {e}", saved.id);
            }
        })
        .detach();
    }

    /// Move the current session to the top of the sidebar index with its latest title.
    fn touch_index(&mut self, title: Option<String>) {
        self.index.retain(|e| e.id != self.session_id);
        self.index.insert(
            0,
            session::IndexEntry {
                id: self.session_id.clone(),
                title: title.unwrap_or_else(|| "Untitled session".into()),
                workdir: Some(self.workspace.root.clone()),
                updated: crate::history_index::now_secs(),
                agent: self.agent_label(),
                cost_usd: self.cost_usd,
                wd_label: crate::history_index::folder_label(&self.workspace.root),
            },
        );
        self.index.truncate(crate::SESSION_LIST_LIMIT);
    }

    /// "Build", "Plan" or the selected custom agent.
    pub(crate) fn agent_label(&self) -> String {
        match (&self.custom_agent, self.mode) {
            (Some(a), _) => a.clone(),
            (None, Mode::Plan) => "Plan".into(),
            (None, Mode::Build) => "Build".into(),
        }
    }

    /// Status of the open session for the sidebar dot.
    pub(crate) fn chat_status(&self) -> ui::nav::ChatStatus {
        use ui::nav::ChatStatus;
        let last_turn = self.items.iter().rev().find_map(|i| match i {
            Item::Turn(t) => Some(t),
            _ => None,
        });
        if self.busy() {
            let asking =
                last_turn.is_some_and(|t| t.tools().any(|c| c.status == ToolStatus::Asking));
            if asking {
                ChatStatus::Waiting
            } else {
                ChatStatus::Running
            }
        } else if last_turn.is_some_and(|t| t.state == TurnState::Failed) {
            ChatStatus::Error
        } else {
            ChatStatus::Idle
        }
    }

    /// Publish pinned chats, the most recent ones and the open one to `ui::nav` (sidebar,
    /// Home, history picker statuses).
    pub(crate) fn publish_sessions(&mut self, cx: &mut Context<Self>) {
        use ui::nav::{ChatSessionInfo, ChatStatus};
        let current_title = session::title(&self.items)
            .map(|t| session::truncate(&t, 60))
            .unwrap_or_else(|| "New session".into());
        let info = |e: &session::IndexEntry| ChatSessionInfo {
            id: e.id.clone().into(),
            title: session::truncate(&e.title, 60).into(),
            status: ChatStatus::Idle,
            pinned: self.flags.is_pinned(&e.id),
            updated: e.updated,
            agent: e.agent.clone().into(),
            workdir: e.wd_label.clone().into(),
            cost_usd: e.cost_usd,
        };
        let current = ChatSessionInfo {
            id: self.session_id.clone().into(),
            title: current_title.into(),
            status: self.chat_status(),
            pinned: self.flags.is_pinned(&self.session_id),
            updated: self
                .index
                .iter()
                .find(|e| e.id == self.session_id)
                .map_or_else(crate::history_index::now_secs, |e| e.updated),
            agent: self.agent_label().into(),
            workdir: self
                .index
                .iter()
                .find(|e| e.id == self.session_id)
                .map(|e| e.wd_label.clone())
                .unwrap_or_else(|| crate::history_index::folder_label(&self.workspace.root))
                .into(),
            cost_usd: self.cost_usd,
        };
        let live: Vec<&session::IndexEntry> = self
            .index
            .iter()
            .filter(|e| !self.flags.is_archived(&e.id))
            .collect();
        let mut recent = 0;
        let mut list: Vec<ChatSessionInfo> = Vec::new();
        for e in &live {
            if e.id == self.session_id {
                list.push(current.clone());
                if !current.pinned {
                    recent += 1;
                }
            } else if self.flags.is_pinned(&e.id) {
                list.push(info(e));
            } else if recent < crate::SIDEBAR_RECENT {
                recent += 1;
                list.push(info(e));
            }
        }
        let mut total = live.len();
        if !self.index.iter().any(|e| e.id == self.session_id) {
            list.insert(0, current);
            total += 1;
        }
        let current = Some(self.session_id.clone().into());
        // Defer: nav observers (the sidebar) must not run inside our own update.
        cx.defer(move |cx| ui::nav::set_chat_sessions(list, current, total, cx));
    }

    /// Reload the session index and pin/archive flags (changed in the history picker).
    pub(crate) fn reload_index(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (index, flags) = cx
                .background_spawn(async {
                    (
                        session::list(&session::sessions_dir(), crate::SESSION_LIST_LIMIT),
                        crate::history_index::Flags::load(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.index = index;
                this.flags = flags;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Handle an open/new/fork request from the sidebar or the history picker, and
    /// reload the list when chats changed on disk.
    pub(crate) fn handle_chat_request(&mut self, cx: &mut Context<Self>) {
        if let Some(nav) = ui::nav::nav_entity(cx) {
            let v = nav.read(cx).chats_version;
            if v != self.chats_version {
                self.chats_version = v;
                self.reload_index(cx);
            }
        }
        let Some(request) = ui::nav::take_chat_request(cx) else {
            return;
        };
        match request {
            ui::nav::ChatRequest::New => self.open_new_chat(cx),
            ui::nav::ChatRequest::Open(id) => {
                self.close_new_chat(cx);
                self.open_session(id.to_string(), false, cx)
            }
            ui::nav::ChatRequest::Fork(id) => {
                self.close_new_chat(cx);
                self.open_session(id.to_string(), true, cx)
            }
        }
    }

    /// Switch to a saved session (the current one is saved first). `fork`: then continue
    /// in a new session with a copy of its history, in its folder.
    fn open_session(&mut self, id: String, fork: bool, cx: &mut Context<Self>) {
        if id == self.session_id {
            if fork {
                self.fork_chat(self.workspace.root.clone(), cx);
            }
            return;
        }
        self.interrupt(cx);
        self.save_session(cx);
        self.switch_seq += 1;
        let seq = self.switch_seq;
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { session::load(&session::sessions_dir(), &id) })
                .await;
            this.update(cx, |this, cx| {
                if this.switch_seq != seq {
                    return;
                }
                match saved {
                    Some(saved) => {
                        this.reset_session_state();
                        this.restore(saved, cx);
                        if fork {
                            this.fork_chat(this.workspace.root.clone(), cx);
                        }
                    }
                    None => this.notice("That session could not be loaded.", cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Restore the most recent session on startup (if nothing happened yet).
    pub(crate) fn restore(&mut self, saved: Saved, cx: &mut Context<Self>) {
        if !self.items.is_empty() || self.busy() {
            return;
        }
        let model_label = saved.model.clone().unwrap_or_default();
        self.items = session::items_from_history(&saved.messages, &model_label);
        self.session_id = saved.id;
        self.persisted = true;
        self.cost_usd = saved.cost_usd;
        self.last_usage = saved.last_usage.clone();
        self.total_usage = saved.usage;
        self.loaded_mcp = saved.loaded_tools.iter().cloned().collect();
        self.mode = saved.mode;
        self.custom_agent = saved.agent.clone();
        self.forced = saved.forced_skills.iter().cloned().collect();
        self.work_mode = saved.work_mode;
        // Sessions saved before workdirs (or whose folder is gone) use the default folder.
        let dir = saved
            .workdir
            .clone()
            .filter(|d| d.is_dir())
            .unwrap_or_else(|| self.wd.default_root.clone());
        self.apply_workdir(dir, cx);
        self.validate_agent(cx);
        self.refresh_placeholder(cx);
        let saved_model = saved.model.as_deref().and_then(ModelRef::parse);
        if let Some(m) = saved_model {
            let known = match &self.config {
                ConfigState::Ready(c) => c.provider(&m.provider).is_some(),
                ConfigState::Loading => true,
                ConfigState::Failed(_) => false,
            };
            if known {
                self.model = Some(m);
            }
        }
        // Replay the last todo list.
        self.todos = self
            .items
            .iter()
            .rev()
            .filter_map(|i| match i {
                Item::Turn(t) => Some(t),
                _ => None,
            })
            .flat_map(|t| t.tools().collect::<Vec<_>>().into_iter().rev())
            .filter(|c| c.name == TODO_TOOL)
            .find_map(|c| c.input.as_ref().and_then(|i| todo::parse(i).ok()))
            .unwrap_or_default();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    // ── Config / models ─────────────────────────────────────────────────

    pub(crate) fn load_config(&mut self, cx: &mut Context<Self>) {
        self.config = ConfigState::Loading;
        self.catalog.clear();
        self.model_tasks.clear();
        let task = cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async { llm::Config::load().map_err(|e| format!("{e:#}")) })
                .await;
            this.update(cx, |this, cx| this.on_config(res, cx)).ok();
        });
        self.config_task = Some(task);
        cx.notify();
    }

    fn on_config(&mut self, res: Result<llm::Config, String>, cx: &mut Context<Self>) {
        let cfg = match res {
            Ok(cfg) => Arc::new(cfg),
            Err(e) => {
                self.config = ConfigState::Failed(e);
                cx.notify();
                return;
            }
        };
        self.catalog = cfg
            .providers
            .iter()
            .map(|p| (p.id.clone(), ProviderModels::Loading))
            .collect();
        if self
            .model
            .as_ref()
            .is_some_and(|m| cfg.provider(&m.provider).is_none())
        {
            self.model = None;
        }
        if self.model.is_none() {
            self.model = cfg
                .default_model
                .as_deref()
                .and_then(ModelRef::parse)
                .filter(|m| cfg.provider(&m.provider).is_some());
        }
        self.model_tasks = cfg
            .providers
            .iter()
            .cloned()
            .map(|p| {
                cx.spawn(async move |this, cx| {
                    let id = p.id.clone();
                    let res = cx
                        .background_spawn(async move {
                            llm::list_models(&p).map_err(|e| format!("{e:#}"))
                        })
                        .await;
                    this.update(cx, |this, cx| this.on_models(id, res, cx)).ok();
                })
            })
            .collect();
        self.config = ConfigState::Ready(cfg);
        cx.notify();
    }

    fn on_models(&mut self, id: String, res: Result<Vec<String>, String>, cx: &mut Context<Self>) {
        let state = match res {
            Ok(mut v) => {
                let mut seen = std::collections::HashSet::new();
                v.retain(|m| seen.insert(m.clone()));
                ProviderModels::Ready(v)
            }
            Err(e) => ProviderModels::Failed(e),
        };
        if let Some(entry) = self.catalog.iter_mut().find(|(p, _)| *p == id) {
            entry.1 = state;
        }
        if self.model.is_none() {
            // First model of the first provider, in config order.
            for (p, m) in &self.catalog {
                match m {
                    ProviderModels::Loading => break,
                    ProviderModels::Ready(v) if !v.is_empty() => {
                        self.model = Some(ModelRef {
                            provider: p.clone(),
                            model: v[0].clone(),
                        });
                        break;
                    }
                    _ => {}
                }
            }
        }
        self.autosend_from_env(cx);
        cx.notify();
    }

    /// Dev/test hook: `HARNESS_CHAT_PROMPT="..."` sends that prompt once, as soon as a model
    /// is available (lets the app be exercised end-to-end without keyboard input).
    fn autosend_from_env(&mut self, cx: &mut Context<Self>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SENT: AtomicBool = AtomicBool::new(false);
        if self.model.is_none() || self.busy() || SENT.load(Ordering::Relaxed) {
            return;
        }
        let Some(text) = std::env::var("HARNESS_CHAT_PROMPT")
            .ok()
            .filter(|t| !t.trim().is_empty())
        else {
            return;
        };
        SENT.store(true, Ordering::Relaxed);
        if !self.items.is_empty() {
            self.new_session(cx);
        }
        self.items.push(Item::User(UserMsg {
            text,
            attachments: Vec::new(),
        }));
        self.scroll.scroll_to_bottom();
        self.begin_run(cx);
    }
}
