//! Pipelines: the list of pipelines and the graph editor (Graph / Runs / Settings).
//! Design: design/screens/pipeline-editor.html.
//!
//! The editor keeps a local copy of the pipeline's editing graph (draft if any). Every edit
//! changes that copy and, debounced, saves it as the draft (`Engine::save_draft`); `Deploy`
//! makes the draft live. Store reloads only replace the local copy when nothing is unsaved.

mod data;
mod edit;
mod graph;
mod inspector;
mod runs;
mod settings;

use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui::{Animation, AnimationExt as _, MouseButton};
use gpui_component::input::InputState;
use harness_core::{Pipeline, PipelineGraph};
use harness_engine::Template;

use super::prelude::*;
use crate::ui::input::InputEvent;
use data::{Detail, ListRow};

/// Debounce between the last edit and the draft save.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Graph,
    Runs,
    Settings,
}

/// Models and skills for pickers (engine calls, loaded off the UI thread).
#[derive(Default)]
struct Catalog {
    models: Vec<String>,
    skills: Vec<String>,
    default_model: String,
}

/// Local editing state of the open pipeline.
struct Editor {
    pipeline_id: Id,
    graph: PipelineGraph,
    name: String,
    description: String,
    color: u32,
    selected: Option<Id>,
    /// Bumps on every local edit; `saved_seq` catches up when a save lands.
    edit_seq: u64,
    saved_seq: u64,
    saving: bool,
    /// When the last save started: older store snapshots are ignored.
    save_started: Option<DateTime<Utc>>,
    /// Adopt the next store snapshot even if it looks older (after Discard).
    force_adopt: bool,
    debounce: Option<Task<()>>,
    /// Bumps when the inspector must be rebuilt (structure changed).
    rev: u64,
    /// Bumps when the settings form must be rebuilt.
    settings_rev: u64,
    /// Bumps on every selection (keys the ring / inspector animations).
    sel_seq: u64,
    inspector: Option<inspector::Inspector>,
    settings: Option<settings::SettingsForm>,
}

impl Editor {
    fn new(p: &Pipeline) -> Self {
        Editor {
            pipeline_id: p.id.clone(),
            graph: p.editing().clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            color: p.color,
            selected: None,
            edit_seq: 0,
            saved_seq: 0,
            saving: false,
            save_started: None,
            force_adopt: false,
            debounce: None,
            rev: 0,
            settings_rev: 0,
            sel_seq: 0,
            inspector: None,
            settings: None,
        }
    }

    fn unsaved(&self) -> bool {
        self.edit_seq > self.saved_seq || self.saving
    }
}

/// The "New pipeline" dialog.
struct NewDialog {
    name: Entity<InputState>,
    template: usize,
    _sub: Subscription,
}

/// The "Run" popover.
struct RunDialog {
    input: Entity<InputState>,
    starting: bool,
    _sub: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    Archive,
    Delete,
}

pub struct PipelinesScreen {
    /// The pipeline in the route (`None` = the list).
    route: Option<Id>,
    list: Option<Vec<ListRow>>,
    list_stale: bool,
    list_load: Option<Task<()>>,
    detail: Option<Detail>,
    detail_missing: bool,
    detail_load: Option<Task<()>>,
    catalog: Catalog,
    catalog_load: Option<Task<()>>,
    templates: Option<Vec<Template>>,
    templates_load: Option<Task<()>>,
    ed: Option<Editor>,
    tab: Tab,
    new_dialog: Option<NewDialog>,
    run_dialog: Option<RunDialog>,
    expanded_run: Option<Id>,
    confirm: Option<Confirm>,
    search: Entity<InputState>,
    query: String,
    _subs: Vec<Subscription>,
}

impl PipelinesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::StoreChanged { changes, .. } => this.on_store(changes, cx),
            AppEvent::Navigated(Route::Pipelines { pipeline }) => {
                this.set_route(pipeline.clone(), cx)
            }
            _ => {}
        });
        let search = ui::input::text_input("Find a node or agent", window, cx);
        let search_sub = cx.subscribe_in(&search, window, |this, s, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                this.query = s.read(cx).value().trim().to_lowercase();
                cx.notify();
            }
        });
        let route = match state::route(cx) {
            Route::Pipelines { pipeline } => pipeline,
            _ => None,
        };
        let mut this = PipelinesScreen {
            route: None,
            list: None,
            list_stale: true,
            list_load: None,
            detail: None,
            detail_missing: false,
            detail_load: None,
            catalog: Catalog::default(),
            catalog_load: None,
            templates: None,
            templates_load: None,
            ed: None,
            tab: Tab::Graph,
            new_dialog: None,
            run_dialog: None,
            expanded_run: None,
            confirm: None,
            search,
            query: String::new(),
            _subs: vec![sub, search_sub],
        };
        this.reload_catalog(cx);
        this.route = route;
        match &this.route {
            None => this.reload_list(cx),
            Some(_) => this.reload_detail(cx),
        }
        this
    }

    // ---- loading -------------------------------------------------------------

    fn on_store(&mut self, changes: &[Change], cx: &mut Context<Self>) {
        let external = changes.iter().any(|c| matches!(c, Change::External));
        let list_relevant = external
            || changes.iter().any(|c| {
                matches!(
                    c,
                    Change::Pipeline(_) | Change::Run(_) | Change::Task { .. }
                )
            });
        if list_relevant {
            if self.route.is_none() {
                self.reload_list(cx);
            } else {
                self.list_stale = true;
            }
        }
        if let Some(id) = self.route.clone() {
            let relevant = external
                || changes.iter().any(|c| match c {
                    Change::Pipeline(p) => *p == id,
                    // Step updates of runs we don't show can't change this pipeline's numbers.
                    Change::Task { run_id, .. } => run_id.as_ref().is_some_and(|r| {
                        self.detail
                            .as_ref()
                            .is_none_or(|d| d.runs.iter().any(|row| row.run.id == *r))
                    }),
                    Change::Run(_) | Change::Agent(_) | Change::Variant { .. } => true,
                    _ => false,
                });
            if relevant {
                self.reload_detail(cx);
            }
        }
        if external || changes.iter().any(|c| matches!(c, Change::Config)) {
            self.reload_catalog(cx);
        }
    }

    fn reload_list(&mut self, cx: &mut Context<Self>) {
        self.list_stale = false;
        self.list_load = Some(load(
            cx,
            |s| data::list(s),
            |this, rows, _| this.list = Some(rows),
        ));
    }

    fn reload_detail(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.route.clone() else { return };
        self.detail_load = Some(load(
            cx,
            move |s| data::detail(s, &id),
            |this, d, cx| this.apply_detail(d, cx),
        ));
    }

    fn reload_catalog(&mut self, cx: &mut Context<Self>) {
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move {
            let skills = engine
                .skills()
                .into_iter()
                .map(|s| s.name)
                .collect::<Vec<_>>();
            Catalog {
                models: engine.models(),
                skills,
                default_model: engine.config().default_model,
            }
        });
        self.catalog_load = Some(cx.spawn(async move |this, cx| {
            let catalog = job.await;
            let _ = this.update(cx, |this, cx| {
                this.catalog = catalog;
                if let Some(ed) = this.ed.as_mut() {
                    ed.rev += 1;
                }
                cx.notify();
            });
        }));
    }

    fn load_templates(&mut self, cx: &mut Context<Self>) {
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move { engine.templates() });
        self.templates_load = Some(cx.spawn(async move |this, cx| {
            let t = job.await;
            let _ = this.update(cx, |this, cx| {
                this.templates = Some(t);
                cx.notify();
            });
        }));
    }

    /// Applies a fresh detail snapshot; adopts the store's graph when nothing local is unsaved.
    fn apply_detail(&mut self, d: Option<Detail>, _cx: &mut Context<Self>) {
        let Some(d) = d else {
            self.detail = None;
            self.detail_missing = true;
            return;
        };
        self.detail_missing = false;
        match self.ed.as_mut() {
            Some(ed) if ed.pipeline_id == d.pipeline.id => {
                let fresh =
                    ed.force_adopt || ed.save_started.is_none_or(|t| d.pipeline.updated_at >= t);
                if !ed.unsaved() && fresh {
                    ed.force_adopt = false;
                    let p = &d.pipeline;
                    if ed.graph != *p.editing() {
                        let workdirs_changed = ed.graph.workdirs != p.editing().workdirs;
                        ed.graph = p.editing().clone();
                        ed.rev += 1;
                        if workdirs_changed {
                            ed.settings_rev += 1;
                        }
                        if ed
                            .selected
                            .as_deref()
                            .is_some_and(|s| ed.graph.node(s).is_none())
                        {
                            ed.selected = None;
                        }
                    }
                    if ed.name != p.name || ed.description != p.description || ed.color != p.color {
                        ed.name = p.name.clone();
                        ed.description = p.description.clone();
                        ed.color = p.color;
                        ed.settings_rev += 1;
                    }
                }
            }
            _ => self.ed = Some(Editor::new(&d.pipeline)),
        }
        self.detail = Some(d);
    }

    fn set_route(&mut self, pipeline: Option<Id>, cx: &mut Context<Self>) {
        if self.route == pipeline {
            return;
        }
        self.persist(cx);
        self.route = pipeline;
        self.detail = None;
        self.detail_missing = false;
        self.ed = None;
        self.tab = Tab::Graph;
        self.run_dialog = None;
        self.confirm = None;
        self.expanded_run = None;
        match &self.route {
            None => {
                if self.list_stale || self.list.is_none() {
                    self.reload_list(cx);
                }
            }
            Some(_) => self.reload_detail(cx),
        }
        cx.notify();
    }

    // ---- saving --------------------------------------------------------------

    /// Records a local edit and schedules the draft save.
    fn touch(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        ed.edit_seq += 1;
        ed.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.persist(cx));
        }));
        cx.notify();
    }

    /// Saves name/description/color and the graph as the draft, now. One save at a time;
    /// edits made meanwhile are saved right after.
    fn persist(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        ed.debounce = None;
        if ed.saving || ed.edit_seq == ed.saved_seq {
            return;
        }
        ed.saving = true;
        ed.save_started = Some(Utc::now());
        let seq = ed.edit_seq;
        let id = ed.pipeline_id.clone();
        let graph = ed.graph.clone();
        let (name, description, color) =
            (ed.name.trim().to_string(), ed.description.clone(), ed.color);
        let job = state::run_engine(cx, {
            let id = id.clone();
            move |e| {
                // The graph saves even if the name is rejected (e.g. a duplicate).
                e.save_draft(&id, graph)?;
                // While the name field is empty, keep the stored name.
                let name = if name.is_empty() {
                    e.store()
                        .pipeline(&id)
                        .map(|p| p.name)
                        .unwrap_or_default()
                } else {
                    name
                };
                e.update_pipeline_meta(&id, &name, &description, color)
            }
        });
        cx.spawn(async move |this, cx| {
            job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(ed) = this.ed.as_mut().filter(|ed| ed.pipeline_id == id) else {
                    return;
                };
                ed.saving = false;
                ed.saved_seq = ed.saved_seq.max(seq);
                if ed.edit_seq > ed.saved_seq {
                    this.persist(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn deploy(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        ed.debounce = None;
        ed.saved_seq = ed.edit_seq;
        let id = ed.pipeline_id.clone();
        let graph = ed.graph.clone();
        let job = state::run_engine(cx, move |e| {
            e.save_draft(&id, graph)?;
            e.deploy(&id)
        });
        cx.spawn(async move |_, cx| {
            if let Some(v) = job.await {
                let _ =
                    cx.update(|cx| state::toast(ToastKind::Success, format!("Deployed v{v}"), cx));
            }
        })
        .detach();
    }

    fn discard_draft(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        ed.debounce = None;
        ed.saved_seq = ed.edit_seq;
        let id = ed.pipeline_id.clone();
        let job = state::run_engine(cx, move |e| e.discard_draft(&id));
        cx.spawn(async move |this, cx| {
            if job.await.is_some() {
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.ed.as_mut() {
                        ed.force_adopt = true;
                    }
                    this.reload_detail(cx);
                    state::toast(ToastKind::Info, "Draft discarded", cx);
                });
            }
        })
        .detach();
    }

    // ---- actions -------------------------------------------------------------

    fn select_node(&mut self, id: Option<Id>, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        if ed.selected != id {
            ed.selected = id;
            ed.sel_seq += 1;
        }
        cx.notify();
    }

    fn open_new_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = ui::input::text_input("Pipeline name (defaults to the template's)", window, cx);
        let sub = cx.subscribe_in(&name, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.create_pipeline(cx);
            }
        });
        name.update(cx, |s, cx| s.focus(window, cx));
        self.new_dialog = Some(NewDialog {
            name,
            template: 0,
            _sub: sub,
        });
        if self.templates.is_none() {
            self.load_templates(cx);
        }
        cx.notify();
    }

    fn create_pipeline(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.new_dialog else { return };
        let Some(t) = self
            .templates
            .as_ref()
            .and_then(|t| t.get(d.template))
            .cloned()
        else {
            return;
        };
        let name = d.name.read(cx).value().to_string();
        let job = state::run_engine(cx, move |e| e.create_pipeline(&t, &name));
        cx.spawn(async move |this, cx| {
            if let Some(p) = job.await {
                let _ = this.update(cx, |this, cx| {
                    this.new_dialog = None;
                    state::toast(ToastKind::Success, format!("Created {}", p.name), cx);
                    state::navigate(
                        Route::Pipelines {
                            pipeline: Some(p.id),
                        },
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    fn toggle_run_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_dialog.is_some() {
            self.run_dialog = None;
            cx.notify();
            return;
        }
        let input = ui::input::composer_rows(
            "What should this run work on? The first line becomes its title.",
            4,
            12,
            window,
            cx,
        );
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { secondary: true } = ev {
                this.start_run(cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.run_dialog = Some(RunDialog {
            input,
            starting: false,
            _sub: sub,
        });
        cx.notify();
    }

    fn start_run(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.route.clone() else { return };
        let Some(d) = self.run_dialog.as_mut() else {
            return;
        };
        let text = d.input.read(cx).value().trim().to_string();
        if text.is_empty() || d.starting {
            return;
        }
        d.starting = true;
        let job = state::run_engine(cx, move |e| e.start_run(&id, &text, None));
        cx.spawn(async move |this, cx| {
            let run = job.await;
            let _ = this.update(cx, |this, cx| {
                match run {
                    Some(run) => {
                        this.run_dialog = None;
                        this.tab = Tab::Runs;
                        this.expanded_run = Some(run.id.clone());
                        state::toast(
                            ToastKind::Success,
                            format!("Started run #{} · {}", run.number, run.title),
                            cx,
                        );
                        this.reload_detail(cx);
                    }
                    None => {
                        if let Some(d) = this.run_dialog.as_mut() {
                            d.starting = false;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- rendering: list ----------------------------------------------------

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let header = PageHeader::new("Pipelines").action(
            Button::new("pipelines-new", "New pipeline")
                .primary()
                .icon(AppIcon::Plus)
                .on_click(cx.listener(|this, _, window, cx| this.open_new_dialog(window, cx))),
        );
        let content = match &self.list {
            None => body().child(skeleton_rows("pipelines-loading", 4)),
            Some(v) if v.is_empty() => body().child(
                EmptyState::new(
                    AppIcon::Workflow,
                    "No pipelines yet",
                    "A pipeline turns an issue into shipped work: a source, agents, gates and tools wired as a graph. Start from a template.",
                )
                .action(
                    Button::new("pipelines-empty-new", "New pipeline")
                        .primary()
                        .icon(AppIcon::Plus)
                        .on_click(cx.listener(|this, _, window, cx| this.open_new_dialog(window, cx))),
                ),
            ),
            Some(rows) => body().child(
                panel()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S3)
                            .h(px(36.))
                            .px(theme::S4)
                            .border_b_1()
                            .border_color(theme::LINE)
                            .text_label()
                            .fg_muted()
                            .child(div().flex_1().child("Pipeline"))
                            .child(div().w(px(150.)).child("Version"))
                            .child(div().w(px(70.)).text_right().child("Runs"))
                            .child(div().w(px(220.)).child("Last run"))
                            .child(div().w(px(110.)).child("")),
                    )
                    .children(rows.iter().enumerate().map(|(ix, r)| list_row(ix, r, cx))),
            ),
        };
        screen().child(header).child(content).into_any_element()
    }

    // ---- rendering: editor --------------------------------------------------

    fn render_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.detail_missing {
            return screen()
                .child(PageHeader::new("Pipelines"))
                .child(
                    body().child(
                        EmptyState::new(
                            AppIcon::Workflow,
                            "Pipeline not found",
                            "It may have been archived or deleted.",
                        )
                        .action(
                            Button::new("pl-missing-back", "All pipelines").on_click(|_, _, cx| {
                                state::navigate(Route::Pipelines { pipeline: None }, cx)
                            }),
                        ),
                    ),
                )
                .into_any_element();
        }
        let (Some(d), Some(_)) = (&self.detail, &self.ed) else {
            return screen()
                .child(PageHeader::new("Pipelines"))
                .child(body().child(skeleton_rows("pipeline-loading", 5)))
                .into_any_element();
        };
        let total_runs = d.total_runs;
        let header = self.render_header(cx);
        let banner = self.render_banner(cx);
        let tabs = Tabs::new("pipeline-tabs")
            .tab("Graph")
            .tab_count("Runs", total_runs)
            .tab("Settings")
            .selected(match self.tab {
                Tab::Graph => 0,
                Tab::Runs => 1,
                Tab::Settings => 2,
            })
            .on_select(cx.listener(|this, ix: &usize, _, cx| {
                this.tab = match ix {
                    0 => Tab::Graph,
                    1 => Tab::Runs,
                    _ => Tab::Settings,
                };
                this.confirm = None;
                cx.notify();
            }))
            .px(theme::S6);
        let content = match self.tab {
            Tab::Graph => self.render_graph_tab(window, cx),
            Tab::Runs => self.render_runs_tab(cx),
            Tab::Settings => self.render_settings_tab(window, cx),
        };
        screen()
            .relative()
            .child(header)
            .child(tabs)
            .children(banner)
            .child(div().flex_1().min_h_0().flex().child(content))
            .children(self.render_run_dialog(cx))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(d), Some(ed)) = (&self.detail, &self.ed) else {
            return div().into_any_element();
        };
        let p = &d.pipeline;
        let has_draft = ed.graph != p.graph;
        let save_state = if ed.unsaved() {
            "saving…"
        } else if has_draft {
            "autosaved"
        } else {
            "no changes"
        };
        let name: SharedString = if ed.name.trim().is_empty() {
            p.name.clone().into()
        } else {
            ed.name.clone().into()
        };
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(theme::S3)
            .min_h(theme::HEADER_H)
            .px(theme::S6)
            .py(theme::S3)
            .border_b_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .id("pl-crumb")
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_label()
                    .fg_muted()
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme::TEXT))
                    .child("Pipelines")
                    .child(icon_sized(AppIcon::ChevronRight, theme::ICON_SM))
                    .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
            )
            .child(dot_el(theme::rgb_u32(ed.color), px(8.)))
            .child(div().text_heading().fg().child(name))
            .child(
                Badge::new(format!("v{} live", p.version)).leading(dot_el(theme::SUCCESS, px(6.))),
            )
            .when(has_draft, |el| {
                el.child(Badge::new(format!("Draft v{}", p.version + 1)).accent())
            })
            .child(div().text_label().fg_muted().child(format!(
                "edited {} ago · {save_state}",
                fmt_ago(p.updated_at)
            )))
            .child(div().flex_1())
            .child(
                Button::new("pl-run", "Run")
                    .icon(AppIcon::Play)
                    .selected(self.run_dialog.is_some())
                    .tooltip(format!("Start a run of the live v{}", p.version))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_run_dialog(window, cx)),
                    ),
            )
            .child(
                Button::new("pl-deploy", format!("Deploy v{}", p.version + 1))
                    .primary()
                    .icon(AppIcon::Check)
                    .disabled(!has_draft)
                    .on_click(cx.listener(|this, _, _, cx| this.deploy(cx))),
            )
            .into_any_element()
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (d, ed) = (self.detail.as_ref()?, self.ed.as_ref()?);
        let p = &d.pipeline;
        if ed.graph == p.graph {
            return None;
        }
        let diff = edit::diff(&p.graph, &ed.graph);
        let n = diff.node_count();
        let title = if n > 0 {
            format!(
                "Draft v{} · {} node{} changed since v{}",
                p.version + 1,
                n,
                if n == 1 { "" } else { "s" },
                p.version
            )
        } else {
            format!("Draft v{} · changed since v{}", p.version + 1, p.version)
        };
        let mut parts: Vec<String> = Vec::new();
        let names = |v: &[String]| {
            let mut s = v.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if v.len() > 3 {
                s.push_str(&format!(" +{}", v.len() - 3));
            }
            s
        };
        if !diff.added.is_empty() {
            parts.push(format!("added {}", names(&diff.added)));
        }
        if !diff.removed.is_empty() {
            parts.push(format!("removed {}", names(&diff.removed)));
        }
        if !diff.edited.is_empty() {
            parts.push(format!("edited {}", names(&diff.edited)));
        }
        if diff.wiring {
            parts.push("wiring changed".into());
        }
        if diff.workdirs {
            parts.push("workdirs changed".into());
        }
        parts.push(format!("new runs still use v{}", p.version));
        let invalid = harness_engine::pipeline::validate(&ed.graph)
            .err()
            .map(|e| format!("{e:#}"));
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme::S3)
                .px(theme::S6)
                .py(px(10.))
                .border_b_1()
                .border_color(theme::LINE)
                .bg(theme::ACCENT_GLOW)
                .overflow_hidden()
                .child(icon_tile(
                    AppIcon::Workflow,
                    theme::ACCENT_SOFT,
                    theme::ACCENT_TEXT,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_body_strong().fg().child(title))
                        .child(
                            div()
                                .text_label()
                                .fg_muted()
                                .truncate()
                                .child(parts.join(" · ")),
                        )
                        .when_some(invalid, |el, e| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme::S1)
                                    .text_label()
                                    .text_color(theme::WARNING)
                                    .child(icon_sized(AppIcon::Alert, px(12.)))
                                    .child(format!("Can't deploy yet: {e}")),
                            )
                        }),
                )
                .child(
                    Button::new("pl-discard", "Discard draft")
                        .danger()
                        .small()
                        .icon(AppIcon::X)
                        .on_click(cx.listener(|this, _, _, cx| this.discard_draft(cx))),
                )
                .with_animation(
                    "pl-draft-banner",
                    Animation::new(theme::DUR_OVERLAY).with_easing(theme::ease_out),
                    |el, t| el.opacity(t).max_h(px(120. * t)),
                )
                .into_any_element(),
        )
    }

    fn render_run_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dlg = self.run_dialog.as_ref()?;
        let d = self.detail.as_ref()?;
        let p = &d.pipeline;
        let has_draft = self.ed.as_ref().is_some_and(|ed| ed.graph != p.graph);
        let starting = dlg.starting;
        Some(
            div()
                .absolute()
                .top(theme::HEADER_H)
                .right(theme::S6)
                .w(px(440.))
                .occlude()
                .child(
                    card()
                        .bg(theme::SURFACE_RAISED)
                        .border_color(theme::LINE_STRONG)
                        .shadow(vec![gpui::BoxShadow {
                            color: theme::alpha(theme::BG, 0.6),
                            offset: gpui::point(px(0.), px(8.)),
                            blur_radius: px(24.),
                            spread_radius: px(0.),
                        }])
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::S2)
                                .child(
                                    div()
                                        .flex_1()
                                        .text_heading()
                                        .fg()
                                        .child(format!("Run {} v{}", p.name, p.version)),
                                )
                                .child(
                                    IconButton::new("pl-run-close", AppIcon::X)
                                        .small()
                                        .tooltip("Close")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.run_dialog = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(ui::input::composer_field(&dlg.input))
                        .child(div().text_label().fg_muted().child(if has_draft {
                            format!(
                                "Runs use the live v{}. Deploy the draft first to run your edits.",
                                p.version
                            )
                        } else {
                            "The text is the run's input (prompt layer 4).".to_string()
                        }))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(theme::S2)
                                .child(
                                    Button::new("pl-run-cancel", "Cancel")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.run_dialog = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(
                                        "pl-run-start",
                                        if starting { "Starting…" } else { "Start run" },
                                    )
                                    .primary()
                                    .small()
                                    .icon(AppIcon::Play)
                                    .kbd("ctrl ↵")
                                    .disabled(starting)
                                    .on_click(cx.listener(|this, _, _, cx| this.start_run(cx))),
                                ),
                        ),
                )
                .with_animation(
                    "pl-run-dialog",
                    Animation::new(theme::DUR_OVERLAY).with_easing(theme::ease_out),
                    |el, t| el.opacity(t).mt(px(-6. * (1. - t))),
                )
                .into_any_element(),
        )
    }

    fn render_new_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dlg = self.new_dialog.as_ref()?;
        let selected = dlg.template;
        let templates: AnyElement = match &self.templates {
            None => skeleton_rows("pl-templates-loading", 3).into_any_element(),
            Some(t) => div()
                .flex()
                .flex_col()
                .gap(theme::S2)
                .children(t.iter().enumerate().map(|(ix, t)| {
                    let active = ix == selected;
                    div()
                        .id(("pl-template", ix))
                        .flex()
                        .gap(theme::S3)
                        .p(theme::S3)
                        .rounded(theme::RADIUS_LG)
                        .border_1()
                        .border_color(if active {
                            theme::ACCENT_RING
                        } else {
                            theme::LINE
                        })
                        .when(active, |el| el.bg(theme::ACCENT_GLOW))
                        .when(!active, |el| el.hover(|s| s.bg(theme::SURFACE_HOVER)))
                        .cursor_pointer()
                        .child(
                            div()
                                .pt(px(6.))
                                .child(dot_el(theme::rgb_u32(t.color), px(8.))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_body_strong().fg().child(t.name))
                                .child(div().text_label().fg_muted().child(t.description))
                                .child(div().text_label().fg_disabled().child(format!(
                                    "{} nodes · {} workdir{}",
                                    t.graph.nodes.len(),
                                    t.graph.workdirs.len(),
                                    if t.graph.workdirs.len() == 1 { "" } else { "s" }
                                ))),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.new_dialog.as_mut() {
                                d.template = ix;
                            }
                            cx.notify();
                        }))
                }))
                .into_any_element(),
        };
        let can_create = self.templates.as_ref().is_some_and(|t| !t.is_empty());
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .child(
                    div()
                        .id("pl-new-scrim")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(theme::SCRIM)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.new_dialog = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(relative(0.1))
                        .left_0()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(
                            card()
                                .id("pl-new-dialog")
                                .w(px(560.))
                                .max_h(relative(0.8))
                                .overflow_y_scroll()
                                .gap(theme::S4)
                                .p(theme::S5)
                                .bg(theme::SURFACE_RAISED)
                                .border_color(theme::LINE_STRONG)
                                .child(div().text_title().fg().child("New pipeline"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(theme::S2)
                                        .child(label("Name"))
                                        .child(ui::input::field(&dlg.name)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(theme::S2)
                                        .child(label("Start from"))
                                        .child(templates),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(theme::S2)
                                        .child(
                                            Button::new("pl-new-cancel", "Cancel")
                                                .ghost()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.new_dialog = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("pl-new-create", "Create pipeline")
                                                .primary()
                                                .disabled(!can_create)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.create_pipeline(cx)
                                                })),
                                        ),
                                )
                                .with_animation(
                                    "pl-new-dialog-in",
                                    Animation::new(theme::DUR_OVERLAY).with_easing(theme::ease_out),
                                    |el, t| el.opacity(t).mt(px(8. * (1. - t))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// One row of the pipeline list.
fn list_row(ix: usize, r: &ListRow, _cx: &mut Context<PipelinesScreen>) -> impl IntoElement {
    let p = &r.pipeline;
    let id = p.id.clone();
    let last: AnyElement = match r.last {
        Some((status, at, number)) => div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .child(StatusDot::new(status).id(("pl-list-dot", ix)))
            .child(
                div()
                    .text_label()
                    .fg_secondary()
                    .child(format!("#{number} · {}", status.label())),
            )
            .child(
                div()
                    .text_label()
                    .fg_muted()
                    .child(format!("{} ago", fmt_ago(at))),
            )
            .into_any_element(),
        None => div()
            .text_label()
            .fg_disabled()
            .child("Never run")
            .into_any_element(),
    };
    div()
        .id(("pl-list-row", ix))
        .flex()
        .items_center()
        .gap(theme::S3)
        .min_h(px(56.))
        .px(theme::S4)
        .py(theme::S2)
        .border_b_1()
        .border_color(theme::LINE_SUBTLE)
        .cursor_pointer()
        .hover(|s| s.bg(theme::SURFACE_RAISED))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(theme::S3)
                .child(dot_el(theme::rgb_u32(p.color), px(8.)))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_body_strong()
                                .fg()
                                .truncate()
                                .child(p.name.clone()),
                        )
                        .when(!p.description.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_label()
                                    .fg_muted()
                                    .truncate()
                                    .child(p.description.clone()),
                            )
                        }),
                ),
        )
        .child(
            div()
                .w(px(150.))
                .flex()
                .gap(theme::S1)
                .child(Badge::new(format!("v{}", p.version)).mono())
                .when(p.draft.is_some(), |el| {
                    el.child(Badge::new(format!("Draft v{}", p.version + 1)).accent())
                }),
        )
        .child(
            div()
                .w(px(70.))
                .text_right()
                .text_mono()
                .fg_secondary()
                .child(r.runs.to_string()),
        )
        .child(div().w(px(220.)).child(last))
        .child(
            div()
                .w(px(110.))
                .flex()
                .justify_end()
                .gap(theme::S1)
                .when(r.active > 0, |el| {
                    el.child(
                        Badge::new(format!("{} active", r.active)).leading(
                            StatusDot::new(Status::Running)
                                .id(("pl-list-run", ix))
                                .small(),
                        ),
                    )
                })
                .when(r.waiting > 0, |el| {
                    el.child(Badge::new(format!("{} waiting", r.waiting)).warning().dot())
                }),
        )
        .on_click(move |_, _, cx| {
            state::navigate(
                Route::Pipelines {
                    pipeline: Some(id.clone()),
                },
                cx,
            )
        })
        .with_animation(
            ElementId::NamedInteger("pl-list-in".into(), ix as u64),
            Animation::new(theme::DUR_ENTER + Duration::from_millis(20 * ix.min(10) as u64))
                .with_easing(theme::ease_out),
            |el, t| el.opacity(t),
        )
}

impl Render for PipelinesScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.route {
            None => self.render_list(cx),
            Some(_) => self.render_editor(window, cx),
        };
        div()
            .relative()
            .size_full()
            .child(main)
            .children(self.render_new_dialog(cx))
    }
}
