//! The turn loop: ask the model, store what it said, run its tool calls, store the results,
//! and again, until something ends the run.
//!
//! Order matters here. The assistant message is stored before its tools run and the results
//! are stored in one message right after, so at any moment the stored conversation is either
//! valid or one "interrupted" result short of it, which the next run supplies.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Result, bail};
use base::cancel::CancelToken;
use llm::{ChatRequest, LlmError, pricing};
use model::{
    AgentEvent, ContentBlock, FileChange, Message, ModelRef, Role, Skill, StopReason, StoredMessage, TodoItem, Usage,
    UsageEntry,
};
use serde_json::Value;
use tools::ToolContext;

use crate::compact::Compaction;
use crate::history;
use crate::host::HostBlobs;
use crate::notes::{note, pr_links};
use crate::prompt::system_prompt;
use crate::session_tools::{finish, skill, text_display};
use crate::sink::Emit;
use crate::spec::{AgentDeps, AgentEnv, AgentOutcome, AgentSpec, Finish, Input, Limits, StopCause, Totals, UserInput};
use crate::stream::{Call, Collector, Live, Reply};
use crate::toolbox::Toolbox;

/// Times a reply cut off at the output limit is asked to continue within one run.
const MAX_CONTINUES: u32 = 2;
/// The longest pause a provider's `retry-after` is given.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// How one model call ended, when it did not fail.
pub(crate) enum Called {
    Reply(Reply),
    /// The token fired; the reply holds what had arrived.
    Cancelled(Reply),
    /// The request does not fit the context window.
    TooLong(String),
}

/// One run in progress: the borrowed arguments and what the loop keeps in memory, mirroring
/// the store.
pub(crate) struct Session<'a> {
    pub spec: &'a AgentSpec,
    pub env: &'a AgentEnv,
    pub deps: &'a AgentDeps<'a>,
    pub sink: &'a dyn Emit,
    pub cancel: &'a CancelToken,
    system: String,
    pub toolbox: Toolbox,
    /// Skills the `skill` tool may load.
    pub offered_skills: Vec<Skill>,
    /// The messages still sent to the model, as stored.
    pub messages: Vec<StoredMessage>,
    pub summary: Option<String>,
    pub todos: Vec<TodoItem>,
    pub files: Vec<FileChange>,
    /// Skills whose text is in the uncompacted conversation.
    pub loaded_skills: Vec<String>,
    pub totals: Totals,
    steps: u32,
    last_text: String,
    /// What the latest conversation request filled, plus its answer: the size of the next one.
    pub context_tokens: u64,
    /// A compaction at the threshold failed in this run and is not tried again.
    threshold_compaction_failed: bool,
    /// Sub-agents created in this run, in all and by template name.
    pub spawned: u32,
    pub spawned_by_kind: BTreeMap<String, u32>,
    /// The finish tool's input, once it was called.
    pub payload: Option<Value>,
    /// The question of an `ask_user` call of the current step.
    pub question: Option<String>,
    prs: Vec<String>,
}

/// Runs the loop with a sink that sub-agents' threads can share. See [`crate::run`].
pub(crate) fn run_shared(
    spec: &AgentSpec,
    env: &AgentEnv,
    deps: &AgentDeps<'_>,
    input: Input,
    sink: &dyn Emit,
    cancel: &CancelToken,
) -> AgentOutcome {
    let started = deps.clock.now_ms();
    let mut session = match Session::open(spec, env, deps, sink, cancel) {
        Ok(session) => session,
        Err(error) => {
            let error = format!("{error:#}");
            sink.emit(&env.session, AgentEvent::Failed { error: error.clone() });
            return AgentOutcome {
                stop: StopCause::Failed { error },
                text: String::new(),
                payload: None,
                totals: Totals::default(),
                files: Vec::new(),
            };
        }
    };
    let stop = session.drive(input).unwrap_or_else(|error| StopCause::Failed { error: format!("{error:#}") });
    session.close(stop, started)
}

impl<'a> Session<'a> {
    fn open(
        spec: &'a AgentSpec,
        env: &'a AgentEnv,
        deps: &'a AgentDeps<'a>,
        sink: &'a dyn Emit,
        cancel: &'a CancelToken,
    ) -> Result<Self> {
        let host = deps.host;
        let messages = host.messages(&env.session)?;
        let offered_skills = skill::offered(&spec.skills, deps.skills);
        let mut toolbox = Toolbox::build(spec, env, deps, &offered_skills);
        toolbox.restore(&messages);
        let system = system_prompt(spec, env, deps, &toolbox, &offered_skills);
        let context_tokens = messages
            .iter()
            .rev()
            .find(|m| m.message.role == Role::Assistant && m.usage.total() > 0)
            .map_or(0, |m| m.usage.context_tokens() + m.usage.output);
        Ok(Self {
            spec,
            env,
            deps,
            sink,
            cancel,
            system,
            toolbox,
            offered_skills,
            loaded_skills: loaded_skills(&messages),
            messages,
            summary: host.summary(&env.session)?,
            todos: host.todos(&env.session)?,
            files: host.files(&env.session)?,
            totals: Totals::default(),
            steps: 0,
            last_text: String::new(),
            context_tokens,
            threshold_compaction_failed: false,
            spawned: 0,
            spawned_by_kind: BTreeMap::new(),
            payload: None,
            question: None,
            prs: Vec::new(),
        })
    }

    pub(crate) fn emit(&self, event: AgentEvent) {
        self.sink.emit(&self.env.session, event);
    }

    pub(crate) fn now(&self) -> i64 {
        self.deps.clock.now_ms()
    }

    /// Where local tools run; `None` without a workdir.
    pub(crate) fn tool_context(&self) -> Option<ToolContext<'a>> {
        let root = self.env.workdir.as_deref()?;
        Some(ToolContext { root, env: &self.env.env, cancel: self.cancel })
    }

    fn drive(&mut self, input: Input) -> Result<StopCause> {
        self.answer_dangling()?;
        match input {
            Input::User(user) => self.append_user(user)?,
            Input::Item(text) => self.append_user(UserInput::text(text))?,
            Input::Continue => {}
            Input::Compact => {
                return Ok(match self.compact(true)? {
                    Compaction::Cancelled => StopCause::Cancelled,
                    Compaction::Done => StopCause::Finished,
                    Compaction::Nothing => {
                        self.emit(AgentEvent::Notice { text: "There is nothing to compact yet.".into() });
                        StopCause::Finished
                    }
                });
            }
        }
        // A request must end with something for the model to answer.
        let ends_with_assistant = match self.messages.last() {
            None if self.summary.is_none() => bail!("There is no conversation to continue."),
            None => true,
            Some(last) => last.message.role == Role::Assistant,
        };
        if ends_with_assistant {
            self.append_note("Continue from where you stopped.")?;
        }

        let (mut nudges, mut continues) = (0, 0);
        loop {
            if self.cancel.is_cancelled() {
                return Ok(StopCause::Cancelled);
            }
            for queued in self.deps.host.take_queued(&self.env.session) {
                self.append_user(queued)?;
            }
            if self.steps >= self.spec.step_limit() {
                let text = format!(
                    "Stopped after {}. Send a message to let it continue.",
                    base::fmt::plural(u64::from(self.steps), "step", "steps")
                );
                self.emit(AgentEvent::Notice { text });
                return Ok(StopCause::StepLimit);
            }
            if let Some(budget) = self.spec.budget_usd {
                let spent = self.deps.host.spent(&self.env.session)?;
                if spent >= budget {
                    let text = format!(
                        "Stopped: the budget of {} is spent ({}).",
                        base::fmt::cost(budget),
                        base::fmt::cost(spent)
                    );
                    self.emit(AgentEvent::Notice { text });
                    return Ok(StopCause::Budget);
                }
            }
            if self.compact_at_threshold() {
                return Ok(StopCause::Cancelled);
            }

            self.steps += 1;
            self.totals.steps += 1;
            let (reply, cancelled) = self.converse()?;
            let text = reply.text();
            if !text.trim().is_empty() {
                self.last_text = text.clone();
            }
            if reply.has_content() {
                let message = Message { role: Role::Assistant, content: reply.content.clone() };
                let model = Some(self.spec.model.clone());
                self.commit(message, reply.usage.unwrap_or_default(), reply.cost_usd, model)?;
            }
            if cancelled {
                self.answer_interrupted(&reply.calls)?;
                return Ok(StopCause::Cancelled);
            }
            self.note_prs(&text);

            if reply.calls.is_empty() {
                if reply.stop == Some(StopReason::MaxTokens) && continues < MAX_CONTINUES {
                    continues += 1;
                    self.append_note(
                        "Your reply was cut off at the output limit. Continue exactly where it stopped, and use \
                         tools for long content.",
                    )?;
                    continue;
                }
                let queued = self.deps.host.take_queued(&self.env.session);
                if !queued.is_empty() {
                    for user in queued {
                        self.append_user(user)?;
                    }
                    continue;
                }
                if let Finish::Tool(tool) = &self.spec.finish
                    && nudges < finish::MAX_NUDGES
                {
                    nudges += 1;
                    self.append_note(&finish::nudge(tool))?;
                    continue;
                }
                return Ok(StopCause::Finished);
            }

            self.run_calls(&reply.calls)?;
            if self.cancel.is_cancelled() {
                return Ok(StopCause::Cancelled);
            }
            if self.payload.is_some() {
                return Ok(StopCause::Finished);
            }
            if let Some(question) = self.question.take() {
                return Ok(StopCause::Waiting { question });
            }
        }
    }

    /// Compacts when the last request filled more of the context window than the limits allow.
    /// Returns whether the run was cancelled meanwhile.
    ///
    /// A failure here does not end the run: the next request may still fit, and when it does
    /// not, the compaction it forces is the one whose failure counts. Ending the run instead
    /// would lock a nearly full session, because every later run would start the same way.
    fn compact_at_threshold(&mut self) -> bool {
        let window = pricing::context_window(&self.spec.model);
        let full = self.context_tokens as f64 > self.spec.limits.compact_at * window as f64;
        if !full || self.threshold_compaction_failed {
            return false;
        }
        match self.compact(false) {
            Ok(Compaction::Cancelled) => true,
            Ok(Compaction::Done | Compaction::Nothing) => false,
            Err(error) => {
                self.threshold_compaction_failed = true;
                // Closes the `CompactionStarted` a watcher has seen: nothing was replaced.
                self.emit(AgentEvent::Compacted { summarised: 0, context_tokens: self.context_tokens });
                self.emit(AgentEvent::Notice {
                    text: format!("Compaction was skipped: {error:#}"),
                });
                false
            }
        }
    }

    /// Stores a message, tells the sink and keeps it for the next request.
    pub(crate) fn commit(&mut self, message: Message, usage: Usage, cost_usd: f64, model: Option<ModelRef>) -> Result<()> {
        let stored = StoredMessage { seq: 0, at: self.now(), message, compacted: false, model, usage, cost_usd };
        let stored = self.deps.host.append(&self.env.session, stored)?;
        self.emit(AgentEvent::MessageCommitted { message: stored.clone() });
        self.messages.push(stored);
        Ok(())
    }

    fn append_user(&mut self, input: UserInput) -> Result<()> {
        let mut content = Vec::new();
        if !input.text.trim().is_empty() {
            content.push(ContentBlock::text(input.text));
        }
        content.extend(input.images.into_iter().map(|image| ContentBlock::Image {
            blob: image.blob,
            media_type: image.media_type,
            name: image.name,
            bytes: image.bytes,
        }));
        if content.is_empty() {
            return Ok(());
        }
        self.commit(Message { role: Role::User, content }, Usage::default(), 0.0, None)
    }

    /// Stores a user message written by the loop itself.
    fn append_note(&mut self, text: &str) -> Result<()> {
        self.commit(Message::user(note(text)), Usage::default(), 0.0, None)
    }

    /// Answers tool calls a previous run left unanswered (a crash, a closed app).
    fn answer_dangling(&mut self) -> Result<()> {
        let Some(last) = self.messages.last() else { return Ok(()) };
        let calls: Vec<(String, String)> =
            last.message.tool_uses().map(|(id, name, _)| (id.to_string(), name.to_string())).collect();
        if history::dangling(&self.messages).is_empty() {
            return Ok(());
        }
        self.store_interrupted(&calls)
    }

    /// Answers the complete tool calls of a reply that was cancelled while streaming.
    fn answer_interrupted(&mut self, calls: &[Call]) -> Result<()> {
        let calls: Vec<(String, String)> = calls.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
        self.store_interrupted(&calls)
    }

    fn store_interrupted(&mut self, calls: &[(String, String)]) -> Result<()> {
        if calls.is_empty() {
            return Ok(());
        }
        let mut results = Vec::with_capacity(calls.len());
        for (id, name) in calls {
            let display = text_display(name, history::INTERRUPTED);
            self.emit(AgentEvent::ToolFinished { id: id.clone(), display, is_error: true });
            results.push(history::interrupted(id));
        }
        self.commit(Message { role: Role::User, content: results }, Usage::default(), 0.0, None)
    }

    fn request(&self) -> ChatRequest {
        ChatRequest {
            system: self.system.clone(),
            messages: history::request_messages(self.summary.as_deref(), &self.messages),
            tools: self.toolbox.specs(),
            max_tokens: 0,
            temperature: None,
            reasoning: self.spec.reasoning,
        }
    }

    /// One conversation step: the request built from what is stored, compacting once when it
    /// does not fit. Returns the reply and whether the run was cancelled meanwhile.
    fn converse(&mut self) -> Result<(Reply, bool)> {
        let mut compacted = false;
        loop {
            let request = self.request();
            match self.call_model(&request, true)? {
                Called::Reply(reply) => return Ok((reply, false)),
                Called::Cancelled(reply) => return Ok((reply, true)),
                Called::TooLong(message) if compacted => {
                    bail!("The conversation does not fit the model's context even after compaction: {message}")
                }
                Called::TooLong(message) => {
                    compacted = true;
                    match self.compact(true)? {
                        Compaction::Done => {}
                        Compaction::Cancelled => return Ok((Reply::default(), true)),
                        Compaction::Nothing => bail!(
                            "The conversation does not fit the model's context and there is nothing to summarise: {message}"
                        ),
                    }
                }
            }
        }
    }

    /// Streams one request, retrying what is worth retrying. `live` requests are conversation
    /// steps: their deltas and tool calls go to the sink. Every attempt that reported usage is
    /// accounted, also a failed or cancelled one.
    pub(crate) fn call_model(&mut self, request: &ChatRequest, live: bool) -> Result<Called> {
        let mut attempt = 0;
        loop {
            if live {
                self.emit(AgentEvent::TurnStarted);
            }
            let mut collector = if live { Collector::avoiding(self.call_ids()) } else { Collector::default() };
            let result = {
                let (sink, session, toolbox) = (self.sink, &self.env.session, &self.toolbox);
                let context = self.tool_context();
                let blobs = HostBlobs(self.deps.host);
                self.deps.llm.stream(&self.spec.model, request, &blobs, self.cancel, &mut |event| {
                    for item in collector.on_event(event) {
                        if !live {
                            continue;
                        }
                        sink.emit(
                            session,
                            match item {
                                Live::Text(text) => AgentEvent::TextDelta { text },
                                Live::Thinking(text) => AgentEvent::ThinkingDelta { text },
                                Live::ToolStarted { id, name } => AgentEvent::ToolCallStarted { id, name },
                                Live::ToolReady(call) => {
                                    let (class, display) = match &call.error {
                                        Some(error) => (model::ToolClass::Internal, text_display(&call.name, error)),
                                        None => toolbox.describe(&call.name, &call.input, context.as_ref()),
                                    };
                                    AgentEvent::ToolCallReady { id: call.id, name: call.name, class, display }
                                }
                            },
                        );
                    }
                })
            };
            // A stream that failed, or ended without its `Stop`, while the token is set was cut
            // off by the cancel, whatever the client reported.
            let cancelled = matches!(result, Err(LlmError::Cancelled))
                || collector.stop() == Some(StopReason::Cancelled)
                || (self.cancel.is_cancelled() && (result.is_err() || collector.stop().is_none()));
            let mut reply = collector.finish();
            if let Some(usage) = reply.usage {
                reply.cost_usd = self.account(usage, live)?;
            }
            let error = match result {
                _ if cancelled => return Ok(Called::Cancelled(reply)),
                Ok(()) => return Ok(Called::Reply(reply)),
                Err(LlmError::ContextTooLong(message)) => return Ok(Called::TooLong(message)),
                Err(error) => error,
            };
            if !error.is_retryable() || attempt >= self.spec.limits.retries {
                match attempt {
                    0 => bail!("{error}"),
                    n => bail!("{error} (after {})", base::fmt::plural(u64::from(n) + 1, "attempt", "attempts")),
                }
            }
            attempt += 1;
            self.emit(AgentEvent::Retrying { attempt, reason: error.to_string() });
            if !self.cancel.sleep(backoff(&self.spec.limits, attempt, &error)) {
                return Ok(Called::Cancelled(Reply::default()));
            }
        }
    }

    /// Ids of the tool calls in the messages still sent to the model.
    fn call_ids(&self) -> Vec<String> {
        self.messages.iter().flat_map(|m| m.message.tool_uses()).map(|(id, _, _)| id.to_string()).collect()
    }

    /// Adds a model call to the totals and the ledger and reports it. Returns its cost.
    fn account(&mut self, usage: Usage, live: bool) -> Result<f64> {
        let model = &self.spec.model;
        let cost_usd = pricing::price(model).map_or(0.0, |price| pricing::cost(price, &usage));
        self.totals.usage.add(usage);
        self.totals.cost_usd += cost_usd;
        self.deps.host.record_usage(&UsageEntry {
            at: self.now(),
            session: Some(self.env.session.clone()),
            run: self.env.run.clone(),
            node: self.env.node.clone(),
            runnable: self.spec.runnable.clone(),
            name: self.spec.name.clone(),
            created_by: self.env.created_by.clone(),
            model: model.clone(),
            usage,
            cost_usd,
            imported: false,
        })?;
        if live {
            self.context_tokens = usage.context_tokens() + usage.output;
        }
        self.emit(AgentEvent::Usage {
            usage,
            cost_usd,
            context_tokens: if live { usage.context_tokens() } else { self.context_tokens },
            context_window: pricing::context_window(model),
        });
        Ok(cost_usd)
    }

    /// Reports pull request links not seen before in this run.
    pub(crate) fn note_prs(&mut self, text: &str) {
        for url in pr_links(text) {
            if !self.prs.contains(&url) {
                self.prs.push(url.clone());
                self.emit(AgentEvent::PrDetected { url });
            }
        }
    }

    fn close(self, stop: StopCause, started: i64) -> AgentOutcome {
        match &stop {
            StopCause::Failed { error } => self.emit(AgentEvent::Failed { error: error.clone() }),
            StopCause::Finished | StopCause::Waiting { .. } => {
                self.emit(AgentEvent::TurnFinished { stop: StopReason::EndTurn })
            }
            StopCause::Cancelled => self.emit(AgentEvent::TurnFinished { stop: StopReason::Cancelled }),
            StopCause::StepLimit | StopCause::Budget => self.emit(AgentEvent::TurnFinished { stop: StopReason::Other }),
        }
        let mut totals = self.totals;
        totals.duration_ms = (self.now() - started).max(0);
        AgentOutcome { stop, text: self.last_text, payload: self.payload, totals, files: self.files }
    }
}

/// The pause before retry number `attempt`: what the provider asked for, else the base
/// doubled each time, up to the limit.
fn backoff(limits: &Limits, attempt: u32, error: &LlmError) -> Duration {
    if let LlmError::RateLimited { retry_after: Some(after), .. } = error {
        return (*after).min(MAX_RETRY_AFTER);
    }
    let factor = 1u32 << attempt.saturating_sub(1).min(16);
    limits.retry_base.saturating_mul(factor).min(limits.retry_max)
}

/// Skills the stored conversation loaded with the `skill` tool.
pub(crate) fn loaded_skills(messages: &[StoredMessage]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for input in history::successful_calls(messages, crate::session_tools::names::SKILL) {
        if let Ok(name) = skill::parse(input)
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_limit_and_honours_retry_after() {
        let limits = Limits { retry_base: Duration::from_secs(2), retry_max: Duration::from_secs(10), ..Default::default() };
        let overloaded = LlmError::Overloaded("busy".into());
        let pauses: Vec<u64> = (1..=5).map(|n| backoff(&limits, n, &overloaded).as_secs()).collect();
        assert_eq!(pauses, [2, 4, 8, 10, 10]);
        let limited = |secs| LlmError::RateLimited { retry_after: Some(Duration::from_secs(secs)), message: String::new() };
        assert_eq!(backoff(&limits, 1, &limited(7)).as_secs(), 7);
        assert_eq!(backoff(&limits, 1, &limited(900)), MAX_RETRY_AFTER);
        let unhinted = LlmError::RateLimited { retry_after: None, message: String::new() };
        assert_eq!(backoff(&limits, 2, &unhinted).as_secs(), 4);
    }
}
