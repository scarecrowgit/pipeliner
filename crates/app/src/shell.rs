//! Root view: sidebar, the current route's screen (with an enter animation), the command
//! palette (ctrl/cmd-K) and the toast stack. Screens are created on first visit and kept.

use std::collections::HashMap;
use std::time::Instant;

use chrono::{Datelike as _, Local};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, px, relative, Animation, AnimationExt as _, AnyElement, AnyView, App,
    AppContext as _, Context, Div, ElementId, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, Render, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Task,
    Window,
};
use gpui_component::input::{self as ginput, Input, InputEvent, InputState};
use gpui_component::Root;
use harness_core::{Config, Id, Status, Store};

use crate::screens;
use crate::state::{self, AppEvent, AppState, Route, ScreenKind, Toast, ToastKind};
use crate::theme::{self, TextStyles as _};
use crate::ui::{
    self, count_pill, dot_el, icon, icon_sized, kbd, ring_el, AppIcon, IconButton, ProgressBar,
};

actions!(
    pipeliner,
    [
        OpenPalette,
        Back,
        Nav1,
        Nav2,
        Nav3,
        Nav4,
        Nav5,
        Nav6,
        Nav7,
        Nav8,
        Nav9
    ]
);

/// Global key bindings and their handlers. They work wherever focus is.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-k", OpenPalette, None),
        KeyBinding::new("ctrl-k", OpenPalette, None),
        KeyBinding::new("escape", Back, None),
        KeyBinding::new("alt-left", Back, None),
        KeyBinding::new("ctrl-1", Nav1, None),
        KeyBinding::new("ctrl-2", Nav2, None),
        KeyBinding::new("ctrl-3", Nav3, None),
        KeyBinding::new("ctrl-4", Nav4, None),
        KeyBinding::new("ctrl-5", Nav5, None),
        KeyBinding::new("ctrl-6", Nav6, None),
        KeyBinding::new("ctrl-7", Nav7, None),
        KeyBinding::new("ctrl-8", Nav8, None),
        KeyBinding::new("ctrl-9", Nav9, None),
    ]);
    cx.on_action(|_: &OpenPalette, cx| {
        AppState::global(cx).update(cx, |s, cx| s.request_palette(cx))
    });
    cx.on_action(|_: &Back, cx| AppState::global(cx).update(cx, |s, cx| s.escape(cx)));
    fn nav(ix: usize, cx: &mut App) {
        let route = Route::numbered()[ix].clone();
        state::navigate(route, cx);
    }
    cx.on_action(|_: &Nav1, cx| nav(0, cx));
    cx.on_action(|_: &Nav2, cx| nav(1, cx));
    cx.on_action(|_: &Nav3, cx| nav(2, cx));
    cx.on_action(|_: &Nav4, cx| nav(3, cx));
    cx.on_action(|_: &Nav5, cx| nav(4, cx));
    cx.on_action(|_: &Nav6, cx| nav(5, cx));
    cx.on_action(|_: &Nav7, cx| nav(6, cx));
    cx.on_action(|_: &Nav8, cx| nav(7, cx));
    cx.on_action(|_: &Nav9, cx| nav(8, cx));
}

// ---------------------------------------------------------------------------
// Sidebar data (computed off the UI thread on every store revision)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct PipelineEntry {
    id: Id,
    name: SharedString,
    color: u32,
    deployed: bool,
    waiting: usize,
}

#[derive(Clone, Debug, Default)]
struct SidebarData {
    /// Tasks waiting for you (Review badge).
    waiting: usize,
    /// Everything in Home's "Needs you": waiting tasks + undeployed drafts.
    needs_you: usize,
    running: usize,
    spend_today: f64,
    spend_month: f64,
    daily_budget: f64,
    monthly_budget: f64,
    pipelines: Vec<PipelineEntry>,
}

impl SidebarData {
    fn query(store: &Store, cfg: &Config) -> SidebarData {
        let tasks = store.tasks();
        let runs = store.runs();
        let run_pipeline: HashMap<&str, &str> = runs
            .iter()
            .map(|r| (r.id.as_str(), r.pipeline_id.as_str()))
            .collect();
        let now = Local::now();
        let today = now.date_naive();
        let mut d = SidebarData {
            daily_budget: cfg.daily_budget_usd,
            monthly_budget: cfg.monthly_budget_usd,
            ..Default::default()
        };
        let mut waiting_by_pipeline: HashMap<&str, usize> = HashMap::new();
        for t in &tasks {
            match t.status {
                Status::Waiting => {
                    d.waiting += 1;
                    if let Some(p) = t.run_id.as_deref().and_then(|r| run_pipeline.get(r)) {
                        *waiting_by_pipeline.entry(p).or_default() += 1;
                    }
                }
                Status::Running => d.running += 1,
                _ => {}
            }
            let day = t.created_at.with_timezone(&Local);
            if day.year() == now.year() && day.month() == now.month() {
                d.spend_month += t.cost_usd;
                if day.date_naive() == today {
                    d.spend_today += t.cost_usd;
                }
            }
        }
        let pipelines = store.pipelines();
        let drafts = pipelines.iter().filter(|p| p.draft.is_some()).count();
        d.needs_you = d.waiting + drafts;
        d.pipelines = pipelines
            .iter()
            .map(|p| PipelineEntry {
                id: p.id.clone(),
                name: p.name.clone().into(),
                color: p.color,
                deployed: !p.graph.nodes.is_empty(),
                waiting: waiting_by_pipeline.get(p.id.as_str()).copied().unwrap_or(0),
            })
            .collect();
        d
    }
}

fn fmt_budget(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("${:.0}", v)
    } else {
        ui::fmt_usd(v)
    }
}

// ---------------------------------------------------------------------------
// Command palette
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct PaletteItem {
    icon: AppIcon,
    title: SharedString,
    hint: SharedString,
    route: Route,
    haystack: String,
}

impl PaletteItem {
    fn new(
        icon: AppIcon,
        title: impl Into<SharedString>,
        hint: impl Into<SharedString>,
        route: Route,
    ) -> Self {
        let title = title.into();
        let hint = hint.into();
        let haystack = format!("{} {}", title, hint).to_lowercase();
        PaletteItem {
            icon,
            title,
            hint,
            route,
            haystack,
        }
    }
}

fn screen_items() -> Vec<PaletteItem> {
    vec![
        PaletteItem::new(AppIcon::Home, "Home", "Screen", Route::Home),
        PaletteItem::new(
            AppIcon::Inbox,
            "Review",
            "Screen",
            Route::Review { task: None },
        ),
        PaletteItem::new(AppIcon::Chat, "Chat", "Screen", Route::Chat { chat: None }),
        PaletteItem::new(AppIcon::Board, "Board", "Screen", Route::Board),
        PaletteItem::new(AppIcon::Timeline, "Timeline", "Screen", Route::Timeline),
        PaletteItem::new(
            AppIcon::Workflow,
            "Pipelines",
            "Screen",
            Route::Pipelines { pipeline: None },
        ),
        PaletteItem::new(
            AppIcon::Bot,
            "Agents",
            "Screen",
            Route::Agents { agent: None },
        ),
        PaletteItem::new(AppIcon::Book, "Memory", "Screen", Route::Memory),
        PaletteItem::new(AppIcon::Shield, "Policies", "Screen", Route::Policies),
        PaletteItem::new(AppIcon::Sliders, "Settings", "Screen", Route::Settings),
    ]
}

fn data_items(store: &Store) -> Vec<PaletteItem> {
    let mut out = vec![];
    for p in store.pipelines() {
        out.push(PaletteItem::new(
            AppIcon::Workflow,
            p.name.clone(),
            "Pipeline",
            Route::Pipelines {
                pipeline: Some(p.id),
            },
        ));
    }
    for c in store.chats().into_iter().take(200) {
        out.push(PaletteItem::new(
            AppIcon::Chat,
            c.title.clone(),
            format!("Chat · {}", c.mode.label()),
            Route::Chat { chat: Some(c.id) },
        ));
    }
    for a in store.agents() {
        out.push(PaletteItem::new(
            AppIcon::Bot,
            a.name.clone(),
            "Agent",
            Route::Agents { agent: Some(a.id) },
        ));
    }
    let mut tasks = store.tasks();
    tasks.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    for t in tasks.into_iter().take(300) {
        let route = if t.status == Status::Waiting {
            Route::Review {
                task: Some(t.id.clone()),
            }
        } else {
            Route::Task { task: t.id.clone() }
        };
        out.push(PaletteItem::new(
            AppIcon::Activity,
            t.title.clone(),
            format!("Task · {}", t.status.label()),
            route,
        ));
    }
    out
}

/// Subsequence match with bonuses for word starts and runs; `None` if not all chars match.
fn fuzzy_score(hay: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.chars().collect();
    let mut score = 0i32;
    let mut hi = 0usize;
    let mut prev: Option<usize> = None;
    for nc in needle.chars() {
        if nc.is_whitespace() {
            continue;
        }
        let mut found = None;
        while hi < hay.len() {
            if hay[hi] == nc {
                found = Some(hi);
                hi += 1;
                break;
            }
            hi += 1;
        }
        let pos = found?;
        score += 1;
        if pos == 0 || !hay[pos - 1].is_alphanumeric() {
            score += 8;
        }
        match prev {
            Some(p) if p + 1 == pos => score += 6,
            Some(p) => score -= ((pos - p - 1) as i32).min(5),
            None => score -= (pos as i32).min(8),
        }
        prev = Some(pos);
    }
    Some(score)
}

struct Palette {
    input: Entity<InputState>,
    items: Vec<PaletteItem>,
    results: Vec<usize>,
    selected: usize,
    closing: bool,
    seq: u64,
    scroll: ScrollHandle,
    _sub: Subscription,
}

impl Palette {
    fn filter(&mut self, query: &str) {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            // Screens first, then the most recent items.
            self.results = (0..self.items.len()).take(60).collect();
        } else {
            let mut scored: Vec<(i32, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| fuzzy_score(&it.haystack, &q).map(|s| (s, i)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.results = scored.into_iter().take(60).map(|(_, i)| i).collect();
        }
        self.selected = 0;
        self.scroll.scroll_to_item(0);
    }
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

pub struct Shell {
    focus: FocusHandle,
    screens: HashMap<ScreenKind, AnyView>,
    sidebar: Option<SidebarData>,
    sidebar_load: Option<Task<()>>,
    palette: Option<Palette>,
    palette_seq: u64,
    /// nav_seq seen last render and when its animation started.
    route_anim: (u64, Instant),
    _subs: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &app,
                window,
                |this, _, ev: &AppEvent, window, cx| match ev {
                    AppEvent::StoreChanged { .. } => this.reload_sidebar(cx),
                    AppEvent::PaletteRequested => {
                        if this.palette.as_ref().is_some_and(|p| !p.closing) {
                            this.close_palette(window, cx);
                        } else {
                            this.open_palette(window, cx);
                        }
                    }
                    AppEvent::CloseOverlay => this.close_palette(window, cx),
                    AppEvent::Navigated(_) => {}
                },
            ),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut this = Shell {
            focus,
            screens: HashMap::new(),
            sidebar: None,
            sidebar_load: None,
            palette: None,
            palette_seq: 0,
            route_anim: (0, Instant::now()),
            _subs: subs,
        };
        this.reload_sidebar(cx);
        this
    }

    fn reload_sidebar(&mut self, cx: &mut Context<Self>) {
        let store = state::store(cx);
        let cfg = state::engine(cx).config();
        let job = cx.background_spawn(async move { SidebarData::query(&store, &cfg) });
        self.sidebar_load = Some(cx.spawn(async move |this, cx| {
            let data = job.await;
            let _ = this.update(cx, |this, cx| {
                this.sidebar = Some(data);
                cx.notify();
            });
        }));
    }

    fn screen(&mut self, kind: ScreenKind, window: &mut Window, cx: &mut Context<Self>) -> AnyView {
        if let Some(v) = self.screens.get(&kind) {
            return v.clone();
        }
        let v = screens::create(kind, window, cx);
        self.screens.insert(kind, v.clone());
        v
    }

    // ---- palette -----------------------------------------------------------------

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = state::store(cx);
        let mut items = screen_items();
        items.extend(data_items(&store));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search or jump to…"));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    let q = input.read(cx).value();
                    if let Some(p) = this.palette.as_mut() {
                        p.filter(&q);
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.activate_selected(window, cx),
                _ => {}
            },
        );
        input.update(cx, |s, cx| s.focus(window, cx));
        self.palette_seq += 1;
        let mut p = Palette {
            input,
            items,
            results: vec![],
            selected: 0,
            closing: false,
            seq: self.palette_seq,
            scroll: ScrollHandle::new(),
            _sub: sub,
        };
        p.filter("");
        self.palette = Some(p);
        AppState::global(cx).update(cx, |s, _| s.overlay_open = true);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        if p.closing {
            return;
        }
        p.closing = true;
        let seq = p.seq;
        AppState::global(cx).update(cx, |s, _| s.overlay_open = false);
        window.focus(&self.focus);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(theme::DUR_EXIT).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .palette
                    .as_ref()
                    .is_some_and(|p| p.seq == seq && p.closing)
                {
                    this.palette = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        if p.results.is_empty() {
            return;
        }
        let n = p.results.len() as isize;
        p.selected = ((p.selected as isize + delta).rem_euclid(n)) as usize;
        p.scroll.scroll_to_item(p.selected);
        cx.notify();
    }

    fn activate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self
            .palette
            .as_ref()
            .and_then(|p| p.results.get(p.selected).and_then(|i| p.items.get(*i)))
            .map(|it| it.route.clone());
        self.close_palette(window, cx);
        if let Some(r) = route {
            state::navigate(r, cx);
        }
    }

    // ---- sidebar -----------------------------------------------------------------

    fn nav_item(
        id: &'static str,
        icon_name: AppIcon,
        label: &'static str,
        route: Route,
        active: bool,
        count: Option<usize>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(theme::NAV_ITEM_H)
            .px(theme::S2)
            .rounded(theme::RADIUS_MD)
            .cursor_pointer()
            .text_body()
            .when(active, |d| {
                d.bg(theme::SURFACE_HOVER).text_color(theme::TEXT)
            })
            .when(!active, |d| {
                d.text_color(theme::TEXT_SECONDARY)
                    .hover(|s| s.bg(theme::SURFACE_RAISED).text_color(theme::TEXT))
            })
            .child(icon(icon_name))
            .child(div().flex_1().child(label))
            .when_some(count.filter(|c| *c > 0), |d, c| d.child(count_pill(c)))
            .on_click(move |_, _, cx| state::navigate(route.clone(), cx))
    }

    fn group_label(text: &'static str) -> Div {
        div()
            .mt(px(14.))
            .mb(theme::S1)
            .px(theme::S2)
            .text_label()
            .fg_muted()
            .child(text)
    }

    fn render_sidebar(&self, route: &Route) -> impl IntoElement {
        let kind = route.kind();
        let d = self.sidebar.clone().unwrap_or_default();
        let selected_pipeline = match route {
            Route::Pipelines { pipeline } => pipeline.clone(),
            _ => None,
        };
        let search = div()
            .id("sidebar-search")
            .flex()
            .flex_none()
            .items_center()
            .gap(theme::S2)
            .h(px(32.))
            .mt(px(6.))
            .mb(px(10.))
            .pl(px(10.))
            .pr(theme::S2)
            .rounded(theme::RADIUS_LG)
            .border_1()
            .border_color(theme::LINE_STRONG)
            .bg(theme::SURFACE_RAISED)
            .text_color(theme::TEXT_MUTED)
            .text_label()
            .cursor_pointer()
            .hover(|s| s.border_color(theme::hex(0x3a3d44)))
            .child(icon_sized(AppIcon::Search, theme::ICON_SM))
            .child(div().flex_1().child("Search or jump to…"))
            .child(kbd(if cfg!(target_os = "macos") {
                "⌘K"
            } else {
                "Ctrl K"
            }))
            .on_click(|_, _, cx| AppState::global(cx).update(cx, |s, cx| s.request_palette(cx)));

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(40.))
            .px(theme::S2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(theme::RADIUS_MD)
                    .bg(theme::ACCENT_STRONG)
                    .text_color(theme::ON_ACCENT)
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("P"),
            )
            .child(div().flex_1().text_body_strong().fg().child("Pipeliner"));

        let pipelines_header = div()
            .flex()
            .items_center()
            .mt(px(14.))
            .mb(theme::S1)
            .pl(theme::S2)
            .text_label()
            .fg_muted()
            .child(div().flex_1().child("Pipelines"))
            .child(
                IconButton::new("sidebar-new-pipeline", AppIcon::Plus)
                    .small()
                    .tooltip("New pipeline")
                    .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
            );

        let pipeline_rows: Vec<AnyElement> = if self.sidebar.is_some() && d.pipelines.is_empty() {
            vec![div()
                .px(theme::S2)
                .py(theme::S1)
                .text_label()
                .fg_disabled()
                .child("No pipelines yet")
                .into_any_element()]
        } else {
            d.pipelines
                .iter()
                .map(|p| {
                    let active = selected_pipeline.as_ref() == Some(&p.id);
                    let id = p.id.clone();
                    div()
                        .id(SharedString::from(format!("sidebar-pipeline-{}", p.id)))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(10.))
                        .h(theme::NAV_ITEM_H)
                        .px(theme::S2)
                        .rounded(theme::RADIUS_MD)
                        .cursor_pointer()
                        .text_body()
                        .when(active, |d| {
                            d.bg(theme::SURFACE_HOVER).text_color(theme::TEXT)
                        })
                        .when(!active, |d| {
                            d.text_color(theme::TEXT_SECONDARY)
                                .hover(|s| s.bg(theme::SURFACE_RAISED).text_color(theme::TEXT))
                        })
                        .child(
                            div()
                                .w(px(16.))
                                .flex()
                                .justify_center()
                                .child(if p.deployed {
                                    dot_el(theme::rgb_u32(p.color), px(8.))
                                } else {
                                    ring_el(theme::TEXT_MUTED, px(8.))
                                }),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(p.name.clone()))
                        .when(p.waiting > 0, |d| d.child(count_pill(p.waiting)))
                        .on_click(move |_, _, cx| {
                            state::navigate(
                                Route::Pipelines {
                                    pipeline: Some(id.clone()),
                                },
                                cx,
                            )
                        })
                        .into_any_element()
                })
                .collect()
        };

        let nav = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(Self::nav_item(
                "nav-home",
                AppIcon::Home,
                "Home",
                Route::Home,
                kind == ScreenKind::Home,
                Some(d.needs_you),
            ))
            .child(Self::nav_item(
                "nav-review",
                AppIcon::Inbox,
                "Review",
                Route::Review { task: None },
                kind == ScreenKind::Review,
                Some(d.waiting),
            ))
            .child(Self::group_label("Work"))
            .child(Self::nav_item(
                "nav-chat",
                AppIcon::Chat,
                "Chat",
                Route::Chat { chat: None },
                kind == ScreenKind::Chat,
                None,
            ))
            .child(Self::nav_item(
                "nav-board",
                AppIcon::Board,
                "Board",
                Route::Board,
                kind == ScreenKind::Board,
                None,
            ))
            .child(Self::nav_item(
                "nav-timeline",
                AppIcon::Timeline,
                "Timeline",
                Route::Timeline,
                kind == ScreenKind::Timeline,
                None,
            ))
            .child(Self::group_label("Build"))
            .child(Self::nav_item(
                "nav-pipelines",
                AppIcon::Workflow,
                "Pipelines",
                Route::Pipelines { pipeline: None },
                kind == ScreenKind::Pipelines && selected_pipeline.is_none(),
                None,
            ))
            .child(Self::nav_item(
                "nav-agents",
                AppIcon::Bot,
                "Agents",
                Route::Agents { agent: None },
                kind == ScreenKind::Agents,
                None,
            ))
            .child(Self::nav_item(
                "nav-memory",
                AppIcon::Book,
                "Memory",
                Route::Memory,
                kind == ScreenKind::Memory,
                None,
            ))
            .child(Self::nav_item(
                "nav-policies",
                AppIcon::Shield,
                "Policies",
                Route::Policies,
                kind == ScreenKind::Policies,
                None,
            ))
            .child(pipelines_header)
            .children(pipeline_rows);

        let today = if d.daily_budget > 0.0 {
            div()
                .flex()
                .items_baseline()
                .gap(px(4.))
                .child(
                    div()
                        .text_body_strong()
                        .fg()
                        .child(ui::fmt_usd(d.spend_today)),
                )
                .child(
                    div()
                        .text_body()
                        .fg_muted()
                        .child(format!("of {}", ui::fmt_usd(d.daily_budget))),
                )
        } else {
            div()
                .text_body_strong()
                .fg()
                .child(ui::fmt_usd(d.spend_today))
        };
        let month = if d.monthly_budget > 0.0 {
            format!(
                "month {} of {}",
                ui::fmt_usd(d.spend_month),
                fmt_budget(d.monthly_budget)
            )
        } else {
            format!("month {}", ui::fmt_usd(d.spend_month))
        };
        let footer = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(6.))
            .mt(theme::S2)
            .pt(theme::S3)
            .px(theme::S2)
            .pb(theme::S1)
            .border_t_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(div().text_label().fg_muted().child("Spend today"))
                    .child(today),
            )
            .when(d.daily_budget > 0.0, |f| {
                f.child(ProgressBar::new("sidebar-spend", d.spend_today / d.daily_budget).budget())
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_x(theme::S3)
                    .text_label()
                    .fg_muted()
                    .child(div().flex_none().child(format!("{} running", d.running)))
                    .child(div().flex_none().child(month)),
            );

        div()
            .w(theme::SIDEBAR_W)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme::S3)
            .bg(theme::BG_SIDEBAR)
            .border_r_1()
            .border_color(theme::LINE)
            .text_color(theme::TEXT_SECONDARY)
            .child(header)
            .child(search)
            .child(nav)
            .child(div().pt(theme::S2).child(Self::nav_item(
                "nav-settings",
                AppIcon::Sliders,
                "Settings",
                Route::Settings,
                kind == ScreenKind::Settings,
                None,
            )))
            .child(footer)
    }

    // ---- overlays ----------------------------------------------------------------

    fn render_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.palette.as_ref()?;
        let (anim_id, dur, closing) = if p.closing {
            (
                ElementId::NamedInteger("palette-out".into(), p.seq),
                theme::DUR_EXIT,
                true,
            )
        } else {
            (
                ElementId::NamedInteger("palette-in".into(), p.seq),
                theme::DUR_OVERLAY,
                false,
            )
        };
        let ease: fn(f32) -> f32 = if closing {
            theme::ease_in
        } else {
            theme::ease_out
        };
        let progress = move |t: f32| if closing { 1.0 - t } else { t };

        let rows = p.results.iter().enumerate().filter_map(|(row, ix)| {
            let it = p.items.get(*ix)?;
            let selected = row == p.selected;
            Some(
                div()
                    .id(("palette-row", row))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(36.))
                    .px(px(10.))
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::SURFACE_HOVER))
                    .child(icon_sized(it.icon, theme::ICON).text_color(if selected {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_body()
                            .fg()
                            .truncate()
                            .child(it.title.clone()),
                    )
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .whitespace_nowrap()
                            .child(it.hint.clone()),
                    )
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.palette.as_mut() {
                            if p.selected != row {
                                p.selected = row;
                                cx.notify();
                            }
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(p) = this.palette.as_mut() {
                            p.selected = row;
                        }
                        this.activate_selected(window, cx);
                    })),
            )
        });
        let list: AnyElement = if p.results.is_empty() {
            div()
                .px(theme::S4)
                .py(theme::S6)
                .text_body()
                .fg_muted()
                .text_center()
                .child("No matches")
                .into_any_element()
        } else {
            div()
                .id("palette-list")
                .track_scroll(&p.scroll)
                .max_h(px(400.))
                .overflow_y_scroll()
                .p(theme::S1)
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        };
        let hint = |k: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(kbd(k))
                .child(label)
        };
        let panel = div()
            .id("palette-panel")
            .w(px(640.))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(theme::LINE_STRONG)
            .bg(theme::SURFACE)
            .overflow_hidden()
            .key_context("Palette")
            .on_action(cx.listener(|this, _: &ginput::MoveUp, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &ginput::MoveDown, _, cx| this.move_selection(1, cx)))
            .on_action(
                cx.listener(|this, _: &ginput::Escape, window, cx| this.close_palette(window, cx)),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .h(px(52.))
                    .px(theme::S4)
                    .border_b_1()
                    .border_color(theme::LINE)
                    .child(icon(AppIcon::Search).text_color(theme::TEXT_MUTED))
                    .child(
                        div().flex_1().child(
                            Input::new(&p.input)
                                .appearance(false)
                                .text_size(px(14.))
                                .bg(theme::TRANSPARENT),
                        ),
                    ),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S4)
                    .h(px(36.))
                    .px(theme::S4)
                    .border_t_1()
                    .border_color(theme::LINE)
                    .text_label()
                    .fg_muted()
                    .child(hint("↑↓", "move"))
                    .child(hint("↵", "open"))
                    .child(hint("esc", "close")),
            )
            .with_animation(
                anim_id.clone(),
                Animation::new(dur).with_easing(ease),
                move |d, t| {
                    let v = progress(t);
                    d.opacity(v).mt(px(8.) * (1.0 - v))
                },
            );

        let scrim_id = ElementId::NamedInteger(
            if closing {
                "scrim-out".into()
            } else {
                "scrim-in".into()
            },
            p.seq,
        );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .child(
                    div()
                        .id("palette-scrim")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(theme::SCRIM)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                        )
                        .with_animation(
                            scrim_id,
                            Animation::new(dur).with_easing(ease),
                            move |d, t| d.opacity(progress(t)),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(relative(0.12))
                        .left_0()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(panel),
                )
                .into_any_element(),
        )
    }

    fn render_toasts(toasts: &[Toast]) -> impl IntoElement {
        div()
            .absolute()
            .bottom(theme::S6)
            .right(theme::S6)
            .flex()
            .flex_col()
            .items_end()
            .gap(theme::S2)
            .children(toasts.iter().map(|t| {
                let id = t.id;
                let icon_name = match t.kind {
                    ToastKind::Success => AppIcon::CircleCheck,
                    ToastKind::Danger => AppIcon::CircleX,
                    ToastKind::Warning => AppIcon::Alert,
                    ToastKind::Info => AppIcon::Info,
                };
                let body = div()
                    .id(("toast", id as usize))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .max_w(px(440.))
                    .py(px(6.))
                    .pl(px(14.))
                    .pr(px(6.))
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .bg(theme::SURFACE_RAISED)
                    .shadow_lg()
                    .child(icon_sized(icon_name, theme::ICON).text_color(t.kind.color()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_body()
                            .fg()
                            .child(t.text.clone()),
                    )
                    .child(
                        IconButton::new(("toast-close", id as usize), AppIcon::X)
                            .small()
                            .tooltip("Dismiss")
                            .on_click(move |_, _, cx| {
                                AppState::global(cx).update(cx, |s, cx| s.dismiss_toast(id, cx))
                            }),
                    );
                if t.leaving {
                    body.with_animation(
                        ElementId::NamedInteger("toast-out".into(), id),
                        Animation::new(theme::DUR_EXIT).with_easing(theme::ease_in),
                        |d, t| d.opacity(1.0 - t).top(px(2.) * t),
                    )
                    .into_any_element()
                } else {
                    body.with_animation(
                        ElementId::NamedInteger("toast-in".into(), id),
                        Animation::new(theme::DUR_TOAST).with_easing(theme::ease_out),
                        |d, t| {
                            d.opacity(t)
                                .top(px(8.) * (1.0 - t))
                                .left(px(12.) * (1.0 - t))
                        },
                    )
                    .into_any_element()
                }
            }))
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = AppState::global(cx);
        let (route, nav_seq, toasts) = {
            let s = app.read(cx);
            (s.route().clone(), s.nav_seq(), s.toasts().to_vec())
        };
        if self.route_anim.0 != nav_seq {
            self.route_anim = (nav_seq, Instant::now());
        }
        let animating = self.route_anim.1.elapsed() < theme::DUR_ENTER * 2;
        let kind = route.kind();
        let view = self.screen(kind, window, cx);
        // Cache the screen (it re-renders only when it notifies) except while its enter
        // animation plays: cached paint would freeze the faded frame.
        let view = if animating {
            window.request_animation_frame();
            view
        } else {
            view.cached(StyleRefinement::default().size_full())
        };

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(theme::BG)
            .child(div().size_full().relative().child(view).with_animation(
                ElementId::NamedInteger("route".into(), nav_seq),
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |d, t| d.opacity(t).top(px(6.) * (1.0 - t)),
            ));

        let sidebar = self.render_sidebar(&route);
        let palette = self.render_palette(cx);

        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .bg(theme::BG)
            .font_family(theme::sans())
            .text_body()
            .text_color(theme::TEXT)
            .child(sidebar)
            .child(main)
            .children(palette)
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
            // Toasts stay above every overlay.
            .child(Self::render_toasts(&toasts))
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_prefers_word_starts() {
        let a = fuzzy_score("bug triage pipeline", "bt").unwrap();
        let b = fuzzy_score("about this", "bt").unwrap();
        assert!(a > b);
        assert!(fuzzy_score("review", "xyz").is_none());
        assert_eq!(fuzzy_score("anything", ""), Some(0));
    }
}
