//! Pipeline editor: any pipeline of `crates/pipeline` (`PipelineDef`: workdirs and an
//! ordered list of steps) drawn as a node canvas. The header switches, creates, renames and
//! deletes pipelines; the toolbar adds steps; the inspector edits the selected step, the
//! pipeline or its workdirs; the run panel shows the runs of this pipeline and its problems.
//! Every change is saved right away through `Pipelines::save_pipeline`.
//!
//! Test hooks: `HARNESS_EDITOR_PIPELINE=<id>` opens a pipeline, `HARNESS_EDITOR_NODE=<step id>`
//! selects a step, `HARNESS_EDITOR_MENU=add|new|switch|workdir|tools|skill` opens a menu,
//! `HARNESS_EDITOR_TAB=workdirs|pipeline|problems` opens a tab,
//! `HARNESS_EDITOR_WORKDIR=<id>` expands a workdir and `HARNESS_EDITOR_SCROLL=<px>` scrolls the
//! page down.

mod canvas;
mod header;
mod inspector;
mod layout;
mod menu;
mod ops;
mod run_panel;
mod snap;
mod step_fields;
mod widgets;
mod workdirs;

use gpui::{
    App, Context, Entity, Focusable, IntoElement, MouseButton, Render, ScrollHandle, SharedString,
    Subscription, Window, anchored, deferred, div, point, prelude::*, px,
};
use pipeline::{PipelineDef, Pipelines, StepDef, WorkdirDef};
use ui::{Button, TextInput, TextInputEvent, theme::*};

use snap::Snap;

/// Which popup menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    /// Pipeline switcher in the header.
    Switch,
    /// "New pipeline".
    New,
    /// "Add step".
    Add,
    /// The selected step's workdir.
    Workdir,
    Tools,
    DelegateTools,
    OnNo,
    BackTo,
    Skill,
    DelegateSkill,
}

/// Inspector tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Step,
    Pipeline,
    Workdirs,
}

/// A destructive action waiting for "Delete" in an inline confirm strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Confirm {
    Pipeline,
    Step(String),
    Workdir(String),
}

/// Text fields; each is saved on Enter or when it loses focus, Esc reverts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    PipeName,
    PipeDesc,
    StepName,
    Model,
    Instructions,
    Question,
    Files,
    Command,
    Timeout,
    DelegateName,
    DelegateModel,
    DelegateInstructions,
    WdName,
    WdBase,
    WdPrefix,
    WdSetup,
    /// `KEY=value` to add to the expanded workdir's environment.
    EnvAdd,
}

impl Field {
    const ALL: [Field; 17] = [
        Field::PipeName,
        Field::PipeDesc,
        Field::StepName,
        Field::Model,
        Field::Instructions,
        Field::Question,
        Field::Files,
        Field::Command,
        Field::Timeout,
        Field::DelegateName,
        Field::DelegateModel,
        Field::DelegateInstructions,
        Field::WdName,
        Field::WdBase,
        Field::WdPrefix,
        Field::WdSetup,
        Field::EnvAdd,
    ];

    fn placeholder(self) -> &'static str {
        match self {
            Field::PipeName => "Pipeline name",
            Field::PipeDesc => "What this pipeline is for",
            Field::StepName => "Step name",
            Field::Model | Field::DelegateModel => "provider/model",
            Field::Instructions | Field::DelegateInstructions => "What the agent does and how",
            Field::Question => "A yes/no question about the issue",
            Field::Files => "**/*.html",
            Field::Command => "cargo test",
            Field::Timeout => "600",
            Field::DelegateName => "Coder",
            Field::WdName => "Workdir name",
            Field::WdBase => "main",
            Field::WdPrefix => "none",
            Field::WdSetup => "Command (empty = none)",
            Field::EnvAdd => "KEY=value, Enter to add",
        }
    }

    /// Shown as a tall wrapped box that switches to the input while editing.
    pub(crate) fn multiline(self) -> bool {
        matches!(
            self,
            Field::PipeDesc | Field::Instructions | Field::Question | Field::Command | Field::DelegateInstructions
        )
    }

    fn index(self) -> usize {
        Field::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

/// The single-line input shows line breaks as `⏎`.
fn to_input(s: &str) -> String {
    s.replace('\n', "⏎")
}
fn from_input(s: &str) -> String {
    s.replace('⏎', "\n")
}

pub struct PipelineEditorScreen {
    pub(crate) pipes: Entity<Pipelines>,
    /// Pipeline shown (`None` or unknown = the default one).
    pub(crate) pid: Option<String>,
    /// Selected step id.
    pub(crate) sel: Option<String>,
    /// Expanded workdir id in the Workdirs tab.
    pub(crate) wd_sel: Option<String>,
    pub(crate) tab: Tab,
    /// Run panel tab: 0 Runs, 1 Problems.
    pub(crate) run_tab: usize,
    pub(crate) zoom: f32,
    pub(crate) scroll: ScrollHandle,
    /// The whole page (it scrolls when the inspector is long).
    page_scroll: ScrollHandle,
    fitted: bool,
    pub(crate) menu: Option<Menu>,
    pub(crate) confirm: Option<Confirm>,
    /// Multi-line field being edited (shows its input instead of the wrapped text).
    pub(crate) long_edit: Option<Field>,
    /// One input per `Field`, in `Field::ALL` order.
    inputs: Vec<Entity<TextInput>>,
    /// The definition (and selection) the inputs were last loaded from.
    synced: Option<PipelineDef>,
    /// Last save / folder picker / delete error.
    pub(crate) error: Option<SharedString>,
    /// Skills installed on this machine (for "Add skill").
    pub(crate) skills: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl PipelineEditorScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pipes = pipeline::global(cx).unwrap_or_else(|| pipeline::init(cx));
        let mut subs = vec![cx.observe(&pipes, |this, _, cx| {
            this.follow_def(cx);
            cx.notify();
        })];
        // Other screens open a pipeline with `ui::nav::open_pipeline`.
        if let Some(nav) = ui::nav::nav_entity(cx) {
            subs.push(cx.observe(&nav, |this, _, cx| {
                if let Some(id) = ui::nav::take_pipeline_focus(cx)
                    && this.pipes.read(cx).pipeline(&id).is_some()
                {
                    this.switch_pipeline(id.to_string(), cx);
                }
            }));
        }
        let inputs: Vec<Entity<TextInput>> = Field::ALL
            .iter()
            .map(|f| cx.new(|cx| TextInput::new(f.placeholder(), cx)))
            .collect();
        for (field, input) in Field::ALL.into_iter().zip(&inputs) {
            subs.push(cx.subscribe_in(
                input,
                window,
                move |this, input, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Submit => {
                        let text = input.read(cx).text().to_string();
                        this.commit(field, text, cx);
                        this.end_long_edit(field, cx);
                        if field != Field::EnvAdd {
                            window.blur();
                        }
                    }
                    TextInputEvent::Blur => {
                        let text = input.read(cx).text().to_string();
                        this.commit(field, text, cx);
                        this.end_long_edit(field, cx);
                    }
                    TextInputEvent::Cancel => {
                        this.revert(field, cx);
                        this.end_long_edit(field, cx);
                        window.blur();
                    }
                    _ => {}
                },
            ));
        }

        let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        let sel = env("HARNESS_EDITOR_NODE");
        let tab = match env("HARNESS_EDITOR_TAB").as_deref() {
            Some("workdirs") => Tab::Workdirs,
            Some("pipeline") => Tab::Pipeline,
            _ if sel.is_some() => Tab::Step,
            _ => Tab::Pipeline,
        };
        let menu = match env("HARNESS_EDITOR_MENU").as_deref() {
            Some("add") => Some(Menu::Add),
            Some("new") => Some(Menu::New),
            Some("switch") => Some(Menu::Switch),
            Some("workdir") => Some(Menu::Workdir),
            Some("tools") => Some(Menu::Tools),
            Some("skill") => Some(Menu::Skill),
            Some("onno") => Some(Menu::OnNo),
            _ => None,
        };

        let mut this = Self {
            pipes,
            pid: ui::nav::take_pipeline_focus(cx)
                .map(|s| s.to_string())
                .or_else(|| env("HARNESS_EDITOR_PIPELINE")),
            sel,
            wd_sel: env("HARNESS_EDITOR_WORKDIR"),
            tab,
            run_tab: usize::from(env("HARNESS_EDITOR_TAB").as_deref() == Some("problems")),
            zoom: 0.8,
            scroll: ScrollHandle::new(),
            page_scroll: ScrollHandle::new(),
            fitted: false,
            menu,
            confirm: None,
            long_edit: None,
            inputs,
            synced: None,
            error: None,
            skills: snap::installed_skills(),
            _subscriptions: subs,
        };
        this.load_inputs(cx);
        this
    }

    // ── The definition ──────────────────────────────────────────────────

    /// The pipeline shown.
    pub(crate) fn def(&self, cx: &App) -> Option<PipelineDef> {
        let p = self.pipes.read(cx);
        self.pid
            .as_deref()
            .and_then(|id| p.pipeline(id))
            .or(p.default_pipeline())
            .cloned()
    }

    pub(crate) fn sel_step<'a>(&self, def: &'a PipelineDef) -> Option<&'a StepDef> {
        self.sel.as_deref().and_then(|id| def.step(id))
    }

    pub(crate) fn sel_workdir<'a>(&self, def: &'a PipelineDef) -> Option<&'a WorkdirDef> {
        self.wd_sel.as_deref().and_then(|id| def.workdir(id))
    }

    /// Change the shown pipeline and save it; keeps the error to show.
    pub(crate) fn edit_def(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut PipelineDef)) {
        let Some(mut def) = self.def(cx) else { return };
        let before = def.clone();
        f(&mut def);
        if def == before {
            return;
        }
        let path = def.path();
        match self.pipes.update(cx, |p, cx| p.save_pipeline(def, cx)) {
            Ok(()) => self.error = None,
            Err(e) => {
                self.error = Some(format!("Could not save {}: {e:#}", pipeline::def::tilde(&path)).into())
            }
        }
        cx.notify();
    }

    // ── Text fields ─────────────────────────────────────────────────────

    pub(crate) fn input(&self, f: Field) -> Entity<TextInput> {
        self.inputs[f.index()].clone()
    }

    fn value_of(&self, f: Field, def: Option<&PipelineDef>) -> String {
        let Some(def) = def else { return String::new() };
        let step = self.sel_step(def);
        let dlg = step.and_then(|s| s.delegate.as_ref());
        let wd = self.sel_workdir(def);
        let s = |g: fn(&StepDef) -> String| step.map(g).unwrap_or_default();
        let w = |g: fn(&WorkdirDef) -> String| wd.map(g).unwrap_or_default();
        match f {
            Field::PipeName => def.name.clone(),
            Field::PipeDesc => def.description.clone(),
            Field::StepName => s(|s| s.name.clone()),
            Field::Model => s(|s| s.model.clone()),
            Field::Instructions => s(|s| s.instructions.clone()),
            Field::Question => s(|s| s.question.clone()),
            Field::Files => s(|s| s.files.clone()),
            Field::Command => s(|s| s.command.clone()),
            Field::Timeout => s(|s| s.timeout_secs.to_string()),
            Field::DelegateName => dlg.map(|d| d.name.clone()).unwrap_or_default(),
            Field::DelegateModel => dlg.map(|d| d.model.clone()).unwrap_or_default(),
            Field::DelegateInstructions => dlg.map(|d| d.instructions.clone()).unwrap_or_default(),
            Field::WdName => w(|w| w.name.clone()),
            Field::WdBase => w(|w| w.base_branch.clone()),
            Field::WdPrefix => w(|w| w.branch_prefix.clone()),
            Field::WdSetup => w(|w| w.setup.clone()),
            Field::EnvAdd => String::new(),
        }
    }

    /// Put the definition's values into every input (after a selection change).
    pub(crate) fn load_inputs(&mut self, cx: &mut App) {
        let def = self.def(cx);
        for f in Field::ALL {
            let v = to_input(&self.value_of(f, def.as_ref()));
            self.input(f).update(cx, |i, cx| i.set_text(v, cx));
        }
        self.synced = def;
    }

    /// The definition changed (saved here or elsewhere): update inputs the user has not
    /// edited.
    fn follow_def(&mut self, cx: &mut App) {
        let def = self.def(cx);
        if def == self.synced {
            return;
        }
        if def.as_ref().map(|d| &d.id) != self.synced.as_ref().map(|d| &d.id) {
            self.load_inputs(cx);
            return;
        }
        for f in Field::ALL {
            let (before, now) = (self.value_of(f, self.synced.as_ref()), self.value_of(f, def.as_ref()));
            let input = self.input(f);
            if before != now && from_input(input.read(cx).text()) == before {
                input.update(cx, |i, cx| i.set_text(to_input(&now), cx));
            }
        }
        self.synced = def;
    }

    fn revert(&mut self, f: Field, cx: &mut Context<Self>) {
        let v = to_input(&self.value_of(f, self.def(cx).as_ref()));
        self.input(f).update(cx, |i, cx| i.set_text(v, cx));
    }

    /// Save edits still sitting in the inputs (before the selection changes).
    pub(crate) fn commit_pending(&mut self, cx: &mut Context<Self>) {
        let def = self.def(cx);
        for f in Field::ALL {
            let text = from_input(self.input(f).read(cx).text());
            if f != Field::EnvAdd && text.trim() != self.value_of(f, def.as_ref()).trim() {
                self.commit(f, text, cx);
            }
        }
        self.long_edit = None;
    }

    fn reject(&mut self, f: Field, msg: &str, cx: &mut Context<Self>) {
        self.error = Some(msg.to_string().into());
        self.revert(f, cx);
        cx.notify();
    }

    fn commit(&mut self, f: Field, text: String, cx: &mut Context<Self>) {
        let raw = from_input(&text);
        let t = raw.trim().to_string();
        let Some(def) = self.def(cx) else { return };
        if t == self.value_of(f, Some(&def)).trim() && f != Field::EnvAdd {
            return;
        }
        let step = self.sel.clone();
        let wd = self.wd_sel.clone();
        let uses_model = self.sel_step(&def).is_some_and(|s| s.kind.uses_model());
        let required = match f {
            Field::PipeName | Field::StepName | Field::DelegateName | Field::WdName => true,
            Field::Model => uses_model,
            Field::DelegateModel => true,
            _ => false,
        };
        if required && t.is_empty() {
            return self.reject(f, "That field cannot be empty; the old value is kept.", cx);
        }
        let edit_step = move |def: &mut PipelineDef, g: &dyn Fn(&mut StepDef)| {
            if let Some(s) = step.as_deref().and_then(|id| def.steps.iter_mut().find(|s| s.id == id)) {
                g(s);
            }
        };
        let edit_wd = move |def: &mut PipelineDef, g: &dyn Fn(&mut WorkdirDef)| {
            if let Some(w) = wd.as_deref().and_then(|id| def.workdirs.iter_mut().find(|w| w.id == id)) {
                g(w);
            }
        };
        match f {
            Field::PipeName => self.edit_def(cx, |d| d.name = t),
            Field::PipeDesc => self.edit_def(cx, |d| d.description = t),
            Field::StepName => self.edit_def(cx, |d| edit_step(d, &|s| s.name = t.clone())),
            Field::Model => self.edit_def(cx, |d| edit_step(d, &|s| s.model = t.clone())),
            Field::Instructions => self.edit_def(cx, |d| edit_step(d, &|s| s.instructions = t.clone())),
            Field::Question => self.edit_def(cx, |d| edit_step(d, &|s| s.question = t.clone())),
            Field::Files => self.edit_def(cx, |d| edit_step(d, &|s| s.files = t.clone())),
            Field::Command => self.edit_def(cx, |d| edit_step(d, &|s| s.command = t.clone())),
            Field::Timeout => match t.parse::<u64>() {
                Ok(v) if v > 0 => self.edit_def(cx, |d| edit_step(d, &|s| s.timeout_secs = v)),
                _ => self.reject(f, "The timeout is a number of seconds, like 600.", cx),
            },
            Field::DelegateName | Field::DelegateModel | Field::DelegateInstructions => {
                self.edit_def(cx, |d| {
                    edit_step(d, &|s| {
                        if let Some(dl) = s.delegate.as_mut() {
                            match f {
                                Field::DelegateName => dl.name = t.clone(),
                                Field::DelegateModel => dl.model = t.clone(),
                                _ => dl.instructions = t.clone(),
                            }
                        }
                    })
                })
            }
            Field::WdName => self.edit_def(cx, |d| edit_wd(d, &|w| w.name = t.clone())),
            Field::WdBase => self.edit_def(cx, |d| edit_wd(d, &|w| w.base_branch = t.clone())),
            Field::WdPrefix => self.edit_def(cx, |d| edit_wd(d, &|w| w.branch_prefix = t.clone())),
            Field::WdSetup => self.edit_def(cx, |d| edit_wd(d, &|w| w.setup = t.clone())),
            Field::EnvAdd => {
                if t.is_empty() {
                    return;
                }
                let Some((k, v)) = t.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                else {
                    self.error = Some("Write the variable as KEY=value.".into());
                    cx.notify();
                    return;
                };
                if k.is_empty() || k.contains(char::is_whitespace) {
                    self.error = Some("The variable name cannot be empty or contain spaces.".into());
                    cx.notify();
                    return;
                }
                self.edit_def(cx, |d| {
                    edit_wd(d, &|w| {
                        w.env.insert(k.clone(), v.clone());
                    })
                });
                self.input(f).update(cx, |i, cx| i.set_text("", cx));
            }
        }
    }

    /// Show the input of a multi-line field and focus it.
    pub(crate) fn start_long_edit(&mut self, f: Field, window: &mut Window, cx: &mut Context<Self>) {
        self.long_edit = Some(f);
        let input = self.input(f);
        window.focus(&input.focus_handle(cx));
        cx.notify();
    }

    fn end_long_edit(&mut self, f: Field, cx: &mut Context<Self>) {
        if self.long_edit == Some(f) {
            self.long_edit = None;
            cx.notify();
        }
    }

    pub(crate) fn focus_field(&mut self, f: Field, window: &mut Window, cx: &mut Context<Self>) {
        if f.multiline() {
            self.start_long_edit(f, window, cx);
        } else {
            window.focus(&self.input(f).focus_handle(cx));
        }
    }

    // ── Selection, menus, zoom ──────────────────────────────────────────

    pub(crate) fn select(&mut self, step: Option<String>, cx: &mut Context<Self>) {
        if self.sel == step {
            if step.is_some() && self.tab != Tab::Step {
                self.tab = Tab::Step;
                cx.notify();
            }
            return;
        }
        self.commit_pending(cx);
        self.sel = step;
        self.tab = if self.sel.is_some() { Tab::Step } else { Tab::Pipeline };
        self.menu = None;
        self.confirm = None;
        self.load_inputs(cx);
        cx.notify();
    }

    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.commit_pending(cx);
            self.tab = tab;
            self.menu = None;
            self.confirm = None;
            cx.notify();
        }
    }

    pub(crate) fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) { None } else { Some(menu) };
        if matches!(self.menu, Some(Menu::Skill | Menu::DelegateSkill)) {
            self.skills = snap::installed_skills();
        }
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn step_zoom(&mut self, dz: f32, cx: &mut Context<Self>) {
        self.zoom = (((self.zoom + dz) * 100.).round() / 100.).clamp(layout::ZOOM_MIN, layout::ZOOM_MAX);
        cx.notify();
    }

    /// The largest zoom (≤ 100%) at which the whole pipeline fits the tallest canvas.
    pub(crate) fn fit(&mut self, cx: &mut Context<Self>) {
        let vw = f32::from(self.scroll.bounds().size.width);
        if vw <= 0. {
            return;
        }
        let Some(def) = self.def(cx) else { return };
        let mut z = 1.0f32;
        while z > layout::ZOOM_MIN {
            let l = layout::layout(&def, layout::cols_for(vw, z));
            if l.h * z <= canvas::VIEWPORT_MAX_H - 4. && l.w * z <= vw {
                break;
            }
            z = ((z - 0.05) * 100.).round() / 100.;
        }
        self.zoom = z.max(layout::ZOOM_MIN);
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    /// Canvas viewport width (last frame), for the column count.
    pub(crate) fn view_w(&self) -> f32 {
        let w = f32::from(self.scroll.bounds().size.width);
        if w > 0. { w } else { 1040. }
    }
}

impl Render for PipelineEditorScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.fitted {
            // Fit the canvas once its viewport has a size (after the first frame).
            self.fitted = true;
            cx.on_next_frame(window, |this, _, cx| {
                this.fit(cx);
                // Test hook: scroll the page down (screenshots of the lower inspector).
                if let Some(y) = std::env::var("HARNESS_EDITOR_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
                    this.page_scroll.set_offset(point(px(0.), px(-y)));
                }
            });
        }
        let snap = Snap::read(self.pipes.read(cx), self.pid.as_deref());
        // A step or workdir that no longer exists (deleted elsewhere) is unselected.
        if let Some(def) = &snap.def {
            if self.sel.as_deref().is_some_and(|id| def.step(id).is_none()) {
                self.sel = None;
            }
            if self.wd_sel.as_deref().is_some_and(|id| def.workdir(id).is_none()) {
                self.wd_sel = None;
            }
        }
        let body = match &snap.def {
            None => self.render_empty(cx).into_any_element(),
            // The header stays put; the main column and the inspector scroll on their own.
            Some(def) => div()
                .size_full()
                .flex()
                .child(
                    div()
                        .id("editor-main-scroll")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.page_scroll)
                        .border_r_1()
                        .border_color(LINE)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_none()
                                .w_full()
                                .child(self.render_toolbar(def, cx))
                                .child(self.render_canvas(def, &snap, cx))
                                .child(self.render_run_panel(def, &snap, cx)),
                        ),
                )
                .child(self.render_inspector(def, &snap, window, cx))
                .into_any_element(),
        };
        div()
            .id("pipeline-editor")
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(TEXT)
            .child(div().flex_none().w_full().child(self.render_header(&snap, cx)))
            .child(
                div()
                    .id("editor-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(snap.def.is_none(), |d| d.overflow_y_scroll())
                    .child(body),
            )
            .when(self.menu.is_some(), |d| {
                // Click-away backdrop below the open menu.
                let vp = window.viewport_size();
                d.child(
                    deferred(
                        anchored().position(point(px(0.), px(0.))).child(
                            div()
                                .id("menu-backdrop")
                                .w(vp.width)
                                .h(vp.height)
                                .occlude()
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_menu(cx))),
                        ),
                    )
                    .with_priority(1),
                )
            })
    }
}

impl PipelineEditorScreen {
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .py(px(120.))
            .child(
                div()
                    .size(px(40.))
                    .rounded(px(RADIUS_LG))
                    .bg(ACCENT_SOFT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::icon("pipelines", 20., ACCENT_TEXT)),
            )
            .child(div().text_size(px(TEXT_HEADING)).font_weight(gpui::FontWeight::MEDIUM).child("No pipelines yet"))
            .child(
                div()
                    .max_w(px(420.))
                    .text_center()
                    .text_color(TEXT_MUTED)
                    .child("A pipeline is a list of steps that issues run through: agents, decisions, your approval, commands, merges and pull requests."),
            )
            .child(
                Button::new("empty-new", "New pipeline")
                    .primary()
                    .icon("plus")
                    .on_click(cx.listener(|this, _, window, cx| this.new_pipeline(None, window, cx))),
            )
    }
}
