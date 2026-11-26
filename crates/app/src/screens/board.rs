//! Board: every task as a card in Queued / Running / Waiting for you / Done columns.
//! Design: design/screens/board.html.
//!
//! * Data is loaded off the UI thread into plain [`CardView`]s (all strings precomputed);
//!   filters and column layout are applied to that cache, never in `render`.
//! * Drag a card onto another card of the same column to reorder (`Engine::set_priority`),
//!   or onto the empty area under the cards to move it to the end.
//! * Running cards show live elapsed time; a one-second ticker runs only while something runs.
//! * Cards fade/slide in; a card that changes column is a new element there, so it animates in.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use gpui::{Animation, AnimationExt as _, FocusHandle, KeyDownEvent};
use harness_core::{Task as CoreTask, TaskKind};

use super::prelude::*;
use crate::ui::input::{self, InputEvent};
use crate::ui::select::{self, Options, SelectEvent, SelectOption};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;

use super::new_task::{NewTaskComposer, NewTaskEvent};

const COLUMNS: [(Status, &str); 4] = [
    (Status::Queued, "Queued"),
    (Status::Running, "Running"),
    (Status::Waiting, "Waiting for you"),
    (Status::Done, "Done"),
];
/// Cards shown in the Done column before "Show more".
const DONE_PAGE: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum KindFilter {
    All,
    Steps,
    Chats,
    SubAgents,
}

impl KindFilter {
    const ALL: [(KindFilter, &'static str, &'static str); 4] = [
        (KindFilter::All, "all", "All kinds"),
        (KindFilter::Steps, "steps", "Pipeline steps"),
        (KindFilter::Chats, "chats", "Chats"),
        (KindFilter::SubAgents, "sub", "Sub-agents"),
    ];

    fn from_value(v: &str) -> KindFilter {
        Self::ALL
            .iter()
            .find(|(_, k, _)| *k == v)
            .map(|(f, _, _)| *f)
            .unwrap_or(KindFilter::All)
    }

    fn matches(self, c: &CardView) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Steps => {
                c.kind != TaskKind::Chat
                    && c.kind != TaskKind::Spawned
                    && c.kind != TaskKind::Manual
                    && c.run_chip.is_some()
            }
            KindFilter::Chats => c.kind == TaskKind::Chat,
            KindFilter::SubAgents => c.kind == TaskKind::Spawned,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TimeFilter {
    Today,
    Week,
    All,
}

/// Everything a card shows, precomputed from the store (live elapsed time aside).
#[derive(Clone)]
struct CardView {
    id: Id,
    kind: TaskKind,
    status: Status,
    /// A queued task you created that waits for "Start".
    held: bool,
    title: SharedString,
    /// `#12 · Bug triage` for pipeline steps, the kind label otherwise.
    run_chip: Option<SharedString>,
    pipeline_id: Option<Id>,
    /// `Bug triage › Fixer ×3`.
    trail: Option<SharedString>,
    actor: SharedString,
    model: SharedString,
    summary: SharedString,
    error: Option<SharedString>,
    variant: Option<u32>,
    /// `made by Fixer · depth 2` for spawned sub-agents.
    spawn: Option<SharedString>,
    cost: f64,
    priority: i64,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    /// Lower-cased text the search box matches against.
    haystack: String,
}

struct BoardData {
    cards: Vec<CardView>,
    /// `(id, name)` of live pipelines, for the filter select.
    pipelines: Vec<(Id, String)>,
    daily_budget: f64,
}

/// Drag payload: the card being moved and the column it came from.
#[derive(Clone)]
struct CardDrag {
    id: Id,
    col: usize,
    title: SharedString,
    status: Status,
}

/// The floating preview that follows the cursor while dragging.
struct DragGhost {
    title: SharedString,
    status: Status,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        card()
            .w(px(260.))
            .gap(theme::S2)
            .opacity(0.92)
            .border_color(theme::ACCENT_RING)
            .bg(theme::SURFACE_RAISED)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .child(StatusDot::new(self.status).id("board-ghost-dot").small())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_body_strong()
                            .fg()
                            .child(self.title.clone()),
                    ),
            )
    }
}

pub struct BoardScreen {
    data: Option<BoardData>,
    /// Filtered, sorted cards per column (indexes into `COLUMNS`).
    cols: [Vec<CardView>; 4],
    has_running: bool,
    ticking: bool,
    done_limit: usize,
    options_dirty: bool,
    pipeline_filter: Option<Id>,
    kind_filter: KindFilter,
    time_filter: TimeFilter,
    search: Entity<InputState>,
    pipeline_select: Entity<SelectState<Options>>,
    kind_select: Entity<SelectState<Options>>,
    /// "New task" composer, shown above the columns while `composer_open`.
    composer: Entity<NewTaskComposer>,
    composer_open: bool,
    /// Focus of the board root: `c` opens the composer while nothing else has focus.
    focus: FocusHandle,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl BoardScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let store_sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                if changes.iter().any(|c| {
                    matches!(
                        c,
                        Change::Task { .. }
                            | Change::Run(_)
                            | Change::Pipeline(_)
                            | Change::External
                    )
                }) {
                    this.reload(cx);
                }
            }
        });

        let search = input::text_input("Search title, agent, run", window, cx);
        let search_sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.rebuild(cx);
            }
        });

        let pipeline_select = select::select_state(
            vec![SelectOption::new("", "All pipelines")],
            Some(""),
            false,
            window,
            cx,
        );
        let pipeline_sub = cx.subscribe_in(
            &pipeline_select,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                if let SelectEvent::Confirm(Some(v)) = ev {
                    this.pipeline_filter = (!v.is_empty()).then(|| v.clone());
                    this.rebuild(cx);
                }
            },
        );
        let options: Vec<SelectOption> = KindFilter::ALL
            .iter()
            .map(|(_, v, l)| SelectOption::new(*v, *l))
            .collect();
        let kind_select = select::select_state(options, Some("all"), false, window, cx);
        let kind_sub = cx.subscribe_in(
            &kind_select,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                if let SelectEvent::Confirm(Some(v)) = ev {
                    this.kind_filter = KindFilter::from_value(v);
                    this.rebuild(cx);
                }
            },
        );

        let composer = cx.new(|cx| NewTaskComposer::new(window, cx));
        // The composer notifies on every title change: keeps the ghost card live.
        let composer_observe = cx.observe(&composer, |_, _, cx| cx.notify());
        let composer_sub = cx.subscribe_in(
            &composer,
            window,
            |this, _, ev: &NewTaskEvent, window, cx| {
                match ev {
                    // The board reloads by itself through the store change.
                    NewTaskEvent::Created(_) | NewTaskEvent::Closed => this.composer_open = false,
                }
                window.focus(&this.focus);
                cx.notify();
            },
        );
        let focus = cx.focus_handle();
        window.focus(&focus);

        let mut this = BoardScreen {
            data: None,
            cols: Default::default(),
            has_running: false,
            ticking: false,
            done_limit: DONE_PAGE,
            options_dirty: false,
            pipeline_filter: None,
            kind_filter: KindFilter::All,
            time_filter: TimeFilter::Week,
            search,
            pipeline_select,
            kind_select,
            composer,
            composer_open: false,
            focus,
            load: None,
            _subs: vec![
                store_sub,
                search_sub,
                pipeline_sub,
                kind_sub,
                composer_observe,
                composer_sub,
            ],
        };
        this.reload(cx);
        this
    }

    /// Opens the composer (or just focuses its title when it is already open).
    fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reset = !self.composer_open;
        self.composer_open = true;
        self.composer.update(cx, |c, cx| {
            if reset {
                c.open(
                    None,
                    "lands in Queued · first in line".into(),
                    true,
                    window,
                    cx,
                );
            }
            c.focus_title(window, cx);
        });
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let daily_budget = state::engine(cx).config().daily_budget_usd;
        self.load = Some(load(
            cx,
            move |s| {
                let tasks = s.tasks();
                let pipelines: HashMap<Id, String> =
                    s.pipelines().into_iter().map(|p| (p.id, p.name)).collect();
                // Per run: number, task, pipeline, node names (for the breadcrumb).
                let runs: HashMap<Id, RunRef> = s
                    .runs()
                    .into_iter()
                    .map(|r| {
                        (
                            r.id,
                            RunRef {
                                number: r.number,
                                title: r.title,
                                pipeline_id: r.pipeline_id,
                                nodes: r.graph.nodes.into_iter().map(|n| (n.id, n.name)).collect(),
                            },
                        )
                    })
                    .collect();
                let actors: HashMap<&Id, &String> =
                    tasks.iter().map(|t| (&t.id, &t.actor)).collect();
                let cards = tasks
                    .iter()
                    .map(|t| card_view(t, &runs, &pipelines, &actors))
                    .collect();
                let mut list: Vec<(Id, String)> = pipelines.into_iter().collect();
                list.sort_by_key(|a| a.1.to_lowercase());
                BoardData {
                    cards,
                    pipelines: list,
                    daily_budget,
                }
            },
            |this, data, cx| {
                this.data = Some(data);
                this.options_dirty = true;
                this.rebuild(cx);
            },
        ));
    }

    /// Applies filters and ordering to the cached cards.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(data) = &self.data else { return };
        let query = self.search.read(cx).value().to_lowercase();
        let query = query.trim();
        let now = Utc::now();
        let cutoff = match self.time_filter {
            TimeFilter::Today => Some(local_midnight()),
            TimeFilter::Week => Some(now - chrono::Duration::days(7)),
            TimeFilter::All => None,
        };
        let mut cols: [Vec<CardView>; 4] = Default::default();
        for c in &data.cards {
            if let Some(p) = &self.pipeline_filter {
                if c.pipeline_id.as_ref() != Some(p) {
                    continue;
                }
            }
            if !self.kind_filter.matches(c) || (!query.is_empty() && !c.haystack.contains(query)) {
                continue;
            }
            // The time window only trims history; live work is always shown.
            if c.status.is_finished()
                && cutoff.is_some_and(|t| c.finished_at.unwrap_or(c.created_at) < t)
            {
                continue;
            }
            cols[column_of(c.status)].push(c.clone());
        }
        for (i, col) in cols.iter_mut().enumerate() {
            if i == 3 {
                col.sort_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then(b.created_at.cmp(&a.created_at))
                });
            } else {
                col.sort_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then(a.created_at.cmp(&b.created_at))
                });
            }
        }
        self.has_running = !cols[1].is_empty();
        self.cols = cols;
        self.ensure_ticker(cx);
        cx.notify();
    }

    /// Re-renders once a second while something runs (live elapsed time).
    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticking || !self.has_running {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let keep = this
                .update(cx, |this, cx| {
                    if this.has_running {
                        cx.notify();
                    } else {
                        this.ticking = false;
                    }
                    this.has_running
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    /// Moves `dragged` before `target` in column `col` (or to the end when `target` is
    /// `None`), renumbers the column's priorities and saves the changes.
    fn reorder(&mut self, col: usize, dragged: &Id, target: Option<&Id>, cx: &mut Context<Self>) {
        let mut ids: Vec<Id> = self.cols[col].iter().map(|c| c.id.clone()).collect();
        let Some(from) = ids.iter().position(|i| i == dragged) else {
            return;
        };
        let target_orig = target.and_then(|t| ids.iter().position(|i| i == t));
        let id = ids.remove(from);
        let to = match target_orig {
            // Dropping on a card below lands after it; above, before it.
            Some(t) => {
                let ix = ids
                    .iter()
                    .position(|i| Some(i) == target)
                    .unwrap_or(ids.len());
                if from < t {
                    ix + 1
                } else {
                    ix
                }
            }
            None => ids.len(),
        };
        ids.insert(to.min(ids.len()), id);
        // Negative numbers sort ahead of untouched tasks (priority 0) and keep this order.
        let n = ids.len() as i64;
        let wanted: HashMap<Id, i64> = ids
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, i as i64 - n))
            .collect();
        let mut changes = Vec::new();
        if let Some(data) = &mut self.data {
            for c in &mut data.cards {
                if let Some(p) = wanted.get(&c.id) {
                    if c.priority != *p {
                        c.priority = *p;
                        changes.push((c.id.clone(), *p));
                    }
                }
            }
        }
        if changes.is_empty() {
            return;
        }
        self.rebuild(cx);
        state::run_engine(cx, move |e| {
            for (id, p) in changes {
                e.set_priority(&id, p)?;
            }
            Ok(())
        })
        .detach();
    }

    fn set_time(&mut self, t: TimeFilter, cx: &mut Context<Self>) {
        self.time_filter = t;
        self.rebuild(cx);
    }

    /// Pushes the loaded pipelines into the filter select (needs a window, so done on render).
    fn sync_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.options_dirty {
            return;
        }
        self.options_dirty = false;
        let Some(data) = &self.data else { return };
        let mut options = vec![SelectOption::new("", "All pipelines")];
        options.extend(
            data.pipelines
                .iter()
                .map(|(id, name)| SelectOption::new(id.clone(), name.clone())),
        );
        // A filter on a pipeline that no longer exists is dropped.
        if self
            .pipeline_filter
            .as_ref()
            .is_some_and(|p| !data.pipelines.iter().any(|(id, _)| id == p))
        {
            self.pipeline_filter = None;
        }
        let selected = self.pipeline_filter.clone().unwrap_or_default();
        select::set_options(&self.pipeline_select, options, Some(&selected), window, cx);
    }

    // ---- rendering ----------------------------------------------------------------

    fn stats(&self, data: &BoardData) -> impl IntoElement {
        let midnight = local_midnight();
        let count = |s: Status| data.cards.iter().filter(|c| c.status == s).count();
        let done_today = data
            .cards
            .iter()
            .filter(|c| {
                c.status == Status::Done && c.finished_at.unwrap_or(c.created_at) >= midnight
            })
            .count();
        let failed_today = data
            .cards
            .iter()
            .filter(|c| {
                c.status == Status::Failed && c.finished_at.unwrap_or(c.created_at) >= midnight
            })
            .count();
        let spend: f64 = data
            .cards
            .iter()
            .filter(|c| c.created_at >= midnight || c.started_at.is_some_and(|s| s >= midnight))
            .map(|c| c.cost)
            .sum();
        let cell = |label: &'static str, value: String, note: Option<String>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(theme::S6)
                .py(theme::S3)
                .border_r_1()
                .border_color(theme::LINE)
                .child(div().text_label().fg_muted().child(label))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(theme::S2)
                        .child(div().text_stat_sm().fg().child(value))
                        .when_some(note, |d, n| d.child(div().text_label().fg_muted().child(n))),
                )
        };
        let spend_cell = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(theme::S6)
            .py(theme::S3)
            .child(div().text_label().fg_muted().child("Spend today"))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme::S2)
                    .child(div().text_stat_sm().fg().child(ui::fmt_usd(spend)))
                    .when(data.daily_budget > 0.0, |d| {
                        d.child(
                            div()
                                .text_label()
                                .fg_muted()
                                .child(format!("of {}", ui::fmt_usd(data.daily_budget))),
                        )
                    }),
            )
            .when(data.daily_budget > 0.0, |d| {
                d.child(ProgressBar::new("board-spend", spend / data.daily_budget).budget())
            });
        div()
            .flex()
            .flex_none()
            .border_b_1()
            .border_color(theme::LINE)
            .child(cell("Queued", count(Status::Queued).to_string(), None))
            .child(cell("Running", count(Status::Running).to_string(), None))
            .child(cell(
                "Waiting for you",
                count(Status::Waiting).to_string(),
                None,
            ))
            .child(cell(
                "Done today",
                done_today.to_string(),
                Some(format!("{failed_today} failed")),
            ))
            .child(spend_cell)
    }

    fn filter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let time_btn =
            |id: &'static str, label: &'static str, t: TimeFilter, cx: &mut Context<Self>| {
                Button::new(id, label)
                    .small()
                    .ghost()
                    .selected(self.time_filter == t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_time(t, cx)))
            };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme::S3)
            .px(theme::S6)
            .py(theme::S3)
            .border_b_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .w(px(260.))
                    .flex_none()
                    .child(
                        input::field_sm(&self.search).prefix(
                            ui::icon_sized(AppIcon::Search, theme::ICON_SM)
                                .text_color(theme::TEXT_MUTED),
                        ),
                    ),
            )
            .child(div().flex_none().w(px(190.)).child(select::select_sm(&self.pipeline_select)))
            .child(div().flex_none().w(px(160.)).child(select::select_sm(&self.kind_select)))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(time_btn("board-t-today", "Today", TimeFilter::Today, cx))
                    .child(time_btn("board-t-7d", "7 days", TimeFilter::Week, cx))
                    .child(time_btn("board-t-all", "All", TimeFilter::All, cx)),
            )
    }

    fn card_el(
        &self,
        c: &CardView,
        col: usize,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let now = Utc::now();
        let id = c.id.clone();
        let waiting = c.status == Status::Waiting;
        let spawned = c.spawn.is_some();
        let time_text = match c.status {
            Status::Running => {
                ui::fmt_duration((now - c.started_at.unwrap_or(c.created_at)).num_seconds())
            }
            Status::Queued | Status::Waiting => {
                ui::fmt_duration((now - c.created_at).num_seconds())
            }
            _ => match (c.started_at, c.finished_at) {
                (Some(s), Some(f)) => ui::fmt_duration((f - s).num_seconds()),
                (_, Some(f)) => format!("{} ago", ui::fmt_duration((now - f).num_seconds())),
                _ => String::new(),
            },
        };
        let status_color = theme::status_color(c.status);
        let drag = CardDrag {
            id: c.id.clone(),
            col,
            title: c.title.clone(),
            status: c.status,
        };
        let drop_id = c.id.clone();

        div()
            .id(SharedString::from(format!("board-card-{}", c.id)))
            .flex()
            .flex_col()
            .gap(theme::S2)
            .p(theme::S3)
            .bg(if waiting {
                theme::WARNING_SOFT
            } else if spawned {
                theme::SPAWN_SOFT
            } else {
                theme::SURFACE_RAISED
            })
            .border_1()
            .border_color(if waiting {
                theme::alpha(theme::WARNING, 0.35)
            } else if spawned {
                theme::SPAWN_LINE
            } else if c.status == Status::Failed {
                theme::alpha(theme::DANGER, 0.35)
            } else {
                theme::LINE_STRONG
            })
            .rounded(theme::RADIUS_LG)
            .cursor_pointer()
            .hover(|s| s.border_color(theme::TEXT_DISABLED))
            .drag_over::<CardDrag>(|s, _, _, _| s.border_color(theme::ACCENT))
            .can_drop(move |v, _, _| v.downcast_ref::<CardDrag>().is_some_and(|d| d.col == col))
            .on_drop(cx.listener(move |this, d: &CardDrag, _, cx| {
                if d.col == col && d.id != drop_id {
                    this.reorder(col, &d.id, Some(&drop_id), cx);
                }
            }))
            .on_drag(drag, |d, _, _, cx| {
                let (title, status) = (d.title.clone(), d.status);
                cx.new(|_| DragGhost { title, status })
            })
            .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
            // Top line: run chip, age.
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::S2)
                    .text_mono_sm()
                    .fg_muted()
                    .child(
                        div().min_w_0().truncate().child(
                            c.run_chip
                                .clone()
                                .unwrap_or_else(|| SharedString::from(c.kind.label())),
                        ),
                    )
                    .child(div().flex_none().child(time_text)),
            )
            // Title with variant chip.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .when_some(c.variant, |d, v| {
                        d.child(VariantChip::variant(v).size(px(18.)))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_body_strong()
                            .fg()
                            .line_clamp(2)
                            .child(c.title.clone()),
                    ),
            )
            .when_some(c.spawn.clone(), |d, s| {
                d.child(div().flex().child(Badge::new(s).spawn()))
            })
            .when_some(c.trail.clone(), |d, t| {
                d.child(div().text_label().fg_muted().child(t))
            })
            .when(!c.summary.is_empty(), |d| {
                d.child(
                    div()
                        .text_label()
                        .fg_secondary()
                        .line_clamp(2)
                        .child(c.summary.clone()),
                )
            })
            .when_some(c.error.clone(), |d, e| {
                d.child(
                    div()
                        .text_label()
                        .text_color(theme::DANGER)
                        .line_clamp(2)
                        .child(e),
                )
            })
            // Footer: status, actor · model, cost.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .pt(theme::S1)
                    .text_label()
                    .fg_muted()
                    .child(
                        StatusDot::new(c.status)
                            .id(SharedString::from(format!("board-dot-{}", c.id)))
                            .small(),
                    )
                    .child(
                        div()
                            .text_color(status_color)
                            .child(status_text(c.status, c.held)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(
                        match (c.actor.is_empty(), c.model.is_empty()) {
                            (false, false) => format!("· {} · {}", c.actor, c.model),
                            (false, true) => format!("· {}", c.actor),
                            (true, false) => format!("· {}", c.model),
                            _ => String::new(),
                        },
                    ))
                    .child(div().flex_none().text_mono_sm().child(ui::fmt_usd(c.cost))),
            )
            .when(c.held && c.kind == TaskKind::Manual, |d| {
                let tid = c.id.clone();
                d.child(
                    div().flex().child(
                        Button::new(SharedString::from(format!("board-start-{}", c.id)), "Start")
                            .small()
                            .ghost()
                            .on_click(move |_, _, cx| {
                                let tid = tid.clone();
                                state::run_engine(cx, move |e| e.start_task(&tid)).detach();
                            }),
                    ),
                )
            })
            .when(waiting, |d| {
                let tid = c.id.clone();
                d.child(
                    Button::new(
                        SharedString::from(format!("board-review-{}", c.id)),
                        "Open in Review",
                    )
                    .small()
                    .icon_right(AppIcon::ChevronRight)
                    .on_click(move |_, _, cx| {
                        state::navigate(
                            Route::Review {
                                task: Some(tid.clone()),
                            },
                            cx,
                        )
                    }),
                )
            })
            .with_animation(
                SharedString::from(format!("board-card-in-{}", c.id)),
                Animation::new(theme::DUR_ENTER + Duration::from_millis(30 * (ix.min(6) as u64)))
                    .with_easing(theme::ease_out),
                |d, t| d.opacity(t).mt(px(8.) * (1.0 - t)),
            )
    }

    /// Dashed preview of the task being drafted, first in the Queued column.
    fn ghost_card(&self, cx: &App) -> impl IntoElement {
        let composer = self.composer.read(cx);
        let title = composer.title(cx);
        let title = ghost_title(&title);
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(theme::S3)
            .border_1()
            .border_dashed()
            .border_color(theme::ACCENT_RING)
            .rounded(theme::RADIUS_XL)
            .bg(theme::ACCENT_GLOW)
            .text_label()
            .fg_muted()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(StatusDot::new(Status::Queued).id("board-ghost-new").small())
                    .child("new")
                    .child(div().flex_1())
                    .child("draft"),
            )
            .child(div().text_body_strong().fg().child(title))
            .child(composer.agent_label())
    }

    fn column(&self, col: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, title) = COLUMNS[col];
        let all = &self.cols[col];
        let shown = if col == 3 {
            all.len().min(self.done_limit)
        } else {
            all.len()
        };
        let hidden = all.len() - shown;
        let highlight = status == Status::Waiting && !all.is_empty();
        let ghost = status == Status::Queued && self.composer_open;
        panel()
            .flex_1()
            .min_w(px(240.))
            .p(theme::S3)
            .gap(theme::S2)
            .when(highlight, |d| {
                d.border_color(theme::alpha(theme::WARNING, 0.35))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .px(theme::S1)
                    .h(px(28.))
                    .child(
                        StatusDot::new(status)
                            .id(SharedString::from(format!("board-col-{}", status.as_str()))),
                    )
                    .child(
                        div()
                            .text_body_strong()
                            .when(highlight, |d| d.text_color(theme::WARNING))
                            .child(title),
                    )
                    .child(div().text_label().fg_muted().child(all.len().to_string()))
                    .child(div().flex_1())
                    .when(status == Status::Queued, |d| {
                        d.child(
                            IconButton::new("board-add-queued", AppIcon::Plus)
                                .small()
                                .tooltip("Add a task to Queued")
                                .selected(self.composer_open)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_composer(window, cx)
                                })),
                        )
                    })
                    .when(status == Status::Waiting && !all.is_empty(), |d| {
                        d.child(
                            Button::new("board-col-review", "Review")
                                .small()
                                .ghost()
                                .icon_right(AppIcon::ChevronRight)
                                .on_click(|_, _, cx| {
                                    state::navigate(Route::Review { task: None }, cx)
                                }),
                        )
                    }),
            )
            .when(ghost, |d| d.child(self.ghost_card(cx)))
            .children(
                all.iter()
                    .take(shown)
                    .enumerate()
                    .map(|(i, c)| self.card_el(c, col, i, cx)),
            )
            .when(all.is_empty() && !ghost, |d| {
                d.child(
                    div()
                        .px(theme::S1)
                        .py(theme::S3)
                        .text_label()
                        .fg_disabled()
                        .child("Nothing here"),
                )
            })
            .when(hidden > 0, |d| {
                d.child(
                    Button::new(
                        "board-done-more",
                        format!("Show {} more", hidden.min(DONE_PAGE)),
                    )
                    .small()
                    .ghost()
                    .icon_right(AppIcon::ChevronDown)
                    .full_width()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.done_limit += DONE_PAGE;
                        cx.notify();
                    })),
                )
            })
            // Dropping below the last card sends it to the end of the column.
            .child(
                div()
                    .id(SharedString::from(format!("board-end-{col}")))
                    .flex_1()
                    .min_h(px(32.))
                    .rounded(theme::RADIUS_LG)
                    .drag_over::<CardDrag>(|s, _, _, _| s.bg(theme::alpha(theme::ACCENT, 0.08)))
                    .can_drop(move |v, _, _| {
                        v.downcast_ref::<CardDrag>().is_some_and(|d| d.col == col)
                    })
                    .on_drop(cx.listener(move |this, d: &CardDrag, _, cx| {
                        if d.col == col {
                            this.reorder(col, &d.id, None, cx);
                        }
                    })),
            )
    }
}

impl Render for BoardScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_options(window, cx);
        let running = self.cols[1].len();
        let mut header = PageHeader::new("Board");
        if let Some(data) = &self.data {
            header = header.subtitle(format!("{} tasks", data.cards.len()));
        }
        if running > 0 {
            header = header.meta(Badge::new(format!("{running} running")).info().dot());
        }
        let header = header
            .action(segmented(
                "board-view",
                &["Board", "Timeline"],
                0,
                |ix, _, cx| {
                    if ix == 1 {
                        state::navigate(Route::Timeline, cx);
                    }
                },
            ))
            .action(
                Button::new("board-new-task", "New task")
                    .primary()
                    .icon(AppIcon::Plus)
                    .kbd("C")
                    .on_click(cx.listener(|this, _, window, cx| this.open_composer(window, cx))),
            );

        let content: gpui::AnyElement = match &self.data {
            None => body()
                .child(skeleton_rows("board-loading", 4))
                .into_any_element(),
            Some(data) if data.cards.is_empty() => body()
                .when(self.composer_open, |d| {
                    d.child(div().w_full().child(self.composer.clone()))
                })
                .child(
                    EmptyState::new(
                        AppIcon::Board,
                        "No tasks yet",
                        "Steps of pipeline runs, chats and sub-agents show up here as cards.",
                    )
                    .action(
                        Button::new("board-empty-pipelines", "Go to Pipelines").on_click(
                            |_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx),
                        ),
                    ),
                )
                .into_any_element(),
            Some(data) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.stats(data))
                .child(self.filter_bar(cx))
                .child(
                    div()
                        .id("board-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(theme::S6)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(theme::S4)
                                .when(self.composer_open, |d| {
                                    d.child(div().w_full().child(self.composer.clone()))
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap(theme::S4)
                                        .children((0..4).map(|col| self.column(col, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        };
        screen()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                // Only when the board itself has focus, never while typing in an input.
                if is_new_task_key(&ev.keystroke.key, ev.keystroke.modifiers.modified())
                    && !this.composer_open
                    && this.focus.is_focused(window)
                {
                    this.open_composer(window, cx);
                }
            }))
            .child(header)
            .child(content)
    }
}

// ---- helpers ------------------------------------------------------------------------

/// Footer status text of a card: held tasks say "on hold" instead of "Queued".
fn status_text(status: Status, held: bool) -> &'static str {
    if held && status == Status::Queued {
        "on hold"
    } else {
        status.label()
    }
}

/// Title shown on the ghost card while the composer's title is still empty.
fn ghost_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        "Untitled task".to_string()
    } else {
        t.to_string()
    }
}

/// Whether a key press (without modifiers) is the "new task" shortcut.
fn is_new_task_key(key: &str, modified: bool) -> bool {
    key == "c" && !modified
}

fn column_of(s: Status) -> usize {
    match s {
        Status::Queued => 0,
        Status::Running => 1,
        Status::Waiting => 2,
        _ => 3,
    }
}

/// Start of today in local time, as UTC.
fn local_midnight() -> DateTime<Utc> {
    Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| d.and_local_timezone(Local).earliest())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24))
}

/// What a card needs to know about the run a task belongs to.
struct RunRef {
    number: u64,
    /// The run's task (first sentence of its input).
    title: String,
    pipeline_id: Id,
    nodes: HashMap<Id, String>,
}

fn card_view(
    t: &CoreTask,
    runs: &HashMap<Id, RunRef>,
    pipelines: &HashMap<Id, String>,
    actors: &HashMap<&Id, &String>,
) -> CardView {
    let run = t.run_id.as_ref().and_then(|r| runs.get(r));
    let pipeline_name = run.and_then(|r| pipelines.get(&r.pipeline_id)).cloned();
    let node_name = run
        .zip(t.node_id.as_ref())
        .and_then(|(r, n)| r.nodes.get(n))
        .cloned();
    let run_chip = match (run, &pipeline_name) {
        (Some(r), Some(p)) => Some(SharedString::from(format!("#{} · {p}", r.number))),
        (Some(r), None) => Some(SharedString::from(format!("#{}", r.number))),
        _ => None,
    };
    let run_title = run
        .map(|r| r.title.clone())
        .filter(|t| !t.trim().is_empty());
    // Pipeline steps are titled by the run's task (what the work is about) with the step
    // below; sub-agents keep their own title and say which task they work for.
    let is_step = t.kind != TaskKind::Spawned && node_name.is_some();
    let (title, trail) = match (&run_title, is_step) {
        (Some(task), true) => {
            let step = node_name.clone().unwrap_or_default();
            let step = match t.variant {
                Some(v) => format!("{step} {}", harness_core::variant_letter(v)),
                None => step,
            };
            (
                task.clone(),
                Some(SharedString::from(format!("Step · {step}"))),
            )
        }
        (Some(task), false) if t.kind == TaskKind::Spawned => (
            t.title.clone(),
            Some(SharedString::from(format!("For · {task}"))),
        ),
        _ => (t.title.clone(), None),
    };
    let spawn = (t.kind == TaskKind::Spawned).then(|| {
        let parent = t
            .parent_id
            .as_ref()
            .and_then(|p| actors.get(p))
            .map(|a| a.as_str())
            .filter(|a| !a.is_empty())
            .unwrap_or("an agent");
        SharedString::from(format!("made by {parent} · depth {}", t.depth))
    });
    let model = if t.model.is_empty() {
        String::new()
    } else {
        Config::short_model(&t.model)
    };
    let haystack = format!(
        "{} {} {} {} {} {} {}",
        t.title,
        title,
        t.actor,
        model,
        t.summary,
        run_chip.as_ref().map(|s| s.as_str()).unwrap_or(""),
        trail.as_ref().map(|s| s.as_str()).unwrap_or("")
    )
    .to_lowercase();
    CardView {
        id: t.id.clone(),
        kind: t.kind,
        status: t.status,
        held: t.status == Status::Queued && harness_engine::tasks::is_held(t),
        title: title.into(),
        run_chip: run_chip
            .or_else(|| (t.kind == TaskKind::Chat).then(|| SharedString::from("Chat")))
            .or_else(|| (t.kind == TaskKind::Manual).then(|| SharedString::from("Task"))),
        pipeline_id: run.map(|r| r.pipeline_id.clone()),
        trail,
        actor: t.actor.clone().into(),
        model: model.into(),
        summary: t
            .summary
            .lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string()
            .into(),
        error: t
            .error
            .as_ref()
            .filter(|_| t.status == Status::Failed)
            .map(|e| SharedString::from(e.clone())),
        variant: t.variant,
        spawn,
        cost: t.cost_usd,
        priority: t.priority,
        created_at: t.created_at,
        started_at: t.started_at,
        finished_at: t.finished_at,
        haystack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_queued_cards_say_on_hold() {
        assert_eq!(status_text(Status::Queued, true), "on hold");
        assert_eq!(status_text(Status::Queued, false), "Queued");
        assert_eq!(status_text(Status::Running, true), "Running");
    }

    #[test]
    fn ghost_title_falls_back() {
        assert_eq!(ghost_title("  "), "Untitled task");
        assert_eq!(ghost_title(" Fix it "), "Fix it");
    }

    #[test]
    fn only_plain_c_opens_the_composer() {
        assert!(is_new_task_key("c", false));
        assert!(!is_new_task_key("c", true));
        assert!(!is_new_task_key("x", false));
    }
}
