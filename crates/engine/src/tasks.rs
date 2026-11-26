//! Tasks you create yourself (Board / Timeline "New task") and AI-written task descriptions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use harness_core::*;
use serde_json::json;

use crate::agent::AgentRun;
use crate::chat::shell_for;
use crate::llm::{self, Msg};
use crate::prompt::{self, Stack};
use crate::tools::ToolContext;
use crate::Engine;

/// Role of the general worker that runs a task without an agent.
pub const WORKER_ROLE: &str = "You are a capable general-purpose worker. You get one task from the user. \
Read the relevant files first, make focused changes in the existing style, check your work (build/test as the \
project instructions say), and reply with a short report: what you did, which files changed, how you checked it, \
anything left. Do not commit.";

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
    if current.is_empty() && instruction.is_empty() {
        user.push_str("\nWrite the description.");
    } else if !instruction.is_empty() {
        user.push_str("\nReturn the full revised description.");
    }
    vec![Msg::System(system.into()), Msg::User(user)]
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
        let message = store
            .messages(&task.id)
            .into_iter()
            .find(|m| m.role == Role::User)
            .map(|m| m.content)
            .unwrap_or_else(|| task.title.clone());
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
        let shell = shell_for(cwd.as_deref());
        let access = match (&cwd, &agent) {
            (None, _) => ToolAccess::None,
            (Some(_), Some(a)) => a.tools,
            (Some(_), None) => ToolAccess::Full,
        };
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
                environment: prompt::environment(cwd.as_deref(), &shell, &[]),
                skills: agent.as_ref().map(|a| a.skills.clone()).unwrap_or_default(),
                skills_on_demand: skills_on_demand.clone(),
                message,
                ..Default::default()
            },
            tools: ToolContext {
                cwd,
                shell,
                env: Default::default(),
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
            _ => panic!("expected a user message"),
        }
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
