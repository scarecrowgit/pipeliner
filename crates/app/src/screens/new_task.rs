//! "New task" composer: a reusable entity that Board and Timeline embed above their content.
//! Design: the `.nt*` block and `<section class="nt">` in design/screens/board.html.
//!
//! The host owns an `Entity<NewTaskComposer>`, subscribes to [`NewTaskEvent`] and renders the
//! entity as a child while the composer is open:
//!
//! ```ignore
//! let composer = cx.new(|cx| NewTaskComposer::new(window, cx));
//! cx.subscribe_in(&composer, window, |this, _, ev: &NewTaskEvent, _, cx| match ev {
//!     NewTaskEvent::Created(task_id) => { this.composer_open = false; cx.notify() }
//!     NewTaskEvent::Closed => { this.composer_open = false; cx.notify() }
//! });
//! composer.update(cx, |c, cx| c.open(None, "lands in Queued · first in line".into(), true, window, cx));
//! ```
//!
//! The AI only drafts the description text (`Engine::draft_description`, streamed into the
//! description input); nothing runs until the task is created (`Engine::create_task`), or, in
//! Pipeline mode, until the run starts (`Engine::start_run_with`).
//!
//! Screenshots (pasted anywhere in the composer, dropped on it or picked) go with the task:
//! a single agent gets them with its first turn, a pipeline run hands them to every step.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use gpui::{
    img, pulsating_between, Animation, AnimationExt as _, ClipboardEntry, EventEmitter,
    ExternalPaths, FocusHandle, KeyBinding, KeyDownEvent, ObjectFit, PathPromptOptions,
    StyledImage as _,
};
use gpui_component::input::{Input, InputState, Paste};
use gpui_component::select::SelectState;
use gpui_component::tooltip::Tooltip;
use harness_core::{Attachment, NodeKind, PipelineGraph};
use harness_engine::pipeline::NewRun;
use harness_engine::tasks::{Draft, DraftRequest, NewTask};

use super::prelude::*;
use crate::ui::input::{self, InputEvent};
use crate::ui::select::{self, Options, SelectEvent, SelectOption};

/// Instruction sent for "Rewrite" when the ask box is empty.
const REWRITE_INSTRUCTION: &str = "Rewrite it more clearly";

/// Screenshots per task.
const MAX_SHOTS: usize = 5;

gpui::actions!(new_task_composer, [PasteScreenshot]);

static KEYS: Once = Once::new();

/// Who does the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    /// One agent (or the general worker) works on it.
    Agent,
    /// It becomes the input of a new pipeline run.
    Pipeline,
}

/// How a step looks in the pipeline preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChipKind {
    /// The step that gets the task first.
    First,
    /// A step that waits for you.
    Gate,
    Step,
}

/// A pipeline as the composer shows it.
#[derive(Clone, Debug)]
struct PipelineInfo {
    id: Id,
    name: String,
    version: u32,
    color: u32,
    steps: Vec<(String, ChipKind)>,
    works_in: String,
}

/// What the composer tells its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewTaskEvent {
    /// The task was created (id of the new task).
    Created(Id),
    /// Pipeline mode: the task started a run (id of the run).
    RunStarted(Id),
    /// Cancelled, closed or discarded with Esc.
    Closed,
}

/// Model and cost of the last finished AI draft (for the "Filled by AI" pill).
struct DraftInfo {
    model: String,
    cost: f64,
}

pub struct NewTaskComposer {
    title: Entity<InputState>,
    description: Entity<InputState>,
    ask: Entity<InputState>,
    cwd: Entity<InputState>,
    agent_select: Entity<SelectState<Options>>,
    run_select: Entity<SelectState<Options>>,
    model_select: Entity<SelectState<Options>>,
    pipeline_select: Entity<SelectState<Options>>,
    focus: FocusHandle,

    who: Who,
    pipeline_id: Option<Id>,
    /// Screenshots that go with the task; files live in the attachments folder.
    attachments: Vec<Attachment>,
    /// Images still being saved in the background.
    adding: usize,

    context_label: SharedString,
    show_run_picker: bool,
    run_id: Option<Id>,
    agent_id: Option<Id>,
    model: String,
    hold: bool,
    creating: bool,
    /// Bumped by every `open`, so the enter animation replays.
    seq: u64,
    /// Options under the description (narrow side panel) instead of beside it.
    stacked: bool,

    /// Option sources, refreshed in the background on every `open`: (id, name).
    agents: Vec<(Id, String)>,
    /// (id, number, title), newest first.
    runs: Vec<(Id, u64, String)>,
    models: Vec<String>,
    pipelines: Vec<PipelineInfo>,

    // AI draft state.
    writing: bool,
    draft: Option<DraftInfo>,
    /// Description before the last AI draft started (what Undo restores).
    undo: Option<String>,
    /// "from the title and run #N".
    source: String,
    cancel: Option<Arc<AtomicBool>>,
    /// Bumped whenever a draft starts or is cancelled; results of older drafts are ignored.
    generation: u64,
    draft_task: Option<Task<()>>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<NewTaskEvent> for NewTaskComposer {}

impl NewTaskComposer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        KEYS.call_once(|| {
            cx.bind_keys([
                KeyBinding::new(
                    "secondary-v",
                    PasteScreenshot,
                    Some("NewTaskComposer > Input"),
                ),
                KeyBinding::new("secondary-v", PasteScreenshot, Some("NewTaskComposer")),
            ]);
        });
        let title = input::text_input(
            "What should be done? e.g. Fix the refund webhook",
            window,
            cx,
        );
        let description = input::composer_rows(
            "Describe the goal, the steps, and when it is done",
            6,
            14,
            window,
            cx,
        );
        let ask = input::text_input(
            "Ask AI to change it, e.g. “shorter”, “add a rollback step”",
            window,
            cx,
        );
        let cwd = input::text_input("Folder (optional), e.g. ~/code/app", window, cx);
        let agent_select = select::select_state(
            vec![SelectOption::new("", "General worker")],
            Some(""),
            false,
            window,
            cx,
        );
        let run_select = select::select_state(
            vec![SelectOption::new("", "No run")],
            Some(""),
            true,
            window,
            cx,
        );
        let model_select = select::select_state(
            vec![SelectOption::new("", "Agent / default model")],
            Some(""),
            true,
            window,
            cx,
        );
        let pipeline_select = select::select_state(
            vec![SelectOption::new("", "No pipelines yet")],
            Some(""),
            true,
            window,
            cx,
        );

        let mut subs = Vec::new();
        for i in [&title, &description] {
            subs.push(
                cx.subscribe_in(i, window, |this, _, ev: &InputEvent, window, cx| {
                    match ev {
                        InputEvent::Change => cx.notify(),
                        // Ctrl/Cmd+Enter creates the task.
                        InputEvent::PressEnter { secondary: true } => this.create(window, cx),
                        _ => {}
                    }
                }),
            );
        }
        subs.push(
            cx.subscribe_in(&ask, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.ask_ai(window, cx);
                }
            }),
        );
        subs.push(cx.subscribe_in(
            &agent_select,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                let SelectEvent::Confirm(v) = ev;
                this.agent_id = v.clone().filter(|v| !v.is_empty());
                cx.notify();
            },
        ));
        subs.push(cx.subscribe_in(
            &run_select,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                let SelectEvent::Confirm(v) = ev;
                this.run_id = v.clone().filter(|v| !v.is_empty());
                cx.notify();
            },
        ));
        subs.push(cx.subscribe_in(
            &model_select,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                let SelectEvent::Confirm(v) = ev;
                this.model = v.clone().unwrap_or_default();
                cx.notify();
            },
        ));
        subs.push(cx.subscribe_in(
            &pipeline_select,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                let SelectEvent::Confirm(v) = ev;
                this.pipeline_id = v.clone().filter(|v| !v.is_empty());
                cx.notify();
            },
        ));

        NewTaskComposer {
            title,
            description,
            ask,
            cwd,
            agent_select,
            run_select,
            model_select,
            pipeline_select,
            focus: cx.focus_handle(),
            who: Who::Agent,
            pipeline_id: None,
            attachments: Vec::new(),
            adding: 0,
            context_label: SharedString::default(),
            show_run_picker: true,
            run_id: None,
            agent_id: None,
            model: String::new(),
            hold: false,
            creating: false,
            seq: 0,
            stacked: false,
            agents: Vec::new(),
            runs: Vec::new(),
            models: Vec::new(),
            pipelines: Vec::new(),
            writing: false,
            draft: None,
            undo: None,
            source: String::new(),
            cancel: None,
            generation: 0,
            draft_task: None,
            load: None,
            _subs: subs,
        }
    }

    /// Resets every field and preselects `run_id` (or none). `context_label` is the head text
    /// ("lands in Queued · first in line", "in run #129").
    pub fn open(
        &mut self,
        run_id: Option<Id>,
        context_label: SharedString,
        show_run_picker: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_draft();
        self.seq += 1;
        self.context_label = context_label;
        self.show_run_picker = show_run_picker;
        self.run_id = run_id;
        self.agent_id = None;
        self.model = String::new();
        self.hold = false;
        self.creating = false;
        self.draft = None;
        self.undo = None;
        self.source = String::new();
        self.discard_all(cx);
        for i in [&self.title, &self.description, &self.ask, &self.cwd] {
            i.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.apply_options(window, cx);
        self.refresh_options(window, cx);
        self.focus_title(window, cx);
        cx.notify();
    }

    /// Lay the options out under the description (for a narrow side panel).
    pub fn set_stacked(&mut self, stacked: bool, cx: &mut Context<Self>) {
        if self.stacked != stacked {
            self.stacked = stacked;
            cx.notify();
        }
    }

    pub fn focus_title(&self, window: &mut Window, cx: &mut App) {
        self.title.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The title typed so far (the board shows it in its ghost preview card).
    pub fn title(&self, cx: &App) -> String {
        self.title.read(cx).value().to_string()
    }

    /// Who does it and what goes with it, for the board's ghost card and the timeline's
    /// draft row: "Worker", "Bug triage · new run · 2 screenshots".
    pub fn preview_label(&self) -> SharedString {
        let who = match self.who {
            Who::Agent => self
                .agent_id
                .as_ref()
                .and_then(|id| self.agents.iter().find(|a| &a.0 == id))
                .map(|a| a.1.clone())
                .unwrap_or_else(|| "Worker".into()),
            Who::Pipeline => match self.pipeline() {
                Some(p) => format!("{} · new run", p.name),
                None => "Pipeline · new run".into(),
            },
        };
        preview_text(&who, self.attachments.len()).into()
    }

    /// Headless screenshots (`PIPELINER_NEW_TASK`): the mode, plus the images listed in
    /// `PIPELINER_DRAFT_IMAGES`.
    pub fn preset_for_shots(
        &mut self,
        pipeline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let who = if pipeline { Who::Pipeline } else { Who::Agent };
        self.set_who(who, window, cx);
        if let Some(paths) = std::env::var_os("PIPELINER_DRAFT_IMAGES") {
            self.add_files(std::env::split_paths(&paths).collect(), cx);
        }
    }

    fn pipeline(&self) -> Option<&PipelineInfo> {
        let id = self.pipeline_id.as_ref()?;
        self.pipelines.iter().find(|p| &p.id == id)
    }

    /// Number the next run gets (shown as "New run #N").
    fn next_run_number(&self) -> u64 {
        self.runs.iter().map(|r| r.1).max().unwrap_or(0) + 1
    }

    fn set_who(&mut self, who: Who, window: &mut Window, cx: &mut Context<Self>) {
        self.who = who;
        if who == Who::Pipeline && self.pipeline().is_none() {
            self.pipeline_id = self.pipelines.first().map(|p| p.id.clone());
            self.apply_options(window, cx);
        }
        cx.notify();
    }

    // ---- options ---------------------------------------------------------------

    /// Pushes the cached agents / runs / models into the selects, keeping the choices.
    fn apply_options(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mut agents = vec![SelectOption::new("", "General worker")];
        agents.extend(
            self.agents
                .iter()
                .map(|(id, name)| SelectOption::new(id.clone(), name.clone())),
        );
        select::set_options(
            &self.agent_select,
            agents,
            Some(self.agent_id.as_deref().unwrap_or("")),
            window,
            cx,
        );
        let mut runs = vec![SelectOption::new("", "No run")];
        runs.extend(
            self.runs
                .iter()
                .map(|(id, n, title)| SelectOption::new(id.clone(), format!("#{n} {title}"))),
        );
        select::set_options(
            &self.run_select,
            runs,
            Some(self.run_id.as_deref().unwrap_or("")),
            window,
            cx,
        );
        let mut models = vec![SelectOption::new("", "Agent / default model")];
        models.extend(
            self.models
                .iter()
                .map(|m| SelectOption::new(m.clone(), Config::short_model(m))),
        );
        select::set_options(&self.model_select, models, Some(&self.model), window, cx);
        let pipelines: Vec<SelectOption> = if self.pipelines.is_empty() {
            vec![SelectOption::new("", "No pipelines yet")]
        } else {
            self.pipelines
                .iter()
                .map(|p| SelectOption::new(p.id.clone(), format!("{} · v{}", p.name, p.version)))
                .collect()
        };
        select::set_options(
            &self.pipeline_select,
            pipelines,
            Some(self.pipeline_id.as_deref().unwrap_or("")),
            window,
            cx,
        );
    }

    /// Re-reads agents, runs and models on a background thread (`Engine::models` may ask the
    /// providers), then applies them.
    fn refresh_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = state::store(cx);
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move {
            let agents: Vec<(Id, String)> =
                store.agents().into_iter().map(|a| (a.id, a.name)).collect();
            let mut runs: Vec<(Id, u64, String)> = store
                .runs()
                .into_iter()
                .map(|r| (r.id, r.number, r.title))
                .collect();
            runs.sort_by_key(|r| std::cmp::Reverse(r.1));
            let pipelines: Vec<PipelineInfo> = store
                .pipelines()
                .into_iter()
                .filter(|p| !p.archived)
                .map(|p| PipelineInfo {
                    steps: pipeline_steps(&p.graph),
                    works_in: works_in(&p.graph),
                    id: p.id,
                    name: p.name,
                    version: p.version,
                    color: p.color,
                })
                .collect();
            (agents, runs, engine.models(), pipelines)
        });
        self.load = Some(cx.spawn_in(window, async move |this, cx| {
            let (agents, runs, models, pipelines) = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.agents = agents;
                this.runs = runs;
                this.models = models;
                this.pipelines = pipelines;
                // Keep the choice if the pipeline still exists, else the first one.
                if this.pipeline().is_none() {
                    this.pipeline_id = this.pipelines.first().map(|p| p.id.clone());
                }
                this.apply_options(window, cx);
                cx.notify();
            });
        }));
    }

    // ---- AI draft --------------------------------------------------------------

    fn run_number(&self) -> Option<u64> {
        let id = self.run_id.as_ref()?;
        self.runs.iter().find(|r| &r.0 == id).map(|r| r.1)
    }

    /// Stops a running draft and makes its late results irrelevant.
    fn cancel_draft(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::SeqCst);
        }
        self.generation += 1;
        self.draft_task = None;
        self.writing = false;
    }

    /// "Write with AI" / "Rewrite".
    fn write_with_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.description.read(cx).value().to_string();
        let instruction = draft_instruction("", &current);
        self.start_draft(instruction, window, cx);
    }

    /// Enter in the ask box.
    fn ask_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ask.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.ask.update(cx, |s, cx| s.set_value("", window, cx));
        self.start_draft(text, window, cx);
    }

    /// Streams a draft into the description. The stream callback runs on the engine's
    /// background thread; the text goes through a channel and is applied on the UI thread.
    fn start_draft(&mut self, instruction: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().to_string();
        let current = self.description.read(cx).value().to_string();
        if title.trim().is_empty() && current.trim().is_empty() {
            state::toast(ToastKind::Warning, "Give the task a title first", cx);
            return;
        }
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::SeqCst);
        }
        self.generation += 1;
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        // Keep the text from before the first draft when one is replaced mid-stream.
        if !self.writing {
            self.undo = Some(current.clone());
        }
        self.writing = true;
        self.draft = None;
        let run_number = match self.who {
            Who::Agent => self.run_number(),
            Who::Pipeline => None,
        };
        self.source = ai_source_label(run_number, self.attachments.len());

        let req = DraftRequest {
            title,
            run_id: run_number.and(self.run_id.clone()),
            current,
            instruction,
            attachments: self.attachments.clone(),
        };
        let engine = state::engine(cx);
        let (tx, rx) = smol::channel::unbounded::<String>();
        let job = cx.background_spawn(async move {
            engine.draft_description(req, cancel, move |text: &str| {
                let _ = tx.send_blocking(text.to_string());
            })
        });
        self.draft_task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(mut text) = rx.recv().await {
                // Only the latest text matters; skip what piled up meanwhile.
                while let Ok(t) = rx.try_recv() {
                    text = t;
                }
                let live = this.update_in(cx, |this, window, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    this.description
                        .update(cx, |s, cx| s.set_value(text, window, cx));
                    cx.notify();
                    true
                });
                if !matches!(live, Ok(true)) {
                    return;
                }
            }
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_draft(generation, result, window, cx)
            });
        }));
        cx.notify();
    }

    fn finish_draft(
        &mut self,
        generation: u64,
        result: anyhow::Result<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.generation != generation {
            return;
        }
        self.writing = false;
        self.cancel = None;
        match result {
            Ok(d) if !d.text.trim().is_empty() => {
                let text = d.text.trim().to_string();
                self.description
                    .update(cx, |s, cx| s.set_value(text, window, cx));
                self.draft = Some(DraftInfo {
                    model: d.model,
                    cost: d.cost_usd,
                });
            }
            Ok(_) => self.restore_description(window, cx),
            Err(e) => {
                state::toast(ToastKind::Danger, format!("{e:#}"), cx);
                self.restore_description(window, cx);
            }
        }
        cx.notify();
    }

    /// Puts back the text from before the draft and forgets it.
    fn restore_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = self.undo.take() {
            self.description
                .update(cx, |s, cx| s.set_value(prev, window, cx));
        }
        self.draft = None;
    }

    fn stop_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.cancel {
            // The engine returns what was written so far; `finish_draft` keeps it.
            c.store(true, Ordering::SeqCst);
        }
        cx.notify();
    }

    // ---- create / close --------------------------------------------------------

    fn create(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().to_string();
        if self.creating || title.trim().is_empty() {
            return;
        }
        if self.adding > 0 {
            state::toast(
                ToastKind::Info,
                "Wait for the screenshots to finish adding",
                cx,
            );
            return;
        }
        if self.who == Who::Pipeline {
            self.start_run(&title, cx);
            return;
        }
        let mut n = build_new_task(
            &title,
            &self.description.read(cx).value(),
            self.run_id.clone(),
            self.agent_id.clone(),
            &self.model,
            &self.cwd.read(cx).value(),
            self.hold,
        );
        n.attachments = self.attachments.clone();
        let label = n.title.clone();
        self.creating = true;
        cx.notify();
        let job = state::run_engine(cx, move |e| e.create_task(n));
        cx.spawn(async move |this, cx| {
            let created = job.await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                if let Some(task) = created {
                    this.cancel_draft();
                    // The task owns the screenshots now.
                    this.attachments.clear();
                    state::toast(ToastKind::Success, format!("Created {label}"), cx);
                    cx.emit(NewTaskEvent::Created(task.id));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Pipeline mode: the task becomes the input of a new run of the chosen pipeline.
    fn start_run(&mut self, title: &str, cx: &mut Context<Self>) {
        let Some(p) = self.pipeline().cloned() else {
            state::toast(ToastKind::Warning, "Pick a pipeline first", cx);
            return;
        };
        let n = NewRun {
            pipeline_id: p.id.clone(),
            title: title.trim().to_string(),
            input: run_input(title, &self.description.read(cx).value()),
            chat_id: None,
            attachments: self.attachments.clone(),
        };
        self.creating = true;
        cx.notify();
        let job = state::run_engine(cx, move |e| e.start_run_with(n));
        cx.spawn(async move |this, cx| {
            let started = job.await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                if let Some(run) = started {
                    this.cancel_draft();
                    this.attachments.clear();
                    state::toast(
                        ToastKind::Success,
                        format!("Started run #{} of {}", run.number, p.name),
                        cx,
                    );
                    cx.emit(NewTaskEvent::RunStarted(run.id));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.cancel_draft();
        self.discard_all(cx);
        cx.emit(NewTaskEvent::Closed);
    }

    // ---- screenshots -----------------------------------------------------------

    fn free_slots(&self) -> usize {
        MAX_SHOTS.saturating_sub(self.attachments.len() + self.adding)
    }

    fn warn_full(cx: &mut App) {
        state::toast(
            ToastKind::Warning,
            format!("Up to {MAX_SHOTS} screenshots per task"),
            cx,
        );
    }

    /// Saves an image on a background thread and adds it.
    fn spawn_add(
        &mut self,
        job: impl FnOnce(&harness_engine::Engine) -> anyhow::Result<Attachment> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.adding += 1;
        cx.notify();
        let engine = state::engine(cx);
        let task = cx.background_spawn(async move { job(&engine) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.adding -= 1;
                match res {
                    Ok(a) if this.attachments.len() < MAX_SHOTS => this.attachments.push(a),
                    Ok(a) => {
                        Self::discard(a.path, cx);
                        Self::warn_full(cx);
                    }
                    Err(e) => state::toast(ToastKind::Danger, format!("{e:#}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Deletes a screenshot file that never went anywhere, in the background.
    fn discard(path: String, cx: &mut App) {
        state::run_engine(cx, move |e| {
            e.discard_attachment(&path);
            Ok(())
        })
        .detach();
    }

    fn discard_all(&mut self, cx: &mut App) {
        for a in std::mem::take(&mut self.attachments) {
            Self::discard(a.path, cx);
        }
    }

    fn remove_shot(&mut self, path: &str, cx: &mut Context<Self>) {
        let before = self.attachments.len();
        self.attachments.retain(|a| a.path != path);
        if self.attachments.len() != before {
            Self::discard(path.to_string(), cx);
        }
        cx.notify();
    }

    /// Adds image files (picked or dropped), as many as fit.
    fn add_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let free = self.free_slots();
        if free == 0 {
            Self::warn_full(cx);
            return;
        }
        if paths.len() > free {
            state::toast(
                ToastKind::Info,
                format!(
                    "Only {} more fit: the rest were skipped",
                    count_noun(free, "screenshot")
                ),
                cx,
            );
        }
        for path in paths.into_iter().take(free) {
            self.spawn_add(move |e| e.add_attachment_file(&path), cx);
        }
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        if self.free_slots() == 0 {
            Self::warn_full(cx);
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add screenshot".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| this.add_files(paths, cx));
            }
        })
        .detach();
    }

    /// Ctrl/Cmd-V anywhere in the composer: an image on the clipboard becomes a screenshot,
    /// anything else pastes as text into the focused field.
    fn on_paste(&mut self, _: &PasteScreenshot, window: &mut Window, cx: &mut Context<Self>) {
        let image = cx.read_from_clipboard().and_then(|item| {
            if item.text().is_some_and(|t| !t.is_empty()) {
                return None;
            }
            item.into_entries().find_map(|e| match e {
                ClipboardEntry::Image(i) => Some(i),
                _ => None,
            })
        });
        match image {
            Some(image) => {
                if self.free_slots() == 0 {
                    Self::warn_full(cx);
                    return;
                }
                let bytes = image.bytes;
                self.spawn_add(move |e| e.save_attachment_bytes(&bytes, None, true), cx);
            }
            None => window.dispatch_action(Box::new(Paste), cx),
        }
    }

    // ---- render pieces ---------------------------------------------------------

    fn render_head(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::S2)
            .pt(px(10.))
            .pb(px(10.))
            .pr(px(12.))
            .pl(px(14.))
            .border_b_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(theme::RADIUS_MD)
                    .bg(theme::ACCENT_SOFT)
                    .text_color(theme::ACCENT_TEXT)
                    .child(ui::icon_sized(AppIcon::Plus, theme::ICON_SM)),
            )
            .child(div().text_heading().fg().child("New task"))
            .child(
                div()
                    .text_label()
                    .fg_muted()
                    .child(self.context_label.clone()),
            )
            .child(div().flex_1())
            .child(
                IconButton::new("nt-close", AppIcon::X)
                    .small()
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
    }

    fn render_description_head(&self, has_desc: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let pill = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .px(theme::S2)
            .rounded(theme::RADIUS_FULL)
            .bg(theme::ACCENT_SOFT)
            .text_color(theme::ACCENT_TEXT)
            .text_size(px(12.))
            .line_height(px(16.))
            .whitespace_nowrap();
        let pill = if self.writing {
            Some(
                pill.child(
                    div()
                        .size(px(6.))
                        .rounded(theme::RADIUS_FULL)
                        .bg(theme::ACCENT)
                        .with_animation(
                            "nt-ai-dot",
                            Animation::new(Duration::from_millis(1200))
                                .repeat()
                                .with_easing(pulsating_between(0.3, 1.0)),
                            |d, t| d.opacity(t),
                        ),
                )
                .child("AI is writing…"),
            )
        } else {
            self.draft.as_ref().map(|d| {
                pill.child(ui::icon_sized(AppIcon::Sparkles, theme::ICON_SM))
                    .child(ai_pill_text(&d.model, d.cost))
            })
        };
        let show_source = pill.is_some();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::S2)
            .child(div().text_label().fg_secondary().child("Description"))
            .children(pill)
            .when(show_source, |d| {
                d.child(div().text_label().fg_muted().child(self.source.clone()))
            })
            .child(div().flex_1())
            .when(self.writing, |d| {
                d.child(
                    Button::new("nt-stop", "Stop")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.stop_draft(cx))),
                )
            })
            .when(!self.writing && self.undo.is_some(), |d| {
                d.child(
                    Button::new("nt-undo", "Undo")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.restore_description(window, cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new(
                    "nt-write",
                    if has_desc { "Rewrite" } else { "Write with AI" },
                )
                .secondary()
                .small()
                .icon(AppIcon::Sparkles)
                .disabled(self.writing)
                .on_click(cx.listener(|this, _, window, cx| this.write_with_ai(window, cx))),
            )
    }

    fn render_body(&self, has_desc: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let description = div()
            .relative()
            .rounded(theme::RADIUS_LG)
            .overflow_hidden()
            .child(input::composer_field(&self.description).bg(theme::BG_SIDEBAR))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(2.))
                    .bg(theme::ACCENT),
            );
        let ask = div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .h(px(32.))
            .pl(px(10.))
            .pr(px(6.))
            .border_1()
            .border_color(theme::LINE_STRONG)
            .rounded(theme::RADIUS_LG)
            .bg(theme::SURFACE_RAISED)
            .text_color(theme::ACCENT_TEXT)
            .child(ui::icon_sized(AppIcon::Sparkles, theme::ICON_SM))
            .child(
                Input::new(&self.ask)
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.)),
            )
            .child(kbd("↵"));
        div()
            .flex()
            .flex_col()
            .when(!self.stacked, |d| d.flex_1())
            .min_w_0()
            .gap(px(10.))
            .p(px(14.))
            .child(input::field(&self.title))
            .child(self.render_description_head(has_desc, cx))
            .child(description)
            .child(self.render_shots(cx))
            .child(ask)
    }

    /// "Screenshots" head, thumbnails and the "Add screenshot" tile.
    fn render_shots(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.attachments.len();
        let free = self.free_slots();
        let paste_key = if cfg!(target_os = "macos") {
            "⌘V"
        } else {
            "Ctrl V"
        };
        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::S2)
            .child(div().text_label().fg_secondary().child("Screenshots"))
            .child(
                div()
                    .text_label()
                    .fg_muted()
                    .child(shots_caption(n, self.who)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_label()
                    .fg_muted()
                    .child(kbd(paste_key))
                    .child("paste anywhere in the task"),
            );
        let tile = || {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .items_center()
                .justify_center()
                .gap(px(4.))
                .w(px(152.))
                .h(px(86.))
                .border_1()
                .border_dashed()
                .border_color(theme::LINE_STRONG)
                .rounded(theme::RADIUS_MD)
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(theme::TEXT_MUTED)
        };
        let tray = div()
            .flex()
            .flex_wrap()
            .gap(theme::S2)
            .children(self.attachments.iter().map(|a| self.render_shot(a, cx)))
            .when(self.adding > 0, |d| {
                d.child(
                    tile().child("Adding…").with_animation(
                        "nt-shot-adding",
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_easing(pulsating_between(0.4, 1.0)),
                        |d, t| d.opacity(t),
                    ),
                )
            })
            .when(free > 0, |d| {
                d.child(
                    tile()
                        .id("nt-drop")
                        .cursor_pointer()
                        .hover(|s| {
                            s.bg(theme::SURFACE_RAISED)
                                .text_color(theme::TEXT)
                                .border_color(theme::ACCENT_RING)
                        })
                        .child(ui::icon_sized(AppIcon::Image, px(16.)))
                        .child("Add screenshot")
                        .child(
                            div()
                                .text_color(theme::TEXT_DISABLED)
                                .child("paste, drop or browse"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
                )
            });
        div()
            .flex()
            .flex_col()
            .gap(theme::S2)
            .child(head)
            .child(tray)
    }

    fn render_shot(&self, a: &Attachment, cx: &mut Context<Self>) -> impl IntoElement {
        let path = a.path.clone();
        let label = format!("Remove {}", a.name);
        div()
            .id(SharedString::from(format!("nt-shot-{}", a.path)))
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.))
            .w(px(152.))
            .child(
                div()
                    .relative()
                    .h(px(86.))
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .rounded(theme::RADIUS_MD)
                    .bg(theme::BG)
                    .overflow_hidden()
                    .child(
                        img(PathBuf::from(&a.path))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("nt-shot-x-{}", a.path)))
                            .absolute()
                            .top(px(4.))
                            .right(px(4.))
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_1()
                            .border_color(theme::LINE_STRONG)
                            .rounded(theme::RADIUS_SM)
                            .bg(theme::SURFACE_RAISED)
                            .text_color(theme::TEXT_SECONDARY)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::SURFACE_HOVER).text_color(theme::TEXT))
                            .tooltip(move |window, cx| {
                                Tooltip::new(label.clone()).build(window, cx)
                            })
                            .child(ui::icon_sized(AppIcon::X, px(12.)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove_shot(&path, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(theme::TEXT_MUTED)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ellipsis()
                            .text_color(theme::TEXT_SECONDARY)
                            .child(a.name.clone()),
                    )
                    .child(div().flex_none().whitespace_nowrap().child(shot_meta(a))),
            )
            .with_animation(
                SharedString::from(format!("nt-shot-in-{}", a.path)),
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |d, t| d.opacity(t).mt(px(6.) * (1.0 - t)),
            )
    }

    fn render_options(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |label: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(76.))
                        .flex_none()
                        .text_label()
                        .fg_muted()
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(control))
        };
        // A two-way toggle: (id, label, icon, selected, on click).
        type Pick = fn(&mut NewTaskComposer, &mut Window, &mut Context<NewTaskComposer>);
        let seg = |items: [(&'static str, &'static str, Option<AppIcon>, bool, Pick); 2],
                   cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(items.into_iter().map(|(id, label, icon, on, pick)| {
                    let b = Button::new(id, label)
                        .small()
                        .full_width()
                        .when(on, |b| b.secondary().selected(true))
                        .when(!on, |b| b.ghost())
                        .on_click(cx.listener(move |this, _, window, cx| pick(this, window, cx)));
                    div().flex_1().min_w_0().child(match icon {
                        Some(i) => b.icon(i),
                        None => b,
                    })
                }))
        };
        let pipeline = self.who == Who::Pipeline;
        let who = seg(
            [
                (
                    "nt-who-agent",
                    "One agent",
                    Some(AppIcon::Bot),
                    !pipeline,
                    |this, window, cx| this.set_who(Who::Agent, window, cx),
                ),
                (
                    "nt-who-pipeline",
                    "Pipeline",
                    Some(AppIcon::Workflow),
                    pipeline,
                    |this, window, cx| this.set_who(Who::Pipeline, window, cx),
                ),
            ],
            cx,
        );
        let col = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(theme::S2)
            .when(!self.stacked, |d| {
                d.w(px(320.))
                    .p(px(14.))
                    .border_l_1()
                    .border_color(theme::LINE)
            })
            .when(self.stacked, |d| d.px(px(14.)).pb(px(14.)))
            .child(row("Who does it", who.into_any_element()));
        if pipeline {
            return col.children(self.render_pipeline_rows(row, window, cx));
        }

        let hold = self.hold;
        let starts = seg(
            [
                (
                    "nt-start",
                    "When a slot frees",
                    None,
                    !hold,
                    |this, _, cx| {
                        this.hold = false;
                        cx.notify();
                    },
                ),
                ("nt-hold", "Hold", None, hold, |this, _, cx| {
                    this.hold = true;
                    cx.notify();
                }),
            ],
            cx,
        );
        col.child(row(
            "Agent",
            select::select_sm(&self.agent_select)
                .w_full()
                .into_any_element(),
        ))
        .when(self.show_run_picker, |d| {
            d.child(row(
                "Run",
                select::select_sm(&self.run_select)
                    .w_full()
                    .into_any_element(),
            ))
        })
        .child(row(
            "Model",
            select::select_sm(&self.model_select)
                .w_full()
                .into_any_element(),
        ))
        .child(row(
            "Works in",
            input::field_sm(&self.cwd).w_full().into_any_element(),
        ))
        .child(row("Starts", starts.into_any_element()))
    }

    /// Pipeline mode: the pipeline, what it will do, and where and how the run starts.
    fn render_pipeline_rows(
        &self,
        row: impl Fn(&'static str, AnyElement) -> Div,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if self.pipelines.is_empty() {
            return vec![row(
                "Pipeline",
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .child(div().text_label().fg_muted().child("No pipelines yet"))
                    .child(
                        Button::new("nt-go-pipelines", "Create one")
                            .ghost()
                            .small()
                            .on_click(|_, _, cx| {
                                state::navigate(Route::Pipelines { pipeline: None }, cx)
                            }),
                    )
                    .into_any_element(),
            )
            .into_any_element()];
        }
        let value = |main: SharedString, muted: Option<&'static str>, mono: bool| {
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .h(theme::CONTROL_H_SM)
                .px(theme::S2)
                .border_1()
                .border_color(theme::LINE_STRONG)
                .rounded(theme::RADIUS_MD)
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .ellipsis()
                        .text_color(theme::TEXT)
                        .when(mono, |d| d.text_mono_sm())
                        .when(!mono, |d| d.text_label())
                        .child(main),
                )
                .children(muted.map(|m| {
                    div()
                        .flex_none()
                        .text_label()
                        .fg_muted()
                        .whitespace_nowrap()
                        .child(m)
                }))
                .into_any_element()
        };
        let p = self.pipeline();
        let mut out = vec![row(
            "Pipeline",
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .children(p.map(|p| dot_el(theme::rgb_u32(p.color), px(8.)).flex_none()))
                .child(
                    select::select_sm(&self.pipeline_select)
                        .w_full()
                        .flex_1()
                        .min_w_0(),
                )
                .into_any_element(),
        )
        .into_any_element()];
        if let Some(p) = p {
            out.push(render_pipe_preview(p).into_any_element());
            out.push(
                row(
                    "Run",
                    value(
                        format!("New run #{}", self.next_run_number()).into(),
                        None,
                        false,
                    ),
                )
                .into_any_element(),
            );
            out.push(
                row(
                    "Models",
                    value("Per step".into(), Some("as set in the pipeline"), false),
                )
                .into_any_element(),
            );
            out.push(
                row("Works in", value(p.works_in.clone().into(), None, true)).into_any_element(),
            );
            out.push(
                row("Starts", value("When a slot frees".into(), None, false)).into_any_element(),
            );
        }
        let _ = cx;
        out
    }

    fn render_foot(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pipeline = self.who == Who::Pipeline;
        let blocked = self.creating
            || self.title.read(cx).value().trim().is_empty()
            || (pipeline && self.pipeline().is_none());
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::S2)
            .pt(px(10.))
            .pb(px(10.))
            .pr(px(12.))
            .pl(px(14.))
            .border_t_1()
            .border_color(theme::LINE)
            .child(div().text_label().fg_muted().child(if pipeline {
                "The AI only drafts the text. Nothing runs until you start the run."
            } else {
                "The AI only drafts the text. Nothing runs until you create it."
            }))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_label()
                    .fg_muted()
                    .child(kbd("Esc"))
                    .child("discard"),
            )
            .child(
                Button::new("nt-cancel", "Cancel")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            .child(
                Button::new(
                    "nt-create",
                    if pipeline { "Start run" } else { "Create task" },
                )
                .primary()
                .small()
                .kbd("⌘↵")
                .disabled(blocked)
                .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
            )
    }
}

impl Render for NewTaskComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_desc = !self.description.read(cx).value().trim().is_empty();
        div()
            .track_focus(&self.focus)
            .key_context("NewTaskComposer")
            .on_action(cx.listener(Self::on_paste))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(theme::ACCENT_GLOW))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_files(paths.paths().to_vec(), cx)
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
            .flex()
            .flex_col()
            .min_w_0()
            .border_1()
            .border_color(theme::ACCENT_RING)
            .rounded(theme::RADIUS_XL)
            .bg(theme::SURFACE)
            .text_color(theme::TEXT_SECONDARY)
            .child(self.render_head(cx))
            .child(
                div()
                    .flex()
                    .when(self.stacked, |d| d.flex_col())
                    .child(self.render_body(has_desc, cx))
                    .child(self.render_options(window, cx)),
            )
            .child(self.render_foot(cx))
            .with_animation(
                ("nt-enter", self.seq),
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |el, t| el.opacity(t).mt(px(-8. * (1. - t))),
            )
    }
}

// ---- pure logic ------------------------------------------------------------------

/// The pipeline preview under the picker: its steps in order, then what the run gets.
fn render_pipe_preview(p: &PipelineInfo) -> impl IntoElement {
    const SHOWN: usize = 8;
    let chip = |label: String, kind: ChipKind| {
        let (border, bg, fg) = match kind {
            ChipKind::First => (theme::ACCENT_RING, theme::ACCENT_SOFT, theme::ACCENT_TEXT),
            ChipKind::Gate => (theme::WARNING, theme::WARNING_SOFT, theme::WARNING),
            ChipKind::Step => (theme::LINE, theme::SURFACE_RAISED, theme::TEXT_SECONDARY),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(20.))
            .px(px(6.))
            .border_1()
            .border_color(border)
            .rounded(theme::RADIUS_SM)
            .bg(bg)
            .text_color(fg)
            .whitespace_nowrap()
            .child(label)
    };
    let mut steps = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(4.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(theme::TEXT_DISABLED);
    for (i, (label, kind)) in p.steps.iter().take(SHOWN).enumerate() {
        if i > 0 {
            steps = steps.child("›");
        }
        steps = steps.child(chip(label.clone(), *kind));
    }
    if p.steps.len() > SHOWN {
        steps = steps.child(format!("+{} more", p.steps.len() - SHOWN));
    }
    if p.steps.is_empty() {
        steps = steps.child("No steps yet");
    }
    div()
        .flex()
        .flex_col()
        .gap(theme::S2)
        .p(px(10.))
        .border_1()
        .border_color(theme::LINE_STRONG)
        .rounded(theme::RADIUS_LG)
        .bg(theme::BG_SIDEBAR)
        // A fixed width (the options column's inner width), capped by the parent: with only
        // `w_full`, gpui measures the wrapping rows from their content and the box grows tall.
        .w(px(292.))
        .max_w_full()
        .child(steps.w_full())
        .child(div().w_full().text_label().fg_muted().child(
            "The task is the input of a new run. Title, description and screenshots go to every step.",
        ))
}

/// "1 screenshot", "2 screenshots".
fn count_noun(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// "a", "a and b", "a, b and c".
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the AI read, shown after "Filled by AI": "from the title, 2 screenshots and run #129".
fn ai_source_label(run_number: Option<u64>, shots: usize) -> String {
    let mut parts = vec!["the title".to_string()];
    if shots > 0 {
        parts.push(count_noun(shots, "screenshot"));
    }
    if let Some(n) = run_number {
        parts.push(format!("run #{n}"));
    }
    format!("from {}", join_and(&parts))
}

/// Ghost card / draft row text: "Bug triage · new run · 2 screenshots".
fn preview_text(who: &str, shots: usize) -> String {
    if shots == 0 {
        who.to_string()
    } else {
        format!("{who} · {}", count_noun(shots, "screenshot"))
    }
}

/// Next to "Screenshots": how many, and who gets them.
fn shots_caption(n: usize, who: Who) -> String {
    let gets = match who {
        Who::Agent => "the agent gets them as images",
        Who::Pipeline => "every step gets them as images",
    };
    if n == 0 {
        gets.to_string()
    } else {
        format!("{n} · {gets}")
    }
}

/// Under a thumbnail: "pasted" or the image size.
fn shot_meta(a: &Attachment) -> String {
    if a.pasted {
        "pasted".into()
    } else {
        format!("{}×{}", a.width, a.height)
    }
}

/// A run's source item from the form: the title, then the description.
fn run_input(title: &str, description: &str) -> String {
    let (title, description) = (title.trim(), description.trim());
    if description.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{description}")
    }
}

/// A pipeline's steps in the order a run meets them, for the preview: each step after
/// everything that can lead to it (the longest path from the source; edges that loop back,
/// like a "send back", are ignored). The first step gets the task, gates wait for you, `×N`
/// marks parallel variants.
fn pipeline_steps(g: &PipelineGraph) -> Vec<(String, ChipKind)> {
    use std::collections::HashMap;
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        OnPath,
        Done,
    }
    fn visit<'a>(
        g: &'a PipelineGraph,
        id: &'a str,
        marks: &mut HashMap<&'a str, Mark>,
        forward: &mut Vec<(&'a str, &'a str)>,
        post: &mut Vec<&'a str>,
    ) {
        marks.insert(id, Mark::OnPath);
        for e in g.edges.iter().filter(|e| e.from == id) {
            match marks.get(e.to.as_str()) {
                // Back to a step on the current path: a loop, not progress.
                Some(Mark::OnPath) => {}
                Some(Mark::Done) => forward.push((id, e.to.as_str())),
                None => {
                    forward.push((id, e.to.as_str()));
                    visit(g, e.to.as_str(), marks, forward, post);
                }
            }
        }
        marks.insert(id, Mark::Done);
        post.push(id);
    }

    let mut marks = HashMap::new();
    let mut forward = vec![];
    let mut post = vec![];
    for n in g
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Source(_)))
    {
        if !marks.contains_key(n.id.as_str()) {
            visit(g, &n.id, &mut marks, &mut forward, &mut post);
        }
    }
    // Reverse post-order is a topological order of the loop-free graph.
    let topo: Vec<&str> = post.into_iter().rev().collect();
    let mut depth: HashMap<&str, usize> = HashMap::new();
    for &u in &topo {
        let d = depth.get(u).copied().unwrap_or(0);
        for &(_, v) in forward.iter().filter(|(f, _)| *f == u) {
            let dv = depth.entry(v).or_insert(0);
            *dv = (*dv).max(d + 1);
        }
    }
    let mut nodes: Vec<(usize, usize, &harness_core::Node)> = topo
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let n = g.nodes.iter().find(|n| n.id == *id)?;
            (!matches!(n.kind, NodeKind::Source(_)))
                .then(|| (depth.get(id).copied().unwrap_or(0), i, n))
        })
        .collect();
    nodes.sort_by_key(|(d, i, _)| (*d, *i));
    nodes
        .into_iter()
        .enumerate()
        .map(|(i, (_, _, n))| {
            let label = match &n.kind {
                NodeKind::Agent(a) if a.variants > 1 => format!("{} ×{}", n.name, a.variants),
                _ => n.name.clone(),
            };
            let kind = if matches!(n.kind, NodeKind::Gate(_)) {
                ChipKind::Gate
            } else if i == 0 {
                ChipKind::First
            } else {
                ChipKind::Step
            };
            (label, kind)
        })
        .collect()
}

/// Where a pipeline works: "payments-api · new branch, docs", or "No folder".
fn works_in(g: &PipelineGraph) -> String {
    if g.workdirs.is_empty() {
        return "No folder".into();
    }
    g.workdirs
        .iter()
        .map(|w| {
            if w.worktree {
                format!("{} · new branch", w.name)
            } else {
                w.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The text of the "Filled by AI" pill: "Filled by AI · Haiku 5.5 · $0.003".
fn ai_pill_text(model: &str, cost: f64) -> String {
    format!(
        "Filled by AI · {} · ${:.3}",
        Config::short_model(model),
        cost
    )
}

/// The instruction for a draft: what the user asked for; "Rewrite" without one asks for a
/// clearer version; writing from scratch needs none.
fn draft_instruction(asked: &str, current: &str) -> String {
    let asked = asked.trim();
    if !asked.is_empty() {
        asked.to_string()
    } else if !current.trim().is_empty() {
        REWRITE_INSTRUCTION.to_string()
    } else {
        String::new()
    }
}

/// Builds the engine request from the form: trims text, empty values become `None`.
fn build_new_task(
    title: &str,
    description: &str,
    run_id: Option<Id>,
    agent_id: Option<Id>,
    model: &str,
    cwd: &str,
    hold: bool,
) -> NewTask {
    let some = |v: Option<Id>| v.filter(|v| !v.trim().is_empty());
    let cwd = cwd.trim();
    NewTask {
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        run_id: some(run_id),
        agent_id: some(agent_id),
        model: model.trim().to_string(),
        cwd: (!cwd.is_empty()).then(|| cwd.to_string()),
        hold,
        attachments: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_label_mentions_run_and_screenshots() {
        assert_eq!(ai_source_label(None, 0), "from the title");
        assert_eq!(ai_source_label(Some(129), 0), "from the title and run #129");
        assert_eq!(ai_source_label(None, 1), "from the title and 1 screenshot");
        assert_eq!(
            ai_source_label(Some(129), 2),
            "from the title, 2 screenshots and run #129"
        );
    }

    #[test]
    fn previews_and_captions() {
        assert_eq!(preview_text("Worker", 0), "Worker");
        assert_eq!(
            preview_text("Bug triage · new run", 2),
            "Bug triage · new run · 2 screenshots"
        );
        assert_eq!(
            shots_caption(0, Who::Agent),
            "the agent gets them as images"
        );
        assert_eq!(
            shots_caption(2, Who::Pipeline),
            "2 · every step gets them as images"
        );
        assert_eq!(run_input(" Fix it ", ""), "Fix it");
        assert_eq!(run_input("Fix it", "\n Goal \n"), "Fix it\n\nGoal");
    }

    fn node(id: &str, kind: NodeKind) -> harness_core::Node {
        harness_core::Node {
            id: id.into(),
            name: id.into(),
            kind,
            x: 0.0,
            y: 0.0,
        }
    }

    fn edge(f: &str, t: &str) -> harness_core::Edge {
        harness_core::Edge {
            from: f.into(),
            to: t.into(),
            port: None,
        }
    }

    #[test]
    fn steps_follow_the_graph_from_the_source() {
        use harness_core::{AgentNode, GateNode, SourceNode, Workdir};
        let g = PipelineGraph {
            workdirs: vec![
                Workdir {
                    id: "w".into(),
                    name: "payments-api".into(),
                    path: "~/p".into(),
                    worktree: true,
                    base_branch: "main".into(),
                    branch_prefix: String::new(),
                    setup: String::new(),
                    shell: String::new(),
                    env: Default::default(),
                },
                Workdir {
                    id: "d".into(),
                    name: "docs".into(),
                    path: "~/d".into(),
                    worktree: false,
                    base_branch: "main".into(),
                    branch_prefix: String::new(),
                    setup: String::new(),
                    shell: String::new(),
                    env: Default::default(),
                },
            ],
            // Listed out of order on purpose: the preview follows the edges.
            nodes: vec![
                node("Reviewer", NodeKind::Agent(AgentNode::default())),
                node("src", NodeKind::Source(SourceNode::default())),
                node(
                    "Fixer",
                    NodeKind::Agent(AgentNode {
                        variants: 3,
                        ..Default::default()
                    }),
                ),
                node("Router", NodeKind::Agent(AgentNode::default())),
                node("Review gate", NodeKind::Gate(GateNode::default())),
            ],
            edges: vec![
                edge("Fixer", "Review gate"),
                edge("src", "Router"),
                edge("Router", "Fixer"),
                edge("Review gate", "Reviewer"),
                // A loop back doesn't repeat a step.
                edge("Reviewer", "Fixer"),
            ],
        };
        assert_eq!(
            pipeline_steps(&g),
            vec![
                ("Router".to_string(), ChipKind::First),
                ("Fixer ×3".to_string(), ChipKind::Step),
                ("Review gate".to_string(), ChipKind::Gate),
                ("Reviewer".to_string(), ChipKind::Step),
            ]
        );
        assert_eq!(works_in(&g), "payments-api · new branch, docs");

        // A decision that can skip ahead: the build still comes after the design steps.
        use harness_core::DecisionNode;
        let g = PipelineGraph {
            workdirs: vec![],
            nodes: vec![
                node("src", NodeKind::Source(SourceNode::default())),
                node("Needs design?", NodeKind::Decision(DecisionNode::default())),
                node("Designer", NodeKind::Agent(AgentNode::default())),
                node("Pick", NodeKind::Gate(GateNode::default())),
                node("Build", NodeKind::Agent(AgentNode::default())),
                node("Open PR", NodeKind::Agent(AgentNode::default())),
            ],
            edges: vec![
                edge("src", "Needs design?"),
                edge("Needs design?", "Build"),
                edge("Needs design?", "Designer"),
                edge("Designer", "Pick"),
                edge("Pick", "Build"),
                edge("Build", "Open PR"),
            ],
        };
        let names: Vec<String> = pipeline_steps(&g).into_iter().map(|s| s.0).collect();
        assert_eq!(
            names,
            ["Needs design?", "Designer", "Pick", "Build", "Open PR"]
        );
        assert_eq!(
            works_in(&PipelineGraph {
                workdirs: vec![],
                nodes: vec![],
                edges: vec![],
            }),
            "No folder"
        );
    }

    #[test]
    fn pill_text_uses_short_model_and_three_decimals() {
        assert_eq!(
            ai_pill_text("nr/cc/claude-haiku-5-5", 0.0031),
            "Filled by AI · Haiku 5.5 · $0.003"
        );
        assert_eq!(
            ai_pill_text("claude-haiku-5-5", 0.0),
            "Filled by AI · Haiku 5.5 · $0.000"
        );
    }

    #[test]
    fn instruction_defaults() {
        assert_eq!(draft_instruction("  shorter ", "text"), "shorter");
        assert_eq!(draft_instruction("", "text"), REWRITE_INSTRUCTION);
        assert_eq!(draft_instruction("  ", ""), "");
    }

    #[test]
    fn new_task_is_trimmed_and_empty_means_none() {
        let n = build_new_task(
            "  Fix it ",
            "\n Goal \n",
            Some("".into()),
            Some("  ".into()),
            " ",
            "  ",
            false,
        );
        assert_eq!(n.title, "Fix it");
        assert_eq!(n.description, "Goal");
        assert_eq!(n.run_id, None);
        assert_eq!(n.agent_id, None);
        assert_eq!(n.model, "");
        assert_eq!(n.cwd, None);
        assert!(!n.hold);

        let n = build_new_task(
            "T",
            "",
            Some("run1".into()),
            Some("agent1".into()),
            "m/x",
            " ~/code/app ",
            true,
        );
        assert_eq!(n.run_id.as_deref(), Some("run1"));
        assert_eq!(n.agent_id.as_deref(), Some("agent1"));
        assert_eq!(n.model, "m/x");
        assert_eq!(n.cwd.as_deref(), Some("~/code/app"));
        assert!(n.hold);
    }
}
