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
//! description input); nothing runs until the task is created (`Engine::create_task`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    pulsating_between, Animation, AnimationExt as _, EventEmitter, FocusHandle, KeyDownEvent,
};
use gpui_component::input::{Input, InputState};
use gpui_component::select::SelectState;
use harness_engine::tasks::{Draft, DraftRequest, NewTask};

use super::prelude::*;
use crate::ui::input::{self, InputEvent};
use crate::ui::select::{self, Options, SelectEvent, SelectOption};

/// Instruction sent for "Rewrite" when the ask box is empty.
const REWRITE_INSTRUCTION: &str = "Rewrite it more clearly";

/// What the composer tells its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewTaskEvent {
    /// The task was created (id of the new task).
    Created(Id),
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
    focus: FocusHandle,

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

        NewTaskComposer {
            title,
            description,
            ask,
            cwd,
            agent_select,
            run_select,
            model_select,
            focus: cx.focus_handle(),
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

    /// "Worker" or the chosen agent's name (for previews).
    pub fn agent_label(&self) -> SharedString {
        self.agent_id
            .as_ref()
            .and_then(|id| self.agents.iter().find(|a| &a.0 == id))
            .map(|a| SharedString::from(a.1.clone()))
            .unwrap_or_else(|| "Worker".into())
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
            (agents, runs, engine.models())
        });
        self.load = Some(cx.spawn_in(window, async move |this, cx| {
            let (agents, runs, models) = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.agents = agents;
                this.runs = runs;
                this.models = models;
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
        self.source = ai_source_label(self.run_number());

        let req = DraftRequest {
            title,
            run_id: self.run_id.clone(),
            current,
            instruction,
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
        let n = build_new_task(
            &title,
            &self.description.read(cx).value(),
            self.run_id.clone(),
            self.agent_id.clone(),
            &self.model,
            &self.cwd.read(cx).value(),
            self.hold,
        );
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
                    state::toast(ToastKind::Success, format!("Created {label}"), cx);
                    cx.emit(NewTaskEvent::Created(task.id));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.cancel_draft();
        cx.emit(NewTaskEvent::Closed);
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
            .child(ask)
    }

    fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let hold = self.hold;
        let starts = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div().flex_1().child(
                    Button::new("nt-start", "When a slot frees")
                        .small()
                        .full_width()
                        .when(!hold, |b| b.secondary().selected(true))
                        .when(hold, |b| b.ghost())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hold = false;
                            cx.notify();
                        })),
                ),
            )
            .child(
                div().flex_1().child(
                    Button::new("nt-hold", "Hold")
                        .small()
                        .full_width()
                        .when(hold, |b| b.secondary().selected(true))
                        .when(!hold, |b| b.ghost())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hold = true;
                            cx.notify();
                        })),
                ),
            );
        div()
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
            .child(row(
                "Who does it",
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

    fn render_foot(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let blocked = self.creating || self.title.read(cx).value().trim().is_empty();
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
            .child(
                div()
                    .text_label()
                    .fg_muted()
                    .child("The AI only drafts the text. Nothing runs until you create it."),
            )
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
                Button::new("nt-create", "Create task")
                    .primary()
                    .small()
                    .kbd("⌘↵")
                    .disabled(blocked)
                    .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
            )
    }
}

impl Render for NewTaskComposer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_desc = !self.description.read(cx).value().trim().is_empty();
        div()
            .track_focus(&self.focus)
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
                    .child(self.render_options(cx)),
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

/// What the AI read, shown after "Filled by AI": "from the title and run #129".
fn ai_source_label(run_number: Option<u64>) -> String {
    match run_number {
        Some(n) => format!("from the title and run #{n}"),
        None => "from the title".to_string(),
    }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_label_mentions_run() {
        assert_eq!(ai_source_label(None), "from the title");
        assert_eq!(ai_source_label(Some(129)), "from the title and run #129");
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
