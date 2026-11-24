//! Sub-agents: `spawn_agent` calls become child sessions that run the same loop, several at
//! once on their own threads, and hand their last message back as the tool result.

use std::panic::{AssertUnwindSafe, catch_unwind};

use base::cancel::CancelToken;
use model::{AgentEvent, PermissionRequest, RequestId, SessionId, ToolClass};
use tools::ToolOutput;

use crate::calls::{Refused, interrupted_output, panic_text};
use crate::host::ChildSession;
use crate::session_tools::names::SPAWN_AGENT;
use crate::session_tools::spawn::{self, GENERAL, SpawnInput};
use crate::session_tools::text_display;
use crate::sink::Emit;
use crate::spec::{AgentDeps, AgentEnv, AgentOutcome, AgentSpec, Input, StopCause, Totals};
use crate::stream::Call;
use crate::tree::{Cap, Full, Lineage, MAX_ALIVE, MAX_CREATED, Slot, child_budget};
use crate::turn::{Session, run_shared};

/// Stack of a sub-agent's thread: the loop is shallow, but tools and JSON are not ours.
const STACK_BYTES: usize = 8 * 1024 * 1024;

/// The nesting no approval reaches. Below it a person decides about every level past the
/// spawn policy's `max_depth`; a gate that allows everything must still not let agents create
/// agents without end.
pub(crate) const DEPTH_CEILING: u32 = 8;

/// A sub-agent that passed every check and has a session.
struct Plan<'t> {
    name: String,
    task: String,
    spec: AgentSpec,
    env: AgentEnv,
    /// The budgets above the sub-agent: its parent's.
    caps: Vec<Cap>,
    /// Its place in the tree, held until its result is taken.
    slot: Slot<'t>,
}

impl<'a> Session<'a> {
    /// Runs a group of `spawn_agent` calls at the same time and returns one result per call,
    /// in the calls' order. Calls the spawn policy refuses, or a person does, get an error
    /// result and start nothing.
    pub(crate) fn spawn_batch(&mut self, calls: &[Call]) -> Vec<ToolOutput> {
        let mut alive = 0;
        let plans: Vec<Result<Plan<'a>, Refused>> = calls.iter().map(|call| self.plan(call, &mut alive)).collect();

        let (deps, sink, cancel, env) = (self.deps, self.sink, self.cancel, self.env);
        let parent = &env.session;
        let outcomes: Vec<Option<AgentOutcome>> = std::thread::scope(|scope| {
            // Every thread is started before the first is waited for.
            let started: Vec<_> = plans
                .iter()
                .map(|plan| {
                    let plan = plan.as_ref().ok()?;
                    let thread = std::thread::Builder::new().name("subagent".into()).stack_size(STACK_BYTES);
                    let spawned = thread.spawn_scoped(scope, move || run_child(plan, deps, sink, cancel, parent));
                    // No thread to be had: the sub-agent runs here, before the next one starts.
                    Some(spawned.map_err(|_| run_child(plan, deps, sink, cancel, parent)))
                })
                .collect();
            started
                .into_iter()
                .enumerate()
                .map(|(at, started)| {
                    Some(match started? {
                        Ok(handle) => {
                            handle.join().unwrap_or_else(|panic| crashed(plan_of(&plans, at), deps, &panic_text(&*panic)))
                        }
                        Err(outcome) => outcome,
                    })
                })
                .collect()
        });

        let mut outputs = Vec::with_capacity(calls.len());
        for (plan, outcome) in plans.into_iter().zip(outcomes) {
            outputs.push(match (plan, outcome) {
                (Ok(plan), Some(outcome)) => {
                    self.totals.cost_usd += outcome.totals.cost_usd;
                    self.totals.usage.add(outcome.totals.usage);
                    let summary = child_summary(&outcome);
                    ToolOutput {
                        display: text_display(&format!("Sub-agent · {}", plan.name), &summary),
                        content: summary,
                        is_error: !outcome.is_finished(),
                        files: outcome.files,
                    }
                }
                (Err(refused), _) => *refused,
                (Ok(_), None) => interrupted_output(SPAWN_AGENT),
            });
        }
        outputs
    }

    /// Checks one call against the spawn policy and creates the child's session.
    fn plan(&mut self, call: &Call, alive: &mut u32) -> Result<Plan<'a>, Refused> {
        let refuse = |message: String| Box::new(ToolOutput::error(SPAWN_AGENT, message));
        if self.cancel.is_cancelled() {
            return Err(Box::new(interrupted_output(SPAWN_AGENT)));
        }
        let SpawnInput { agent, task, label } = spawn::parse(&call.input).map_err(refuse)?;
        let spec = self.spec;
        let template = match &agent {
            None => None,
            Some(kind) => Some(spec.templates.iter().find(|t| t.name.eq_ignore_ascii_case(kind)).ok_or_else(|| {
                let kinds: Vec<&str> = spec.templates.iter().map(|t| t.name.as_str()).collect();
                let kinds = if kinds.is_empty() { "none".to_string() } else { kinds.join(", ") };
                refuse(format!(
                    "There is no sub-agent kind `{kind}`. Kinds you can create: {kinds}. Leave `agent` out for a \
                     general-purpose sub-agent."
                ))
            })?),
        };
        let policy = &spec.spawn;
        if self.spawned >= policy.per_run {
            return Err(refuse(format!(
                "The limit of {} per run is reached; this one was not created. Do the rest yourself.",
                base::fmt::plural(u64::from(policy.per_run), "sub-agent", "sub-agents")
            )));
        }
        let kind = template.map_or(GENERAL, |t| t.name.as_str());
        let of_kind = self.spawned_by_kind.get(kind).copied().unwrap_or(0);
        if let Some(template) = template
            && template.per_run > 0
            && of_kind >= template.per_run
        {
            return Err(refuse(format!(
                "A run may create {} of kind `{kind}` and that many were created; this one was not.",
                template.per_run
            )));
        }
        if *alive >= policy.alive_at_once {
            return Err(refuse(format!(
                "Only {} may run at the same time; this one was not started. Create it again once the others \
                 have answered.",
                base::fmt::plural(u64::from(policy.alive_at_once), "sub-agent", "sub-agents")
            )));
        }
        let depth = self.env.depth + 1;
        let name = label.unwrap_or_else(|| kind.to_string());
        let ceiling = policy.max_depth.max(DEPTH_CEILING);
        if depth > ceiling {
            return Err(refuse(format!(
                "Sub-agents cannot be nested deeper than {ceiling} levels; this one was not created. Do the task yourself."
            )));
        }
        if depth > policy.max_depth {
            let request = PermissionRequest {
                id: RequestId::new(base::id::new_id()),
                session: self.env.session.clone(),
                agent: spec.name.clone(),
                tool: SPAWN_AGENT.to_string(),
                call_id: Some(call.id.clone()),
                class: ToolClass::Internal,
                display: text_display(&format!("Create sub-agent “{name}” at depth {depth}"), &task),
                policy: format!("{SPAWN_AGENT} deeper than {} = ask", policy.max_depth),
                always_rule: None,
                at: self.now(),
            };
            self.ask(SPAWN_AGENT, request)?;
        }

        // What the child may spend: never more than is left above it.
        let remaining = self
            .remaining_budget()
            .map_err(|error| refuse(format!("The budget could not be read, so no sub-agent was created: {error:#}")))?;
        if remaining.is_some_and(|left| left <= 0.0) {
            return Err(refuse("The budget is spent; this sub-agent was not created.".to_string()));
        }
        let budget = child_budget(remaining, policy.budget_usd);
        let slot = self.tree.reserve().map_err(|full| {
            refuse(match full {
                Full::Created => format!(
                    "This run and its sub-agents have created {MAX_CREATED} sub-agents in all, which is the most a \
                     run may; this one was not created. Do the rest yourself."
                ),
                Full::Alive => format!(
                    "{MAX_ALIVE} sub-agents of this run are working right now, which is the most at one time; this \
                     one was not started. Create it again once some have answered, or do the task yourself."
                ),
            })
        })?;

        let template_model = template.and_then(|t| t.model.clone()).filter(|m| !m.is_empty());
        let child = ChildSession {
            name: name.clone(),
            task: task.clone(),
            model: template_model.unwrap_or_else(|| spec.model.clone()),
            depth,
            created_by: spec.runnable.clone(),
        };
        let session = match self.deps.host.create_child(&self.env.session, &child) {
            Ok(session) => session,
            Err(error) => {
                slot.cancel();
                return Err(refuse(format!("The sub-agent's session could not be created: {error:#}")));
            }
        };
        self.spawned += 1;
        *self.spawned_by_kind.entry(kind.to_string()).or_insert(0) += 1;
        *alive += 1;
        self.emit(AgentEvent::SubagentStarted { session: session.clone(), name: name.clone(), task: task.clone(), depth });
        let env = AgentEnv { session: session.clone(), depth, created_by: Some(spec.runnable.clone()), ..self.env.clone() };
        let spec = spawn::child_spec(spec, template, &name, &session, budget);
        Ok(Plan { spec, env, name, task, caps: self.caps.clone(), slot })
    }
}

/// Runs one sub-agent to its end and tells the parent's watchers how it went.
fn run_child(
    plan: &Plan<'_>,
    deps: &AgentDeps<'_>,
    sink: &dyn Emit,
    cancel: &CancelToken,
    parent: &SessionId,
) -> AgentOutcome {
    let lineage = Lineage { tree: plan.slot.tree(), caps: &plan.caps };
    let run = || run_shared(&plan.spec, &plan.env, deps, Input::Item(plan.task.clone()), sink, cancel, lineage);
    let outcome =
        catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|panic| crashed(Some(plan), deps, &panic_text(&*panic)));
    sink.emit(
        parent,
        AgentEvent::SubagentFinished {
            session: plan.env.session.clone(),
            summary: child_summary(&outcome),
            ok: outcome.is_finished(),
            tool_calls: outcome.totals.tool_calls,
            secs: u64::try_from(outcome.totals.duration_ms / 1000).unwrap_or(0),
            cost_usd: outcome.totals.cost_usd,
        },
    );
    outcome
}

/// The plan a started thread belongs to.
fn plan_of<'p, 't>(plans: &'p [Result<Plan<'t>, Refused>], at: usize) -> Option<&'p Plan<'t>> {
    plans.get(at).and_then(|plan| plan.as_ref().ok())
}

/// The outcome of a sub-agent whose run panicked. Its totals died with it, but what it spent
/// is in the ledger, and the parent's totals must still include it.
fn crashed(plan: Option<&Plan<'_>>, deps: &AgentDeps<'_>, message: &str) -> AgentOutcome {
    let cost_usd = plan.and_then(|plan| deps.host.spent(&plan.env.session).ok()).unwrap_or(0.0);
    AgentOutcome {
        stop: StopCause::Failed { error: format!("the sub-agent crashed ({message})") },
        text: String::new(),
        payload: None,
        totals: Totals { cost_usd, ..Totals::default() },
        files: Vec::new(),
    }
}

/// What the parent's model reads as the result: the child's last message, led by a line that
/// says so when the child did not finish.
fn child_summary(outcome: &AgentOutcome) -> String {
    let text = outcome.text.trim();
    let lead = match &outcome.stop {
        StopCause::Finished if text.is_empty() => return "(The sub-agent finished without a summary.)".to_string(),
        StopCause::Finished => return text.to_string(),
        StopCause::Waiting { question } => format!("The sub-agent stopped to ask: {question}"),
        StopCause::Cancelled => "The sub-agent was cancelled before it finished.".to_string(),
        StopCause::StepLimit => "The sub-agent reached its step limit before it finished.".to_string(),
        StopCause::Budget => "The sub-agent spent its budget before it finished.".to_string(),
        StopCause::Failed { error } => format!("The sub-agent failed: {error}"),
    };
    if text.is_empty() { format!("[{lead}]") } else { format!("[{lead} Its last message:]\n\n{text}") }
}
