//! Tasks you create yourself (Board / Timeline "New task") and AI-written task descriptions.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use harness_core::*;
use serde_json::json;

use crate::agent::AgentRun;
use crate::attachments;
use crate::chat::shell_for;
use crate::llm::{self, Msg};
use crate::prompt::{self, Stack};
use crate::tools::{self, ToolContext};
use crate::Engine;

/// Role of the general worker that runs a task without an agent.
pub const WORKER_ROLE: &str = "You are a capable general-purpose worker. You get one task from the user. \
If a working folder is attached, read the relevant files first, make focused changes in the existing style, and \
check your work (build/test as the project instructions say). If no folder is attached, use run_command to carry \
out and verify the task. Reply with a short report: what you did, which files changed or commands ran, how you \
checked it, anything left. Do not commit.";

/// What you enter in the "New task" dialog.
#[derive(Clone, Debug, Default)]
pub struct NewTask {
    pub title: String,
    pub description: String,
    pub run_id: Option<Id>,
    /// `None` = general worker.
    pub agent_id: Option<Id>,
    /// Empty = the agent's model, else the config default.
    pub model: String,
    pub cwd: Option<String>,
    /// Create it queued but don't start it.
    pub hold: bool,
    /// Screenshots the agent gets as images with the task.
    pub attachments: Vec<Attachment>,
}

/// Asks the fast model to write or revise a task description.
#[derive(Clone, Debug, Default)]
pub struct DraftRequest {
    pub title: String,
    pub run_id: Option<Id>,
    /// Existing description; may be empty.
    pub current: String,
    /// E.g. "shorter"; empty = write fresh.
    pub instruction: String,
    /// Screenshots attached to the task; the model looks at them.
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub text: String,
    pub model: String,
    pub cost_usd: f64,
}

/// Whether a task was created on hold (not started yet).
pub fn is_held(t: &Task) -> bool {
    t.output["held"].as_bool().unwrap_or(false)
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(n).collect();
        o.push('…');
        o
    }
}

/// Whether instructions sent to this task start another agent turn: it is finished, runs an
/// agent, and no newer attempt of the same step replaced it.
pub fn can_follow_up(store: &Store, t: &Task) -> bool {
    !t.status.is_active()
        && matches!(
            t.kind,
            TaskKind::Manual | TaskKind::Agent | TaskKind::Spawned
        )
        && store.newer_attempt(t).is_none()
}

/// The model-side history of a task's thread for a follow-up turn: everything before the
/// newest user message, plus that message as `(text, images)`.
///
/// Tool results are placed right after the assistant message that asked for them (a steering
/// message may sit between them in time). Calls without a stored result (an interrupted or
/// cancelled turn) are dropped, because gateways reject unanswered `tool_calls`. When the
/// thread does not start with a user turn (pipeline steps keep their first prompt out of the
/// thread), `fallback_first_user` takes its place so the history always opens with a user turn.
pub(crate) fn task_history(
    messages: &[Message],
    fallback_first_user: &str,
) -> (Vec<Msg>, String, Vec<llm::ImageData>) {
    let msgs: Vec<&Message> = messages.iter().filter(|m| !m.partial).collect();
    let last_user = msgs.iter().rposition(|m| m.role == Role::User);
    let (latest, latest_images) = match last_user {
        Some(i) => attachments::images_for(&msgs[i].attachments, msgs[i].content.clone()),
        None => (String::new(), vec![]),
    };
    let before = &msgs[..last_user.unwrap_or(msgs.len())];
    let results: HashMap<&str, &Message> = before
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, *m)))
        .collect();
    // Only an assistant turn without a user turn before it needs the fallback; a thread with
    // no user/assistant messages at all would otherwise get two user messages in a row.
    let starts_with_assistant = before
        .iter()
        .find(|m| matches!(m.role, Role::User | Role::Assistant))
        .is_some_and(|m| m.role == Role::Assistant);

    let mut out = vec![];
    if starts_with_assistant && !fallback_first_user.trim().is_empty() {
        out.push(Msg::User(fallback_first_user.trim().to_string()));
    }
    for m in before {
        match m.role {
            Role::User => {
                let (text, images) = attachments::images_for(&m.attachments, m.content.clone());
                if images.is_empty() {
                    if !text.trim().is_empty() {
                        out.push(Msg::User(text));
                    }
                } else {
                    out.push(Msg::UserImages { text, images });
                }
            }
            Role::Assistant => {
                let calls: Vec<llm::Call> = m
                    .tool_calls
                    .iter()
                    .filter(|c| results.contains_key(c.id.as_str()))
                    .map(|c| llm::Call {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        arguments: c.arguments.clone(),
                    })
                    .collect();
                if m.content.trim().is_empty() && calls.is_empty() {
                    continue;
                }
                out.push(Msg::Assistant {
                    content: m.content.clone(),
                    tool_calls: calls.clone(),
                });
                for c in &calls {
                    out.push(Msg::Tool {
                        call_id: c.id.clone(),
                        content: tools::truncate_middle(&results[c.id.as_str()].content, 20_000),
                    });
                }
            }
            Role::Tool | Role::System => {}
        }
    }
    (out, latest, latest_images)
}

/// The thread's messages, marking your undelivered ones as delivered: they are part of the
/// history now and must not reach the agent a second time as steering.
pub(crate) fn take_thread_messages(store: &Store, thread_id: &str) -> Vec<Message> {
    let mut msgs = store.messages(thread_id);
    for m in &mut msgs {
        if m.role == Role::User && !m.delivered {
            m.delivered = true;
            let _ = store.put_message(m);
        }
    }
    msgs
}

/// The shell wrapper and environment of the run's workdir that `cwd` belongs to (the only
/// workdir when there is exactly one), so follow-ups keep e.g. `CARGO_TARGET_DIR`.
/// Without a match the shell is the default for `cwd` and the environment is empty.
pub(crate) fn workdir_shell_and_env(
    run: Option<&Run>,
    cwd: Option<&Path>,
) -> (String, BTreeMap<String, String>) {
    if let (Some(run), Some(cwd)) = (run, cwd) {
        let workdirs = &run.graph.workdirs;
        let found = workdirs
            .iter()
            .find(|w| {
                run.worktrees
                    .get(&w.id)
                    .is_some_and(|p| cwd.starts_with(Path::new(p)))
                    || cwd.starts_with(harness_core::config::expand_tilde(&w.path))
            })
            .or_else(|| (workdirs.len() == 1).then(|| &workdirs[0]));
        if let Some(w) = found {
            let shell = if w.shell.is_empty() {
                shell_for(Some(cwd))
            } else {
                w.shell.clone()
            };
            return (shell, crate::pipeline::run_env(&w.env, run));
        }
    }
    (shell_for(cwd), BTreeMap::new())
}

/// "Run #N · title\nInput: ..." — what a task of a run should know about it.
fn run_context(run: &Run, limit: usize) -> String {
    let mut s = format!(
        "Run #{} · {}\nInput: {}",
        run.number,
        run.title,
        clip(&run.input, limit)
    );
    if let Some(r) = run.external_ref.as_deref().filter(|r| !r.is_empty()) {
        s.push_str(&format!("\nReference: {r}"));
    }
    s
}

/// Prompt for [`Engine::draft_description`].
pub fn description_messages(req: &DraftRequest, run: Option<&Run>) -> Vec<Msg> {
    let system = "You write task descriptions for an AI agent. Write: a one-line Goal, a short list of Steps, \
and a \"Done when\" line. Plain text or Markdown, under 150 words, no preamble and no closing remarks. \
If the user gives an instruction, revise the current description following it and return the whole new description.";
    let mut user = String::new();
    if req.title.trim().is_empty() {
        user.push_str("Task title: (none yet)\n");
    } else {
        user.push_str(&format!("Task title: {}\n", req.title.trim()));
    }
    if let Some(run) = run {
        user.push_str(&format!(
            "\nThis task belongs to a pipeline run.\n{}\n",
            run_context(run, 1500)
        ));
    }
    let current = req.current.trim();
    let instruction = req.instruction.trim();
    if !current.is_empty() {
        user.push_str(&format!("\nCurrent description:\n{current}\n"));
    }
    if !instruction.is_empty() {
        user.push_str(&format!("\nInstruction: {instruction}\n"));
    }
    if !req.attachments.is_empty() {
        user.push_str(&format!(
            "\n{} attached (below): use what they show.\n",
            if req.attachments.len() == 1 {
                "1 screenshot is".to_string()
            } else {
                format!("{} screenshots are", req.attachments.len())
            }
        ));
    }
    if current.is_empty() && instruction.is_empty() {
        user.push_str("\nWrite the description.");
    } else if !instruction.is_empty() {
        user.push_str("\nReturn the full revised description.");
    }
    let images: Vec<_> = req
        .attachments
        .iter()
        .filter_map(attachments::load)
        .collect();
    let user = if images.is_empty() {
        Msg::User(user)
    } else {
        Msg::UserImages { text: user, images }
    };
    vec![Msg::System(system.into()), user]
}

impl Engine {
    /// Creates a task on the Board. Unless `hold`, it starts right away.
    pub fn create_task(&self, n: NewTask) -> Result<Task> {
        let title = n.title.trim().to_string();
        if title.is_empty() {
            bail!("a task needs a title");
        }
        let store = self.store();
        let config = self.config();
        let agent = match n.agent_id.as_deref() {
            Some(id) => Some(store.agent(id).context("agent not found")?),
            None => None,
        };
        let mut t = Task::new(TaskKind::Manual, title.clone());
        t.run_id = n.run_id.clone();
        t.actor = agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Worker".into());
        t.model = if !n.model.is_empty() {
            n.model.clone()
        } else if let Some(a) = &agent {
            a.model.clone()
        } else {
            config.default_model.clone()
        };
        t.cwd = n.cwd.clone().filter(|c| !c.trim().is_empty());
        t.status = Status::Queued;
        t.priority = store
            .tasks()
            .iter()
            .filter(|t| t.status.is_active())
            .map(|t| t.priority)
            .min()
            .map(|p| p - 1)
            .unwrap_or(0);
        let description = n.description.trim().to_string();
        t.output = json!({"description": description, "held": n.hold});
        store.put_task(&t)?;
        let text = if description.is_empty() {
            title.clone()
        } else {
            description
        };
        let mut m = Message::new(&t.id, Role::User, text);
        m.author = "you".into();
        m.delivered = true;
        m.attachments = n.attachments.clone();
        store.put_message(&m)?;
        store.log(
            "task.created",
            t.run_id.as_deref(),
            Some(&t.id),
            format!("You created {title}"),
        );
        if !n.hold {
            self.start_task(&t.id)?;
        }
        Ok(store.task(&t.id).unwrap_or(t))
    }

    /// Starts a queued task you created (clears its hold). It runs in the background.
    pub fn start_task(&self, id: &str) -> Result<()> {
        let store = self.store().clone();
        let t = store.task(id).context("task not found")?;
        if t.kind != TaskKind::Manual {
            bail!("only tasks you created can be started here");
        }
        if t.status != Status::Queued {
            bail!("the task is not queued");
        }
        // Already starting in this process (waiting for a slot).
        if self.cancels.lock().contains_key(id) {
            return Ok(());
        }
        let cancel = self.cancel_flag(id);
        store.update_task(id, |t| {
            if t.output.is_object() {
                t.output["held"] = json!(false);
            } else {
                t.output = json!({"held": false});
            }
        })?;
        let engine = self.clone();
        let task_id = id.to_string();
        let spawned = std::thread::Builder::new()
            .name(format!("task-{id}"))
            .spawn(move || engine.run_manual_task(&task_id, cancel));
        if let Err(e) = spawned {
            self.drop_cancel_flag(id);
            bail!("could not start the task: {e}");
        }
        Ok(())
    }

    fn run_manual_task(&self, id: &str, cancel: Arc<AtomicBool>) {
        let store = self.store().clone();
        let _slot = self.acquire_slot();
        let started = store.update_task(id, |t| {
            if t.status == Status::Queued && !cancel.load(Ordering::SeqCst) {
                t.status = Status::Running;
                t.started_at = Some(now());
            }
        });
        let Ok(task) = started else {
            self.drop_cancel_flag(id);
            return;
        };
        if task.status != Status::Running {
            self.drop_cancel_flag(id);
            return;
        }
        let res = self.run_manual_agent(&task, cancel);
        self.drop_cancel_flag(id);
        let (status, summary, error) = match res {
            Ok(text) => (Status::Done, clip(&text, 200), None),
            Err(e) => (Status::Failed, String::new(), Some(format!("{e:#}"))),
        };
        let _ = store.update_task(id, |t| {
            if t.status == Status::Cancelled {
                return;
            }
            t.status = status;
            if status == Status::Done {
                t.summary = summary.clone();
            }
            t.error = error.clone();
            t.finished_at = Some(now());
        });
        match status {
            Status::Done => store.log(
                "task.done",
                task.run_id.as_deref(),
                Some(id),
                format!("{} finished", task.title),
            ),
            _ => store.log(
                "task.failed",
                task.run_id.as_deref(),
                Some(id),
                format!("{} failed", task.title),
            ),
        }
    }

    fn run_manual_agent(&self, task: &Task, cancel: Arc<AtomicBool>) -> Result<String> {
        let store = self.store();
        let config = self.config();
        let first = take_thread_messages(store, &task.id)
            .into_iter()
            .find(|m| m.role == Role::User);
        // Screenshots sent with the task go with its first turn.
        let (message, images) = match first {
            Some(m) => attachments::images_for(&m.attachments, m.content),
            None => (task.title.clone(), vec![]),
        };
        let agent = task
            .actor
            .as_str()
            .ne("Worker")
            .then(|| store.agent_by_name(&task.actor))
            .flatten();
        let run = task.run_id.as_deref().and_then(|r| store.run(r));
        let cwd = task
            .cwd
            .as_deref()
            .map(harness_core::config::expand_tilde)
            .filter(|p| p.is_dir());
        let (shell, env) = workdir_shell_and_env(run.as_ref(), cwd.as_deref());
        let access = agent.as_ref().map(|a| a.tools).unwrap_or(ToolAccess::Full);
        let skills_on_demand = agent
            .as_ref()
            .map(|a| a.skills_on_demand.clone())
            .unwrap_or_default();
        let model = if task.model.is_empty() {
            config.default_model.clone()
        } else {
            task.model.clone()
        };
        let agent_run = AgentRun {
            engine: self,
            task_id: task.id.clone(),
            thread_id: task.id.clone(),
            actor: task.actor.clone(),
            model,
            stack: Stack {
                rules: config.rules.clone(),
                role: agent
                    .as_ref()
                    .map(|a| a.role.clone())
                    .unwrap_or_else(|| WORKER_ROLE.into()),
                context: run
                    .as_ref()
                    .map(|r| run_context(r, 1500))
                    .unwrap_or_default(),
                environment: prompt::environment(cwd.as_deref(), &shell, access, &[]),
                skills: agent.as_ref().map(|a| a.skills.clone()).unwrap_or_default(),
                skills_on_demand: skills_on_demand.clone(),
                message,
                images,
                ..Default::default()
            },
            tools: ToolContext {
                cwd,
                shell,
                env,
                access,
                workdirs: vec![],
                skills_on_demand,
                config: config.clone(),
                cancel,
            },
            extra_tools: vec![],
            history: vec![],
            max_steps: agent.as_ref().map(|a| a.max_steps).unwrap_or(60),
        };
        Ok(agent_run.run(&mut |_, _, _| None)?.text)
    }

    /// Starts another agent turn on a finished task: stores your instructions, marks the
    /// task running right away and continues the conversation in the background.
    pub(crate) fn start_followup(&self, task: &Task, text: &str) -> Result<()> {
        if !matches!(
            task.kind,
            TaskKind::Manual | TaskKind::Agent | TaskKind::Spawned
        ) {
            bail!("this step does not run an agent");
        }
        let store = self.store().clone();
        if store.newer_attempt(task).is_some() {
            bail!("a newer attempt replaced this task; send instructions there");
        }
        let id = task.id.clone();
        let mut m = Message::new(&id, Role::User, text);
        m.author = "you".into();
        m.delivered = true;
        store.put_message(&m)?;
        let cancel = self.cancel_flag(&id);
        // Running at once: the UI shows the turn and a quick second message steers it.
        // Marked as a live follow-up so `close_orphans` leaves it be while the run is over.
        self.mark_followup(&id);
        let updated = store.update_task(&id, |t| {
            t.status = Status::Running;
            t.started_at.get_or_insert_with(now);
            t.finished_at = None;
            t.error = None;
        });
        if let Err(e) = updated {
            self.drop_followup(&id);
            self.drop_cancel_flag(&id);
            return Err(e);
        }
        store.log(
            "task.followup",
            task.run_id.as_deref(),
            Some(&id),
            format!("You followed up on {}", task.title),
        );
        let engine = self.clone();
        let tid = id.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("followup-{id}"))
            .spawn(move || engine.run_followup(&tid, cancel));
        if let Err(e) = spawned {
            let _ = store.update_task(&id, |t| {
                t.status = Status::Failed;
                t.error = Some(format!("could not start the follow-up: {e}"));
                t.finished_at = Some(now());
            });
            self.drop_followup(&id);
            self.drop_cancel_flag(&id);
            bail!("could not start the follow-up: {e}");
        }
        Ok(())
    }

    fn run_followup(&self, id: &str, cancel: Arc<AtomicBool>) {
        /// Releases the follow-up marker and cancel flag on every way out, panics included.
        struct Cleanup<'a>(&'a Engine, &'a str);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.drop_followup(self.1);
                self.0.drop_cancel_flag(self.1);
            }
        }
        self.mark_followup(id);
        let _cleanup = Cleanup(self, id);
        let store = self.store().clone();
        let _slot = self.acquire_slot();
        let task = match store.task(id) {
            Some(t) if !cancel.load(Ordering::SeqCst) => t,
            _ => {
                let _ = store.update_task(id, |t| {
                    if t.status.is_active() {
                        t.status = Status::Cancelled;
                        t.finished_at = Some(now());
                    }
                });
                return;
            }
        };
        let res = self.run_followup_turn(&task, cancel.clone());
        let cancelled = cancel.load(Ordering::SeqCst);
        let (status, summary, error) = match res {
            Ok(text) => {
                let summary = match task.kind {
                    TaskKind::Agent => text,
                    TaskKind::Spawned => text.chars().take(600).collect(),
                    _ => clip(&text, 200),
                };
                (Status::Done, summary, None)
            }
            Err(_) if cancelled => (Status::Cancelled, String::new(), None),
            Err(e) => (Status::Failed, String::new(), Some(format!("{e:#}"))),
        };
        let _ = store.update_task(id, |t| {
            if t.status == Status::Cancelled {
                return;
            }
            t.status = status;
            if status == Status::Done {
                t.summary = summary.clone();
            }
            t.error = error.clone();
            t.finished_at = Some(now());
        });
        match status {
            Status::Done => store.log(
                "task.done",
                task.run_id.as_deref(),
                Some(id),
                format!("{} finished", task.title),
            ),
            Status::Failed => store.log(
                "task.failed",
                task.run_id.as_deref(),
                Some(id),
                format!("{} failed", task.title),
            ),
            _ => {}
        }
    }

    /// One follow-up turn; returns the agent's reply.
    fn run_followup_turn(&self, task: &Task, cancel: Arc<AtomicBool>) -> Result<String> {
        if task.kind == TaskKind::Agent {
            if let Some(res) = crate::pipeline::followup_agent(self, task, cancel.clone()) {
                return res;
            }
        }
        let store = self.store();
        let config = self.config();
        let agent = task
            .actor
            .as_str()
            .ne("Worker")
            .then(|| store.agent_by_name(&task.actor))
            .flatten();
        let run = task.run_id.as_deref().and_then(|r| store.run(r));
        let cwd = task
            .cwd
            .as_deref()
            .map(harness_core::config::expand_tilde)
            .filter(|p| p.is_dir());
        let (shell, env) = workdir_shell_and_env(run.as_ref(), cwd.as_deref());
        let access = agent.as_ref().map(|a| a.tools).unwrap_or(ToolAccess::Full);
        let skills_on_demand = agent
            .as_ref()
            .map(|a| a.skills_on_demand.clone())
            .unwrap_or_default();
        let model = if task.model.is_empty() {
            config.default_model.clone()
        } else {
            task.model.clone()
        };
        let default_role = if task.kind == TaskKind::Spawned {
            crate::chat::DELEGATE_ROLE
        } else {
            WORKER_ROLE
        };
        let msgs = take_thread_messages(store, &task.id);
        let (history, mut latest, images) = task_history(&msgs, &task.title);
        if latest.trim().is_empty() && !images.is_empty() {
            latest = "See the attached image.".into();
        }
        let agent_run = AgentRun {
            engine: self,
            task_id: task.id.clone(),
            thread_id: task.id.clone(),
            actor: task.actor.clone(),
            model,
            stack: Stack {
                rules: config.rules.clone(),
                role: agent
                    .as_ref()
                    .map(|a| a.role.clone())
                    .unwrap_or_else(|| default_role.into()),
                // In the system prompt, so the first turn is just your message.
                instructions: run
                    .as_ref()
                    .map(|r| run_context(r, 1500))
                    .unwrap_or_default(),
                context: String::new(),
                environment: prompt::environment(cwd.as_deref(), &shell, access, &[]),
                skills: agent.as_ref().map(|a| a.skills.clone()).unwrap_or_default(),
                skills_on_demand: skills_on_demand.clone(),
                message: latest,
                images,
            },
            tools: ToolContext {
                cwd,
                shell,
                env,
                access,
                workdirs: vec![],
                skills_on_demand,
                config: config.clone(),
                cancel,
            },
            extra_tools: vec![],
            history,
            max_steps: agent.as_ref().map(|a| a.max_steps).unwrap_or(60),
        };
        Ok(agent_run.run(&mut |_, _, _| None)?.text)
    }

    /// Has the fast model write (or revise) a task description. `on_text` receives the
    /// text written so far (the whole draft, not just the new piece) as it streams.
    /// When cancelled, returns what was written up to then.
    pub fn draft_description(
        &self,
        req: DraftRequest,
        cancel: Arc<AtomicBool>,
        mut on_text: impl FnMut(&str),
    ) -> Result<Draft> {
        if req.title.trim().is_empty() && req.current.trim().is_empty() {
            bail!("give the task a title first");
        }
        let config = self.config();
        let model = config.fast().to_string();
        let (provider, model_id) = config
            .resolve(&model)
            .map(|(p, m)| (p.clone(), m))
            .context("no model provider configured (Settings › Models)")?;
        let run = req.run_id.as_deref().and_then(|r| self.store().run(r));
        let msgs = description_messages(&req, run.as_ref());
        let mut text = String::new();
        let cancel_fn = || cancel.load(Ordering::SeqCst);
        let reply = llm::complete(
            &provider,
            &model_id,
            &msgs,
            &[],
            Some(700),
            &cancel_fn,
            &mut |d| {
                text.push_str(d);
                on_text(&text);
            },
        );
        let (text, cost) = match reply {
            Ok(r) => {
                let cost = config.cost(&model, r.tokens_in, r.tokens_out);
                let t = if r.content.trim().is_empty() {
                    text
                } else {
                    r.content
                };
                (t, cost)
            }
            Err(_) if cancel.load(Ordering::SeqCst) => (text, 0.0),
            Err(e) => return Err(e),
        };
        self.store().log(
            "task.draft",
            req.run_id.as_deref(),
            None,
            format!("AI drafted a description · ${cost:.3}"),
        );
        Ok(Draft {
            text: text.trim().to_string(),
            model,
            cost_usd: cost,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        let g = PipelineGraph {
            workdirs: vec![],
            nodes: vec![],
            edges: vec![],
        };
        Run {
            id: "r1".into(),
            number: 12,
            pipeline_id: "p".into(),
            pipeline_version: 1,
            graph: g,
            title: "Fix login bug".into(),
            input: "Users cannot log in".into(),
            attachments: vec![],
            external_ref: Some("GH-42".into()),
            status: Status::Running,
            chat_id: None,
            outputs: Default::default(),
            frontier: vec![],
            heads: Default::default(),
            worktrees: Default::default(),
            notes: Default::default(),
            pr_url: None,
            cost_usd: 0.0,
            error: None,
            created_at: now(),
            finished_at: None,
        }
    }

    fn user_text(m: &[Msg]) -> String {
        match &m[1] {
            Msg::User(s) => s.clone(),
            Msg::UserImages { text, .. } => text.clone(),
            _ => panic!("expected a user message"),
        }
    }

    #[test]
    fn prompt_with_screenshots_sends_them_as_images() {
        let dir = std::env::temp_dir().join(format!("pipeliner-draft-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let shot = |p: &Path| Attachment {
            path: p.display().to_string(),
            name: "shot.png".into(),
            mime: "image/png".into(),
            width: 10,
            height: 10,
            pasted: true,
        };
        let req = DraftRequest {
            title: "Fix the chart".into(),
            attachments: vec![shot(&path), shot(&dir.join("gone.png"))],
            ..Default::default()
        };
        let m = description_messages(&req, None);
        match &m[1] {
            Msg::UserImages { text, images } => {
                assert!(text.contains("2 screenshots are attached"), "{text}");
                // The missing file is skipped, the other one is sent.
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].mime, "image/png");
            }
            _ => panic!("expected images"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prompt_fresh() {
        let req = DraftRequest {
            title: "Add dark mode".into(),
            ..Default::default()
        };
        let m = description_messages(&req, None);
        assert_eq!(m.len(), 2);
        assert!(matches!(&m[0], Msg::System(s) if s.contains("Done when")));
        let u = user_text(&m);
        assert!(u.contains("Add dark mode"));
        assert!(!u.contains("Instruction:"));
        assert!(!u.contains("Current description"));
        assert!(!u.contains("Run #"));
    }

    #[test]
    fn prompt_with_run_and_revision() {
        let req = DraftRequest {
            title: "Add dark mode".into(),
            run_id: Some("r1".into()),
            current: "Goal: dark mode".into(),
            instruction: "shorter".into(),
            ..Default::default()
        };
        let u = user_text(&description_messages(&req, Some(&run())));
        assert!(u.contains("Run #12"));
        assert!(u.contains("Fix login bug"));
        assert!(u.contains("Users cannot log in"));
        assert!(u.contains("GH-42"));
        assert!(u.contains("Goal: dark mode"));
        assert!(u.contains("Instruction: shorter"));
    }

    #[test]
    fn prompt_truncates_run_input() {
        let mut r = run();
        r.input = "x".repeat(5000);
        let req = DraftRequest {
            title: "t".into(),
            ..Default::default()
        };
        let u = user_text(&description_messages(&req, Some(&r)));
        assert!(u.len() < 2000);
    }

    fn msg(role: Role, content: &str, secs: i64) -> Message {
        let mut m = Message::new("t", role, content);
        m.created_at = now() + chrono::Duration::seconds(secs);
        m
    }

    fn asst_call(content: &str, call: &str, secs: i64) -> Message {
        let mut m = msg(Role::Assistant, content, secs);
        m.tool_calls = vec![ToolCall {
            id: call.into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        }];
        m
    }

    fn tool(call: &str, content: &str, secs: i64) -> Message {
        let mut m = msg(Role::Tool, content, secs);
        m.tool_call_id = Some(call.into());
        m
    }

    #[test]
    fn history_keeps_tool_results_next_to_their_calls() {
        let messages = vec![
            msg(Role::User, "first", 0),
            asst_call("looking", "c1", 1),
            // You steered while the tool was running; its result lands after your message.
            msg(Role::User, "also check X", 2),
            tool("c1", "file contents", 3),
            msg(Role::Assistant, "done", 4),
            msg(Role::User, "now do Y", 5),
        ];
        let (h, latest, images) = task_history(&messages, "fallback");
        assert_eq!(latest, "now do Y");
        assert!(images.is_empty());
        assert!(matches!(&h[0], Msg::User(t) if t == "first"));
        assert!(matches!(&h[1], Msg::Assistant { tool_calls, .. } if tool_calls.len() == 1));
        assert!(
            matches!(&h[2], Msg::Tool { call_id, content } if call_id == "c1" && content == "file contents")
        );
        assert!(matches!(&h[3], Msg::User(t) if t == "also check X"));
        assert!(matches!(&h[4], Msg::Assistant { content, .. } if content == "done"));
        assert_eq!(h.len(), 5);
    }

    #[test]
    fn history_drops_unanswered_tool_calls_and_partials() {
        let mut partial = msg(Role::Assistant, "streaming", 3);
        partial.partial = true;
        let messages = vec![
            msg(Role::User, "first", 0),
            asst_call("", "c1", 1),
            // cancelled before the tool answered
            msg(Role::System, "cancelled", 2),
            partial,
            msg(Role::User, "retry", 4),
        ];
        let (h, latest, _) = task_history(&messages, "fallback");
        assert_eq!(latest, "retry");
        assert_eq!(h.len(), 1);
        assert!(matches!(&h[0], Msg::User(t) if t == "first"));
    }

    #[test]
    fn history_starts_with_the_fallback_prompt_when_the_thread_does_not() {
        let messages = vec![
            msg(Role::System, "Step started", 0),
            msg(Role::Assistant, "Reading first.", 1),
            msg(Role::User, "change the colour", 2),
        ];
        let (h, latest, _) = task_history(&messages, "  # Task\nbuild it  ");
        assert_eq!(latest, "change the colour");
        assert!(matches!(&h[0], Msg::User(t) if t == "# Task\nbuild it"));
        assert!(matches!(&h[1], Msg::Assistant { content, .. } if content == "Reading first."));
        // Without a fallback there is nothing to put first.
        let (h, _, _) = task_history(&messages, " ");
        assert!(matches!(&h[0], Msg::Assistant { .. }));
        // A thread that already starts with you needs no fallback.
        let own = vec![msg(Role::User, "a", 0), msg(Role::User, "b", 1)];
        let (h, latest, _) = task_history(&own, "fallback");
        assert_eq!(latest, "b");
        assert_eq!(h.len(), 1);
        assert!(matches!(&h[0], Msg::User(t) if t == "a"));
    }

    #[test]
    fn history_without_user_or_assistant_before_gets_no_fallback() {
        // A step that failed before its first assistant turn: only a system note precedes.
        let messages = vec![
            msg(Role::System, "Step started", 0),
            msg(Role::User, "try again", 1),
        ];
        let (h, latest, _) = task_history(&messages, "# Task\nbuild it");
        assert_eq!(latest, "try again");
        assert!(h.is_empty(), "{h:?}");
    }

    #[test]
    fn take_thread_messages_marks_user_messages_delivered() {
        let store = Store::open_in_memory().unwrap();
        let mut first = Message::new("t1", Role::User, "first");
        first.delivered = false;
        let mut note = Message::new("t1", Role::Assistant, "ok");
        note.created_at = first.created_at + chrono::Duration::seconds(1);
        store.put_message(&first).unwrap();
        store.put_message(&note).unwrap();

        let msgs = take_thread_messages(&store, "t1");
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().all(|m| m.role != Role::User || m.delivered));
        assert!(store
            .messages("t1")
            .iter()
            .filter(|m| m.role == Role::User)
            .all(|m| m.delivered));

        // A message that arrives afterwards is still steering for the running turn.
        let mut late = Message::new("t1", Role::User, "late");
        late.delivered = false;
        late.created_at = first.created_at + chrono::Duration::seconds(2);
        store.put_message(&late).unwrap();
        let pending = store.take_undelivered("t1");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "late");
    }

    #[test]
    fn workdir_env_expands_run_placeholders() {
        let mut r = run();
        r.number = 7;
        r.graph.workdirs = vec![Workdir {
            id: "w1".into(),
            name: "app".into(),
            path: "/definitely/not/here/app".into(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: String::new(),
            setup: String::new(),
            shell: "nix-shell --run".into(),
            env: BTreeMap::from([
                (
                    "CARGO_TARGET_DIR".to_string(),
                    "{run_cache}/target".to_string(),
                ),
                ("RUN_NO".to_string(), "run-{run}".to_string()),
            ]),
        }];
        r.worktrees
            .insert("w1".into(), "/definitely/not/here/wt/w1".into());
        let cwd = Path::new("/definitely/not/here/wt/w1/crates");
        let (shell, env) = workdir_shell_and_env(Some(&r), Some(cwd));
        assert_eq!(shell, "nix-shell --run");
        let cache = harness_core::paths::run_cache_dir(7).display().to_string();
        assert_eq!(env["CARGO_TARGET_DIR"], format!("{cache}/target"));
        assert_eq!(env["RUN_NO"], "run-7");
        // A single workdir is used even when the folder is somewhere else.
        let (shell, env) = workdir_shell_and_env(Some(&r), Some(Path::new("/elsewhere")));
        assert_eq!(shell, "nix-shell --run");
        assert_eq!(env["RUN_NO"], "run-7");
        // No run: nothing to add.
        let (_, env) = workdir_shell_and_env(None, Some(cwd));
        assert!(env.is_empty());
    }

    #[test]
    fn history_without_a_user_message_is_everything_and_empty_latest() {
        let messages = vec![msg(Role::Assistant, "hi", 0)];
        let (h, latest, _) = task_history(&messages, "");
        assert_eq!(latest, "");
        assert_eq!(h.len(), 1);
    }

    fn engine() -> Engine {
        Engine::new(Store::open_in_memory().unwrap(), Config::default())
    }

    #[test]
    fn create_held_task() {
        let e = engine();
        let mut old = Task::new(TaskKind::Agent, "old");
        old.priority = 5;
        e.store().put_task(&old).unwrap();
        let t = e
            .create_task(NewTask {
                title: "  Write docs ".into(),
                description: "Document the CLI".into(),
                hold: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(t.kind, TaskKind::Manual);
        assert_eq!(t.title, "Write docs");
        assert_eq!(t.status, Status::Queued);
        assert_eq!(t.actor, "Worker");
        assert!(t.priority < 5);
        assert!(is_held(&t));
        let msgs = e.store().messages(&t.id);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Document the CLI");
        assert_eq!(msgs[0].author, "you");
        assert!(msgs[0].delivered);
        // Still queued: nothing started it.
        assert_eq!(e.store().task(&t.id).unwrap().status, Status::Queued);
    }

    #[test]
    fn empty_description_uses_title_and_empty_title_fails() {
        let e = engine();
        let t = e
            .create_task(NewTask {
                title: "Do it".into(),
                hold: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(e.store().messages(&t.id)[0].content, "Do it");
        assert!(e
            .create_task(NewTask {
                title: "   ".into(),
                hold: true,
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn start_rejects_other_kinds() {
        let e = engine();
        let t = Task::new(TaskKind::Agent, "x");
        e.store().put_task(&t).unwrap();
        assert!(e.start_task(&t.id).is_err());
    }
}
