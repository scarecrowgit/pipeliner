//! Pipeliner engine: model calls, agent runtime with tools, pipeline executor,
//! git worktrees, rendering and pull requests. No UI code; the app drives it
//! through [`Engine`] and observes it through the [`Store`]'s change feed.

pub mod agent;
pub mod attachments;
pub mod chat;
pub mod git;
pub mod llm;
pub mod pipeline;
pub mod prompt;
pub mod render;
pub mod templates;
pub mod tools;
pub mod update;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use harness_core::*;
use parking_lot::{Mutex, RwLock};

pub use templates::Template;

/// Shared handle to the engine. Cheap to clone.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

pub struct Inner {
    pub store: Arc<Store>,
    config: RwLock<Config>,
    /// Cancel flags of running tasks.
    cancels: Mutex<HashMap<Id, Arc<AtomicBool>>>,
    /// Limits concurrently running agent tasks.
    slots: Arc<Semaphore>,
    /// Runs being executed by a thread in this process.
    active_runs: Mutex<std::collections::HashSet<Id>>,
    /// This process holds the engine lock and executes runs.
    owner: AtomicBool,
    lock_file: Mutex<Option<std::fs::File>>,
}

impl std::ops::Deref for Engine {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Engine {
    pub fn new(store: Arc<Store>, config: Config) -> Engine {
        let slots = config.parallel_slots.max(1) as usize;
        Engine(Arc::new(Inner {
            store,
            config: RwLock::new(config),
            cancels: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(slots)),
            active_runs: Mutex::new(Default::default()),
            owner: AtomicBool::new(false),
            lock_file: Mutex::new(None),
        }))
    }

    /// Resumes runs that were active when the app last quit.
    /// Interrupted agent tasks are marked failed (retry them from the Board or thread).
    /// Becomes the engine owner for this data folder (or waits to become it) and resumes runs.
    ///
    /// Only one process at a time executes pipeline runs: the one holding
    /// `<data dir>/engine.lock`. Others (a second window, the CLI while the app is open)
    /// still create runs and record decisions in the shared database; the owner picks
    /// them up within a couple of seconds. When the owner exits, another process takes over.
    pub fn start(&self) {
        if self.try_become_owner() {
            self.recover();
        }
        let engine = self.clone();
        std::thread::Builder::new()
            .name("engine-owner".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if !engine.is_owner() {
                    if engine.try_become_owner() {
                        engine.recover();
                    }
                    continue;
                }
                // Runs created or resumed by other processes.
                for r in engine.store.runs() {
                    if r.status == Status::Running || r.status == Status::Waiting {
                        engine.spawn_run_thread(r.id.clone());
                    }
                }
            })
            .ok();
    }

    /// Whether this process executes pipeline runs.
    pub fn is_owner(&self) -> bool {
        self.owner.load(Ordering::SeqCst)
    }

    fn try_become_owner(&self) -> bool {
        if self.is_owner() {
            return true;
        }
        let Some(dir) = self.store.dir() else {
            // In-memory store (tests): nothing to share.
            self.owner.store(true, Ordering::SeqCst);
            return true;
        };
        let _ = std::fs::create_dir_all(&dir);
        let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("engine.lock"))
        else {
            return false;
        };
        if f.try_lock().is_ok() {
            *self.lock_file.lock() = Some(f);
            self.owner.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// After taking ownership: steps that were running in a process that is gone are
    /// marked interrupted; active runs are resumed (their frontier re-executes).
    fn recover(&self) {
        let local: std::collections::HashSet<Id> = self.cancels.lock().keys().cloned().collect();
        for t in self.store.tasks() {
            if !t.status.is_active() || local.contains(&t.id) {
                continue;
            }
            // Gates of active runs are resumed by their run thread.
            let run_active = t
                .run_id
                .as_deref()
                .and_then(|r| self.store.run(r))
                .map(|r| r.status.is_active())
                .unwrap_or(false);
            if t.kind == TaskKind::Gate && run_active {
                continue;
            }
            let _ = self.store.update_task(&t.id, |t| {
                t.status = Status::Failed;
                t.error = Some("Interrupted: the app was closed while this ran.".into());
                t.finished_at = Some(now());
            });
        }
        for r in self.store.runs() {
            if r.status.is_active() {
                self.spawn_run_thread(r.id.clone());
            }
        }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn config(&self) -> Config {
        self.config.read().clone()
    }

    /// Replaces the config, saves it to disk and notifies views.
    pub fn set_config(&self, c: Config) -> Result<()> {
        c.save()?;
        *self.config.write() = c;
        self.store.emit(Change::Config);
        Ok(())
    }

    /// Models for pickers: config list, else agents' and default models.
    pub fn models(&self) -> Vec<String> {
        let c = self.config();
        let mut v: Vec<String> = c.models.clone();
        if v.is_empty() {
            v.push(c.default_model.clone());
            for p in &c.providers {
                for m in llm::list_models(p).unwrap_or_default() {
                    v.push(format!("{}/{}", p.id, m));
                }
            }
            for a in self.store.agents() {
                v.push(a.model);
            }
        }
        v.retain(|m| !m.is_empty());
        v.sort();
        v.dedup();
        v
    }

    pub fn skills(&self) -> Vec<harness_core::skills::Skill> {
        harness_core::skills::list(&self.config())
    }

    // ---- chats ---------------------------------------------------------------

    pub fn new_chat(&self, mode: ChatMode) -> Result<Chat> {
        let c = Chat {
            id: new_id(),
            title: "New chat".into(),
            mode,
            model: self.config().default_model,
            agent_id: None,
            pipeline_id: None,
            cwd: None,
            task_id: None,
            archived: false,
            created_at: now(),
            updated_at: now(),
        };
        self.store.put_chat(&c)?;
        self.store.log(
            "chat.created",
            None,
            None,
            format!("New {} chat", mode.label()),
        );
        Ok(c)
    }

    /// Sends your message. Direct: the model replies (streamed). Agent/Orchestrator: the
    /// agent works with tools as a task. Pipeline: starts the chat's pipeline with the text.
    /// If the chat's agent is mid-task, the message steers it instead.
    pub fn send(&self, chat_id: &str, text: &str) -> Result<()> {
        self.send_with(chat_id, text, vec![])
    }

    /// Like [`Engine::send`], with images attached (the text may be empty then).
    pub fn send_with(&self, chat_id: &str, text: &str, attachments: Vec<Attachment>) -> Result<()> {
        chat::send(self, chat_id, text, attachments)
    }

    /// Stores image bytes (pasted or dropped) as an attachment.
    pub fn save_attachment_bytes(
        &self,
        bytes: &[u8],
        name: Option<&str>,
        pasted: bool,
    ) -> Result<Attachment> {
        attachments::save_bytes(bytes, name, pasted)
    }

    /// Copies an image file into the attachments folder.
    pub fn add_attachment_file(&self, path: &std::path::Path) -> Result<Attachment> {
        attachments::add_file(path)
    }

    /// Deletes an attachment that was never sent (only inside the attachments folder).
    pub fn discard_attachment(&self, path: &str) {
        attachments::discard(path)
    }

    // ---- runs ----------------------------------------------------------------

    /// Starts a pipeline run with `input` (issue text) as its source item.
    pub fn start_run(&self, pipeline_id: &str, input: &str, chat_id: Option<&str>) -> Result<Run> {
        pipeline::start(self, pipeline_id, input, chat_id)
    }

    pub fn cancel_run(&self, run_id: &str) -> Result<()> {
        // Mark the run first so its thread stops scheduling new steps.
        self.store.update_run(run_id, |r| {
            if r.status.is_active() {
                r.status = Status::Cancelled;
                r.finished_at = Some(now());
            }
        })?;
        for t in self.store.tasks_for_run(run_id) {
            if t.status.is_active() {
                self.cancel_task(&t.id)?;
            }
        }
        self.store
            .log("run.cancelled", Some(run_id), None, "Run cancelled");
        Ok(())
    }

    /// Re-runs a failed or cancelled pipeline step (and continues the run from there).
    pub fn retry(&self, task_id: &str) -> Result<()> {
        let t = self.store.task(task_id).context("task not found")?;
        let (Some(run_id), Some(node_id)) = (t.run_id.clone(), t.node_id.clone()) else {
            bail!("only pipeline steps can be retried");
        };
        if t.kind == TaskKind::Spawned {
            bail!("a sub-agent can't be retried on its own; retry the step that spawned it");
        }
        if let Some(n) = self.store.newer_attempt(&t) {
            bail!(
                "this step already has a newer attempt ({}); open that one instead",
                n.status.as_str()
            );
        }
        let run = self.store.run(&run_id).context("run not found")?;
        if run.status == Status::Done {
            bail!("run #{} has finished; start a new run instead", run.number);
        }
        if self.active_runs.lock().contains(&run_id) || run.status == Status::Running {
            bail!("run #{} is already running", run.number);
        }
        self.store.update_run(&run_id, |r| {
            r.status = Status::Running;
            r.error = None;
            r.finished_at = None;
            if !r.frontier.contains(&node_id) {
                r.frontier.push(node_id.clone());
            }
        })?;
        self.store.update_task(task_id, |t| {
            t.status = Status::Cancelled;
            t.summary = "Retried".into();
        })?;
        self.store.log(
            "task.retry",
            Some(&run_id),
            Some(task_id),
            format!("Retrying {}", t.title),
        );
        self.spawn_run_thread(run_id);
        Ok(())
    }

    // ---- tasks ---------------------------------------------------------------

    /// Posts your message into a task's thread; a running agent reads it before its next step.
    pub fn steer(&self, task_id: &str, text: &str) -> Result<()> {
        let mut m = Message::new(task_id, Role::User, text);
        m.author = "you".into();
        self.store.put_message(&m)?;
        let t = self.store.task(task_id);
        let title = t.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        self.store.log(
            "task.steer",
            t.as_ref().and_then(|t| t.run_id.as_deref()),
            Some(task_id),
            format!("You steered {title}"),
        );
        Ok(())
    }

    /// Cancels a task and every sub-task it spawned.
    pub fn cancel_task(&self, task_id: &str) -> Result<()> {
        if self
            .store
            .task(task_id)
            .map(|t| t.status.is_active())
            .unwrap_or(false)
        {
            self.cancel_flag(task_id).store(true, Ordering::SeqCst);
        }
        self.store.update_task(task_id, |t| {
            if t.status.is_active() {
                t.status = Status::Cancelled;
                t.finished_at = Some(now());
            }
        })?;
        for c in self.store.children(task_id) {
            if c.status.is_active() {
                self.cancel_task(&c.id)?;
            }
        }
        Ok(())
    }

    /// Moves a task on the Board / Timeline (lower = earlier).
    pub fn set_priority(&self, task_id: &str, priority: i64) -> Result<()> {
        self.store.update_task(task_id, |t| t.priority = priority)?;
        Ok(())
    }

    /// Your decision on a task that waits for you (gate, plan approval, question).
    pub fn decide(&self, task_id: &str, kind: DecisionKind) -> Result<()> {
        let t = self.store.task(task_id).context("task not found")?;
        if t.status != Status::Waiting {
            bail!("this item is no longer waiting for you");
        }
        if self
            .store
            .decisions_for(task_id)
            .iter()
            .any(|d| !d.consumed)
        {
            // Already decided (double click); the run will pick it up.
            return Ok(());
        }
        let d = Decision {
            id: new_id(),
            task_id: task_id.into(),
            run_id: t.run_id.clone(),
            kind: kind.clone(),
            created_at: now(),
            consumed: false,
        };
        self.store.put_decision(&d)?;
        let what = match &kind {
            DecisionKind::Pick { index } => format!("Picked {}", variant_letter(*index)),
            DecisionKind::Approve => "Approved".into(),
            DecisionKind::SendBack { .. } => "Sent back with a note".into(),
            DecisionKind::Reject { .. } => "Rejected".into(),
            DecisionKind::Reply { .. } => "Replied".into(),
        };
        self.store.log(
            "decision",
            t.run_id.as_deref(),
            Some(task_id),
            format!("{what} · {}", t.title),
        );
        // Runs waiting in this process are polling; a run whose thread is gone resumes now.
        if let Some(run_id) = t.run_id {
            self.spawn_run_thread(run_id);
        }
        Ok(())
    }

    // ---- pipelines -----------------------------------------------------------

    pub fn templates(&self) -> Vec<Template> {
        templates::all(&self.config())
    }

    pub fn create_pipeline(&self, template: &Template, name: &str) -> Result<Pipeline> {
        let mut p = templates::instantiate(template, name);
        let ids = self.ensure_agents(&template.agents)?;
        templates::remap_agents(&mut p.graph, &ids);
        self.store.put_pipeline(&p)?;
        self.store.log(
            "pipeline.created",
            None,
            None,
            format!("Pipeline {} created", p.name),
        );
        Ok(p)
    }

    /// Saves template agents, reusing saved agents with the same name. Returns template key -> id.
    fn ensure_agents(&self, defs: &[AgentDef]) -> Result<HashMap<String, Id>> {
        let mut ids = HashMap::new();
        for d in defs {
            let id = match self.store.agent_by_name(&d.name) {
                Some(a) => a.id,
                None => {
                    let mut a = d.clone();
                    a.id = new_id();
                    a.created_at = now();
                    a.updated_at = now();
                    self.store.put_agent(&a)?;
                    self.store.log(
                        "agent.created",
                        None,
                        None,
                        format!("Agent {} created", a.name),
                    );
                    a.id
                }
            };
            ids.insert(d.id.clone(), id);
        }
        Ok(ids)
    }

    /// Turns a pipeline's inline agent steps (and coder delegates) into saved agents the
    /// steps refer to, so they can be edited on the Agents screen. Saves a new version.
    /// Returns the names of the agents it saved or reused.
    pub fn adopt_agents(&self, pipeline_id: &str) -> Result<Vec<String>> {
        let mut p = self
            .store
            .pipeline(pipeline_id)
            .context("pipeline not found")?;
        let mut graph = p.graph.clone();
        let mut names = vec![];
        let fallback_model = self.config().default_model;
        for n in &mut graph.nodes {
            let NodeKind::Agent(a) = &mut n.kind else {
                continue;
            };
            if a.agent.is_none() {
                let def = AgentDef {
                    id: n.id.clone(),
                    name: n.name.clone(),
                    description: String::new(),
                    role: String::new(),
                    model: if a.model.is_empty() {
                        fallback_model.clone()
                    } else {
                        a.model.clone()
                    },
                    tools: a.tools.unwrap_or(ToolAccess::Full),
                    skills: std::mem::take(&mut a.skills),
                    skills_on_demand: std::mem::take(&mut a.skills_on_demand),
                    max_steps: a.max_steps,
                    version: 1,
                    created_at: now(),
                    updated_at: now(),
                };
                let ids = self.ensure_agents(std::slice::from_ref(&def))?;
                a.agent = ids.get(&def.id).cloned();
                a.model = String::new();
                a.tools = None;
                names.push(def.name);
            }
            if let Some(d) = a.delegate.as_mut().filter(|d| d.agent.is_none()) {
                let def = AgentDef {
                    id: format!("{}-delegate", n.id),
                    name: if d.name.is_empty() {
                        "Coder".into()
                    } else {
                        d.name.clone()
                    },
                    description: String::new(),
                    role: std::mem::take(&mut d.instructions),
                    model: if d.model.is_empty() {
                        fallback_model.clone()
                    } else {
                        std::mem::take(&mut d.model)
                    },
                    tools: ToolAccess::Full,
                    skills: vec![],
                    skills_on_demand: std::mem::take(&mut d.skills_on_demand),
                    max_steps: d.max_steps,
                    version: 1,
                    created_at: now(),
                    updated_at: now(),
                };
                let ids = self.ensure_agents(std::slice::from_ref(&def))?;
                d.agent = ids.get(&def.id).cloned();
                d.name = String::new();
                names.push(def.name);
            }
        }
        if names.is_empty() {
            return Ok(names);
        }
        p.graph = graph;
        p.version += 1;
        p.draft = None;
        p.updated_at = now();
        self.store.put_pipeline(&p)?;
        Ok(names)
    }

    /// Saves edits as the pipeline's draft (runs keep using the live version).
    pub fn save_draft(&self, pipeline_id: &str, graph: PipelineGraph) -> Result<()> {
        let mut p = self
            .store
            .pipeline(pipeline_id)
            .context("pipeline not found")?;
        p.draft = if graph == p.graph { None } else { Some(graph) };
        p.updated_at = now();
        self.store.put_pipeline(&p)
    }

    /// Makes the draft live as a new version.
    pub fn deploy(&self, pipeline_id: &str) -> Result<u32> {
        let mut p = self
            .store
            .pipeline(pipeline_id)
            .context("pipeline not found")?;
        let Some(d) = p.draft.take() else {
            return Ok(p.version);
        };
        pipeline::validate(&d)?;
        p.graph = d;
        p.version += 1;
        p.updated_at = now();
        self.store.put_pipeline(&p)?;
        self.store.log(
            "pipeline.deployed",
            None,
            None,
            format!("{} v{} deployed", p.name, p.version),
        );
        Ok(p.version)
    }

    pub fn discard_draft(&self, pipeline_id: &str) -> Result<()> {
        let mut p = self
            .store
            .pipeline(pipeline_id)
            .context("pipeline not found")?;
        p.draft = None;
        self.store.put_pipeline(&p)
    }

    /// Sends a tiny request to check a provider/model works. Returns the model's reply.
    pub fn test_model(&self, model: &str) -> Result<String> {
        let c = self.config();
        let (p, m) = c.resolve(model).context("no provider for this model")?;
        let (text, _, _) = llm::quick(p, &m, "Reply with the single word: ok", "ping")?;
        Ok(text)
    }

    /// Turns an approved orchestrator plan into a pipeline: one agent step per plan step,
    /// working in `cwd` (a git repo gets worktrees), ending with a review gate.
    pub fn pipeline_from_plan(
        &self,
        name: &str,
        steps: &[String],
        cwd: Option<&str>,
    ) -> Result<Pipeline> {
        let mut g = PipelineGraph::default();
        let wd = cwd.filter(|c| !c.is_empty()).map(|c| {
            let path = harness_core::config::expand_tilde(c);
            Workdir {
                id: "work".into(),
                name: "Work".into(),
                path: c.to_string(),
                worktree: git::is_repo(&path),
                base_branch: git::git(&path, &["rev-parse", "--abbrev-ref", "HEAD"])
                    .unwrap_or_else(|_| "main".into()),
                branch_prefix: "pipeliner/".into(),
                setup: String::new(),
                shell: String::new(),
                env: Default::default(),
            }
        });
        if let Some(w) = &wd {
            g.workdirs.push(w.clone());
        }
        g.nodes.push(Node {
            id: "source".into(),
            name: "Task".into(),
            kind: NodeKind::Source(SourceNode::default()),
            x: 0.0,
            y: 0.0,
        });
        let mut prev = "source".to_string();
        for (i, step) in steps.iter().enumerate() {
            let id = format!("step{}", i + 1);
            let title: String = step
                .split(['.', ':', '\n'])
                .next()
                .unwrap_or(step)
                .chars()
                .take(40)
                .collect();
            g.nodes.push(Node {
                id: id.clone(),
                name: title,
                kind: NodeKind::Agent(AgentNode {
                    instructions: step.clone(),
                    workdir: wd.as_ref().map(|w| w.id.clone()),
                    ..Default::default()
                }),
                x: 0.0,
                y: (i + 1) as f32,
            });
            g.edges.push(Edge {
                from: prev.clone(),
                to: id.clone(),
                port: None,
            });
            prev = id;
        }
        g.nodes.push(Node {
            id: "review".into(),
            name: "Review".into(),
            kind: NodeKind::Gate(GateNode::default()),
            x: 0.0,
            y: (steps.len() + 1) as f32,
        });
        g.edges.push(Edge {
            from: prev,
            to: "review".into(),
            port: None,
        });
        pipeline::validate(&g)?;
        let t = Template {
            id: "plan",
            name: "Plan",
            description: "Saved from an orchestrator plan.",
            graph: g,
            color: 0xa78bfa,
            agents: vec![],
        };
        self.create_pipeline(&t, name)
    }

    // ---- internals -----------------------------------------------------------

    pub(crate) fn spawn_run_thread(&self, run_id: Id) {
        if !self.is_owner() {
            // The owning process picks the run up from the database.
            return;
        }
        {
            let mut active = self.active_runs.lock();
            if !active.insert(run_id.clone()) {
                return;
            }
        }
        let engine = self.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("run-{run_id}"))
            .spawn({
                let run_id = run_id.clone();
                move || {
                    struct Guard(Engine, Id);
                    impl Drop for Guard {
                        fn drop(&mut self) {
                            self.0.active_runs.lock().remove(&self.1);
                            // A retry/decision may have arrived while this thread was exiting.
                            if let Some(r) = self.0.store.run(&self.1) {
                                if r.status == Status::Running && !std::thread::panicking() {
                                    self.0.spawn_run_thread(self.1.clone());
                                }
                            }
                        }
                    }
                    let _guard = Guard(engine.clone(), run_id.clone());
                    if let Err(e) = pipeline::drive(&engine, &run_id) {
                        let msg = format!("{e:#}");
                        let _ = engine.store.update_run(&run_id, |r| {
                            if r.status.is_active() {
                                r.status = Status::Failed;
                                r.error = Some(msg.clone());
                                r.finished_at = Some(now());
                            }
                        });
                        engine.store.log("run.failed", Some(&run_id), None, msg);
                    }
                }
            });
        if spawned.is_err() {
            self.active_runs.lock().remove(&run_id);
        }
    }

    pub(crate) fn cancel_flag(&self, task_id: &str) -> Arc<AtomicBool> {
        self.cancels
            .lock()
            .entry(task_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    pub(crate) fn drop_cancel_flag(&self, task_id: &str) {
        self.cancels.lock().remove(task_id);
    }

    pub(crate) fn acquire_slot(&self) -> SlotGuard {
        self.slots.acquire()
    }
}

/// Minimal counting semaphore for agent slots.
pub struct Semaphore {
    count: Mutex<usize>,
    cv: parking_lot::Condvar,
}

impl Semaphore {
    pub fn new(n: usize) -> Self {
        Semaphore {
            count: Mutex::new(n),
            cv: parking_lot::Condvar::new(),
        }
    }
    fn acquire(self: &Arc<Self>) -> SlotGuard {
        let mut c = self.count.lock();
        while *c == 0 {
            self.cv.wait(&mut c);
        }
        *c -= 1;
        SlotGuard(self.clone())
    }
}

pub struct SlotGuard(Arc<Semaphore>);
impl Drop for SlotGuard {
    fn drop(&mut self) {
        *self.0.count.lock() += 1;
        self.0.cv.notify_one();
    }
}
