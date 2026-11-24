//! A chat turn on its own thread: builds what the agent loop needs from the session, runs it,
//! delivers messages queued in the meantime, and writes the outcome into the header.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use agent::{AgentDeps, AgentEnv, AgentOutcome, AgentSpec, Input, RuleGate, StopCause};
use anyhow::{Result, anyhow, bail};
use base::cancel::CancelToken;
use mcp::OnChange;
use model::{AgentEvent, Event, Session, SessionId, SessionState, WorkMode};
use tools::ToolRegistry;

use crate::bus::lock;
use crate::engine::{Inner, NOT_WIRED, running_marker};
use crate::gate::StoreRules;
use crate::host::{StoreHost, to_input};
use crate::mcp::McpView;
use crate::sink::{BusSink, TurnNotes};

/// Lets MCP tools of every configured server be offered in chat.
const MCP_GRANT: &str = "mcp__*";

/// Starts a turn of `id` on a new thread. The session must already be claimed in `live`.
pub(crate) fn spawn(inner: &Arc<Inner>, id: SessionId, input: Input, cancel: CancelToken) -> Result<()> {
    if inner.is_shut_down() {
        bail!("The app is shutting down");
    }
    let thread_inner = inner.clone();
    let name = format!("chat-{}", base::fmt::truncate_chars(id.as_str(), 20));
    let spawned = std::thread::Builder::new().name(name).spawn(move || drive(&thread_inner, &id, input, &cancel));
    match spawned {
        Ok(handle) => {
            let mut threads = lock(&inner.threads);
            threads.retain(|h| !h.is_finished());
            threads.push(handle);
            Ok(())
        }
        Err(error) => Err(anyhow!("could not start the chat's thread: {error}")),
    }
}

/// The turn loop of one session: one agent run, then another for every message that was
/// queued after the agent last looked.
fn drive(inner: &Arc<Inner>, id: &SessionId, mut input: Input, cancel: &CancelToken) {
    let _ = inner.store.set_marker(&running_marker(id), "1");
    inner.set_state(id, SessionState::Running);
    let capacity = || inner.store.settings().map_or(1, |s| s.slots);
    let slot = inner.slots.acquire(capacity, cancel);
    loop {
        let (outcome, notes) = if slot.is_some() { run_guarded(inner, id, input, cancel) } else { cancelled() };
        // Only a turn that ended on its own goes on with what was queued; queued messages stay
        // for the person after a cancel, a question, a limit or a failure.
        let deliver = outcome.stop == StopCause::Finished && !cancel.is_cancelled();
        let next = if deliver { inner.live.next(id) } else { None };
        let state = if next.is_some() { SessionState::Running } else { final_state(&outcome.stop) };
        inner.finish_turn(id, state, &outcome, &notes);
        let next = next.or_else(|| inner.live.release(id, deliver));
        match next {
            Some(message) => {
                inner.set_state(id, SessionState::Running);
                input = Input::User(to_input(message));
            }
            None => break,
        }
    }
    let _ = inner.store.remove_marker(&running_marker(id));
    drop(slot);
}

fn cancelled() -> (AgentOutcome, TurnNotes) {
    let outcome = AgentOutcome {
        stop: StopCause::Cancelled,
        text: String::new(),
        payload: None,
        totals: Default::default(),
        files: Vec::new(),
    };
    (outcome, TurnNotes::default())
}

fn final_state(stop: &StopCause) -> SessionState {
    match stop {
        StopCause::Finished | StopCause::Cancelled | StopCause::StepLimit | StopCause::Budget => SessionState::Idle,
        StopCause::Waiting { .. } => SessionState::Waiting,
        StopCause::Failed { .. } => SessionState::Failed,
    }
}

/// One agent run that cannot take the thread down: setup errors and panics become a failed
/// outcome, reported like the loop's own failures.
fn run_guarded(inner: &Arc<Inner>, id: &SessionId, input: Input, cancel: &CancelToken) -> (AgentOutcome, TurnNotes) {
    let result = catch_unwind(AssertUnwindSafe(|| run_once(inner, id, input, cancel)));
    let error = match result {
        Ok(Ok(done)) => return done,
        Ok(Err(error)) => format!("{error:#}"),
        Err(panic) => {
            let what = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            format!("the agent crashed: {what}")
        }
    };
    inner.publish(Event::Agent { session: id.clone(), event: AgentEvent::Failed { error: error.clone() } });
    let (mut outcome, notes) = cancelled();
    outcome.stop = StopCause::Failed { error };
    (outcome, notes)
}

fn run_once(inner: &Arc<Inner>, id: &SessionId, input: Input, cancel: &CancelToken) -> Result<(AgentOutcome, TurnNotes)> {
    let session = inner.session(id)?;
    let folder = session.workdir.as_ref().map(|w| w.path.clone());
    let catalog = inner.catalog_for(folder.as_deref());
    let spec = spec_for(inner, &session, &catalog)?;
    let env = AgentEnv {
        session: id.clone(),
        depth: session.depth,
        workdir: folder.clone(),
        repo: session.workdir.as_ref().map(|w| w.source.clone().unwrap_or_else(|| w.path.clone())),
        env: BTreeMap::new(),
        read_only: session.workdir.as_ref().is_some_and(|w| w.mode == WorkMode::ReadOnly),
        mode: session.mode,
        run: None,
        node: None,
        created_by: None,
    };
    let host = StoreHost::new(inner);
    let rules = StoreRules(inner);
    let gate = RuleGate::new(&rules, |request, cancel| inner.ask_person(request, cancel));
    let mcp = mcp_view(inner, folder.as_deref());
    let deps = AgentDeps {
        llm: &*inner.llm,
        host: &host,
        gate: &gate,
        mcp: Some(&mcp),
        skills: &catalog,
        clock: &*inner.clock,
        tools: ToolRegistry::new(),
    };
    let mut sink = BusSink::new(inner, id.clone());
    let outcome = agent::run(&spec, &env, &deps, input, &mut sink, cancel);
    Ok((outcome, sink.notes))
}

/// What the agent of a session is: its runnable's definition with the session's model,
/// forced skills and the app's limits.
pub(crate) fn spec_for(inner: &Inner, session: &Session, catalog: &catalog::Catalog) -> Result<AgentSpec> {
    let Some(runnable) = catalog.get(&session.runnable) else {
        bail!("{} no longer exists in the library", session.runnable);
    };
    let Some(def) = runnable.agent.as_ref() else { bail!("{NOT_WIRED}") };
    let mut spec = AgentSpec::from_def(runnable.name.clone(), runnable.reference.clone(), def);
    spec.model = session.model.clone();
    if spec.model.is_empty() {
        bail!("No model is configured: add a provider to config.toml or pick a model for this chat");
    }
    for skill in &session.forced_skills {
        if !spec.skills.forced.contains(skill) {
            spec.skills.forced.push(skill.clone());
        }
    }
    if !spec.tools.allow.iter().any(|a| a == MCP_GRANT) {
        spec.tools.allow.push(MCP_GRANT.to_string());
    }
    spec.limits = inner.limits.clone();
    // A chat an autopilot started works under the autopilot's budget too.
    if let Some(cap) = inner.store.marker(&super::budget_marker(&session.id))?.and_then(|v| v.parse::<f64>().ok()) {
        spec.budget_usd = Some(spec.budget_usd.map_or(cap, |own| own.min(cap)));
    }
    if let Ok(settings) = inner.store.settings() {
        spec.limits.compact_at = f64::from(settings.compact_at);
    }
    Ok(spec)
}

/// The MCP servers of a session's folder, started; their state changes refresh running
/// sessions.
pub(crate) fn mcp_view(inner: &Arc<Inner>, folder: Option<&std::path::Path>) -> McpView {
    inner.mcp.view(folder, &mcp_on_change(inner))
}

/// The MCP servers of a session's folder, without starting any.
pub(crate) fn mcp_peek(inner: &Arc<Inner>, folder: Option<&std::path::Path>) -> McpView {
    inner.mcp.peek(folder, &mcp_on_change(inner))
}

fn mcp_on_change(inner: &Arc<Inner>) -> OnChange {
    let weak = Arc::downgrade(inner);
    Arc::new(move || {
        let Some(inner) = weak.upgrade() else { return };
        for id in inner.live.running() {
            if let Ok(Some(session)) = inner.store.session(&id) {
                inner.publish(Event::SessionChanged { session });
            }
        }
    })
}

impl Inner {
    /// Writes what a turn added up to into the session header, and settles sub-agent sessions
    /// it left behind.
    fn finish_turn(&self, id: &SessionId, state: SessionState, outcome: &AgentOutcome, notes: &TurnNotes) {
        let spent = self.spent_below(id).ok();
        let usage = self.store.spend_for_session(id).ok().map(|t| t.usage);
        let now = self.now();
        let result = self.update_session(id, |s| {
            s.state = state;
            s.updated = s.updated.max(now);
            if let Some(spent) = spent {
                s.cost_usd = spent;
            }
            if let Some(usage) = usage {
                s.usage = usage;
            }
            if let Some(tokens) = notes.context_tokens {
                s.context_tokens = tokens;
            }
            if let Some(window) = notes.context_window {
                s.context_window = window;
            }
            if notes.pr_url.is_some() {
                s.pr_url = notes.pr_url.clone();
            }
        });
        if let Err(error) = result {
            self.notice_error(&format!("Could not save the chat: {error:#}"));
        }
        let failed = matches!(outcome.stop, StopCause::Failed { .. } | StopCause::Cancelled);
        for (child, ok) in &notes.finished_children {
            self.settle_child(child, if *ok { SessionState::Done } else { SessionState::Failed });
        }
        // Sub-agents still marked running stopped with their parent.
        if let Ok(children) = self.store.child_sessions(id) {
            for child in children.into_iter().filter(|c| c.state == SessionState::Running) {
                self.settle_child(&child.id, if failed { SessionState::Failed } else { SessionState::Done });
            }
        }
    }

    fn settle_child(&self, child: &SessionId, state: SessionState) {
        let spent = self.spent_below(child).ok();
        let usage = self.store.spend_for_session(child).ok().map(|t| t.usage);
        let _ = self.update_session(child, |s| {
            s.state = state;
            if let Some(spent) = spent {
                s.cost_usd = spent;
            }
            if let Some(usage) = usage {
                s.usage = usage;
            }
        });
    }
}
