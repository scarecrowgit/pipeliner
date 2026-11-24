//! The sidebar (`.sb`) and its collapsed form, the 56px icon rail (`.rail`).
//!
//! Everything it shows comes from hub models: the chat index (pinned and recent chats, the
//! open chat's sub-agents, "N waiting"), the inbox ("4"), the catalog and runs (the pipelines
//! list), and ops (project, spend, runtimes, the collapse setting). Clicks navigate through
//! the router or call [`hub::Commands`].
//!
//! Probes: `sidebar-project`, `sidebar-collapse`, `sidebar-expand`, `sidebar-search`,
//! `sidebar-new-chat`, `sidebar-nav-<home|inbox|chats|issues|agents>`,
//! `sidebar-pipelines-all`, `sidebar-pipeline-<id>`, `sidebar-chats-all`, `sidebar-chat-<id>`
//! with `sidebar-chat-<id>-pin` and `-archive` on hover, `sidebar-spend`.

mod chats;
mod foot;
pub(crate) mod limits;
pub(crate) mod projects;
mod rail;

use gpui::{
    AnimationExt as _, AnyElement, App, Context, ElementId, Entity, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use hub::{CatalogModel, ChatIndex, InboxModel, OpsModel, Route, Router, RunsModel, Section};
use model::{PipelineId, RunnableKind, RunnableRef, RunnableState, SessionId, SessionState};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use crate::collapse;
use crate::screen::section_label;

/// Recent chats listed before "View all".
const RECENT_ROWS: usize = 30;

/// What the last frame of the sidebar showed, for tests and checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The highlighted nav entry.
    pub active: Option<Section>,
    /// Drawn as the rail.
    pub collapsed: bool,
    /// The Inbox count ("4"), 0 when none is drawn.
    pub inbox_waiting: usize,
    /// The Chats hint ("1 waiting"), 0 when none is drawn.
    pub chats_waiting: u64,
    /// The highlighted chat row.
    pub current_chat: Option<SessionId>,
    /// Every chat row with its state, top to bottom.
    pub chats: Vec<(SessionId, SessionState)>,
    /// Every pipeline row: its id and the text at its right ("37 queued", "v3", "draft").
    pub pipelines: Vec<(String, String)>,
}

/// The sidebar view. Build it with [`Sidebar::new`] after [`hub::Hub::new`].
pub struct Sidebar {
    router: Entity<Router>,
    chats: Entity<ChatIndex>,
    inbox: Entity<InboxModel>,
    ops: Entity<OpsModel>,
    runs: Entity<RunsModel>,
    catalog: Entity<CatalogModel>,
    shown: Shown,
    _subscriptions: Vec<Subscription>,
}

impl Sidebar {
    /// A sidebar over the hub's models (loading them if nobody did yet).
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let router = hub::router(cx);
        let chats = hub::chat_index(cx);
        let inbox = hub::inbox(cx);
        let ops = hub::ops(cx);
        let runs = hub::runs(cx);
        let catalog = hub::catalog(cx);
        let subscriptions = vec![
            cx.observe(&router, |_, _, cx| cx.notify()),
            cx.observe(&chats, |_, _, cx| cx.notify()),
            cx.observe(&inbox, |_, _, cx| cx.notify()),
            cx.observe(&runs, |_, _, cx| cx.notify()),
            cx.observe(&catalog, |_, _, cx| cx.notify()),
            cx.observe(&ops, |_, ops, cx| {
                collapse::settle(&ops, cx);
                cx.notify();
            }),
            cx.observe_global::<collapse::Pending>(|_, cx| cx.notify()),
        ];
        ops.update(cx, |ops, cx| ops.ensure_schedules(cx));
        Self {
            router,
            chats,
            inbox,
            ops,
            runs,
            catalog,
            shown: Shown::default(),
            _subscriptions: subscriptions,
        }
    }

    /// What the last frame showed.
    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    fn active(&self, cx: &App) -> Section {
        self.router.read(cx).current().section()
    }

    fn open_chat(&self, cx: &App) -> Option<SessionId> {
        match self.router.read(cx).current() {
            Route::Chat { session } => Some(session.clone()),
            _ => None,
        }
    }

    fn open_pipeline(&self, cx: &App) -> Option<PipelineId> {
        match self.router.read(cx).current() {
            Route::Editor { pipeline, .. } => Some(pipeline.clone()),
            _ => None,
        }
    }

    fn project_name(&self, cx: &App) -> SharedString {
        self.ops.read(cx).current_project().map(|p| p.name.clone()).unwrap_or_else(|| "No project".into()).into()
    }

    /// The pipelines list, in catalog order.
    fn pipelines(&self, cx: &App) -> Vec<PipelineRow> {
        let catalog = self.catalog.read(cx);
        let open = self.runs.read(cx).open_runs();
        let ops = self.ops.read(cx);
        let now = base::time::now_ms();
        catalog
            .of_kind(RunnableKind::Pipeline)
            .filter(|runnable| !runnable.archived)
            .map(|runnable| {
                let reference = runnable.reference.clone();
                let activity = catalog.activity(&reference);
                let running = open.iter().filter(|run| run.pipeline.as_str() == reference.id()).count();
                let queued = activity.map_or(0, |a| a.queued);
                let state = activity.map(|a| a.state.clone()).unwrap_or_default();
                let next = ops.next_scheduled_run(&reference).filter(|at| *at >= now);
                let (status, meta) = pipeline_status(runnable.paused, &state, queued, running);
                // A deployed pipeline at rest shows when its schedule next runs it ("02:00").
                let meta = match (status, next) {
                    (Status::Idle, Some(at)) if matches!(state, RunnableState::Live { .. }) => Some(next_run_label(now, at).into()),
                    _ => meta,
                };
                PipelineRow { reference, name: runnable.name.clone().into(), status, meta }
            })
            .collect()
    }

    fn render_full(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active(cx);
        let inbox_waiting = self.inbox.read(cx).waiting();
        let chats_waiting = self.chats.read(cx).waiting();
        self.shown.inbox_waiting = inbox_waiting;
        self.shown.chats_waiting = chats_waiting;

        let nav = col().flex_none().gap(px(1.)).children(NAV.iter().map(|entry| {
            let route = entry.route;
            let mut item = NavItem::new(entry.probe, entry.icon, section_label(entry.section))
                .current(active == entry.section)
                .on_click(move |_, _, cx| hub::navigate(route(), cx));
            if entry.section == Section::Inbox && inbox_waiting > 0 {
                item = item.count(base::fmt::count(inbox_waiting as u64));
            }
            if entry.section == Section::Chats && chats_waiting > 0 {
                item = item.hint(format!("{chats_waiting} waiting"));
            }
            item.probe(entry.probe)
        }));

        let pipelines = self.pipelines(cx);
        self.shown.pipelines =
            pipelines.iter().map(|row| (row.reference.id().to_string(), row.meta.clone().unwrap_or_default().to_string())).collect();
        let open_pipeline = self.open_pipeline(cx);
        let pipeline_section = col()
            .flex_none()
            .mt(px(SPACE_4))
            .gap(px(1.))
            .child(section_head(
                "Pipelines",
                "sidebar-pipelines-all",
                base::fmt::count(pipelines.len() as u64),
                Route::Agents { runnable: None },
                window,
                cx,
            ))
            .children(pipelines.into_iter().map(|row| {
                let probe: SharedString = format!("sidebar-pipeline-{}", row.reference.id()).into();
                let pipeline = PipelineId::new(row.reference.id());
                let current = open_pipeline.as_ref() == Some(&pipeline);
                ListItem::new(ElementId::Name(probe.clone()), row.name)
                    .dot(row.status)
                    .when_some(row.meta, |item, meta| item.meta(meta))
                    .current(current)
                    .on_click(move |_, _, cx| {
                        hub::navigate(Route::Editor { pipeline: pipeline.clone(), node: None }, cx)
                    })
                    .probe(probe)
            }));

        let chats = self.render_chats(window, cx);
        let scroll = div()
            .id("sidebar-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .mx(px(-SPACE_2))
            .px(px(SPACE_2))
            .child(pipeline_section)
            .child(chats);

        col()
            .id("sidebar")
            .debug_selector(|| "sidebar".into())
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .pt(px(SPACE_2))
            .px(px(SPACE_2))
            .pb(px(SPACE_3))
            .child(self.render_top(window, cx))
            .child(self.render_actions())
            .child(nav)
            .child(scroll)
            .child(self.render_foot(window, cx))
            .into_any_element()
    }

    /// `.sb-top`: the project switcher button and the collapse button.
    fn render_top(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = ui::overlays(window, cx).is_open(projects::ANCHOR, cx);
        let rest = Look::bg(if open { SURFACE_HOVER } else { TRANSPARENT });
        let hover = Look::bg(if open { SURFACE_HOVER } else { SURFACE_RAISED });
        let faded = motion::hover_fade_state("sidebar-project", rest, hover, window, cx);
        let project = row()
            .id("sidebar-project")
            .debug_selector(|| "sidebar-project".into())
            .flex_1()
            .gap(px(SPACE_2))
            .h(px(CONTROL_HEIGHT_MD))
            .px(px(SPACE_1_5))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .type_style(BODY_STRONG)
            .on_click(move |_, window, cx| projects::open(window, cx));
        let project = faded
            .apply(project)
            .child(logo(20.))
            .child(truncate(self.project_name(cx)))
            .child(icon(IconName::ChevronDown, 14., TEXT_MUTED))
            .popover_anchor(projects::ANCHOR)
            .probe("sidebar-project");
        row()
            .flex_none()
            .h(px(CONTROL_HEIGHT))
            .gap(px(SPACE_1))
            .child(project)
            .child(
                IconButton::new("sidebar-collapse", IconName::Panel)
                    .tooltip("Collapse sidebar · ⌘\\")
                    .on_click(|_, _, cx| collapse::toggle(cx))
                    .probe("sidebar-collapse"),
            )
    }

    /// `.sb-actions`: the search field that opens the palette, and New chat.
    fn render_actions(&self) -> impl IntoElement {
        let search = div()
            .id("sidebar-search")
            .debug_selector(|| "sidebar-search".into())
            .flex()
            .flex_1()
            .items_center()
            .min_w_0()
            .gap(px(SPACE_2))
            .h(px(CONTROL_HEIGHT_MD))
            .pl(px(SPACE_2_5))
            .pr(px(SPACE_1_5))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_LG))
            .bg(SURFACE_RAISED)
            .text_color(TEXT_MUTED)
            .cursor_pointer()
            .on_click(|_, _, cx| open_palette(cx));
        let search = motion::fade(
            search,
            "sidebar-search-fade",
            Look::bg(SURFACE_RAISED).fg(TEXT_MUTED),
            Look::bg(SURFACE_RAISED).fg(TEXT_SECONDARY),
        )
        .child(icon(IconName::Search, 14., TEXT_MUTED))
        .child(div().flex_1().child("Search"))
        .child(Kbd::new("⌘K"))
        .probe("sidebar-search");
        row()
            .flex_none()
            .gap(px(SPACE_1_5))
            .pt(px(SPACE_2))
            .pb(px(SPACE_2_5))
            .child(search)
            .child(
                IconButton::new("sidebar-new-chat", IconName::Edit)
                    .bordered()
                    .tooltip("New chat · ⌘N")
                    .on_click(|_, _, cx| hub::navigate(Route::NewChat, cx))
                    .probe("sidebar-new-chat"),
            )
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = collapse::is_collapsed(self.ops.read(cx), cx);
        self.shown.collapsed = collapsed;
        self.shown.active = Some(self.active(cx));
        self.shown.current_chat = self.open_chat(cx);
        let body = if collapsed { self.render_rail(window, cx) } else { self.render_full(window, cx) };
        // Swapping forms fades the new one in; GPUI 0.2.2 has no transforms, and widths do
        // not animate (GRAPHITE.md, "Motion").
        if motion::reduced() {
            return body;
        }
        div()
            .flex()
            .flex_none()
            .h_full()
            .child(body)
            .with_animation(
                ElementId::NamedInteger("sidebar-form".into(), u64::from(collapsed)),
                gpui::Animation::new(motion::scaled(motion::DURATION_STATE)).with_easing(motion::ease_standard),
                |el, t| el.opacity(t),
            )
            .into_any_element()
    }
}

/// Open the palette over the current screen (the search field, the rail button and ⌘K).
pub(crate) fn open_palette(cx: &mut App) {
    hub::router(cx).update(cx, |router, cx| router.open_overlay(hub::Overlay::Palette, cx));
}

/// One entry of the primary nav.
struct Nav {
    section: Section,
    icon: IconName,
    probe: &'static str,
    route: fn() -> Route,
}

const NAV: [Nav; 5] = [
    Nav { section: Section::Home, icon: IconName::Home, probe: "sidebar-nav-home", route: || Route::Home },
    Nav { section: Section::Inbox, icon: IconName::Inbox, probe: "sidebar-nav-inbox", route: || Route::Inbox { item: None } },
    Nav { section: Section::Chats, icon: IconName::Chat, probe: "sidebar-nav-chats", route: || Route::Chats },
    Nav { section: Section::Issues, icon: IconName::Issues, probe: "sidebar-nav-issues", route: || Route::Issues { issue: None } },
    Nav {
        section: Section::Agents,
        icon: IconName::Orchestrator,
        probe: "sidebar-nav-agents",
        route: || Route::Agents { runnable: None },
    },
];

/// A row of the pipelines list.
pub(crate) struct PipelineRow {
    pub reference: RunnableRef,
    pub name: SharedString,
    pub status: Status,
    pub meta: Option<SharedString>,
}

/// A pipeline's dot and meta: live with its queue while items wait, running while runs are
/// open, else its deployed version (grey), a draft ring, or paused.
fn pipeline_status(paused: bool, state: &RunnableState, queued: u32, running: usize) -> (Status, Option<SharedString>) {
    if paused || matches!(state, RunnableState::Paused) {
        return (Status::Idle, Some("paused".into()));
    }
    match state {
        RunnableState::Draft { .. } => (Status::Draft, Some("draft".into())),
        _ if queued > 0 => (Status::Live, Some(format!("{} queued", base::fmt::count(u64::from(queued))).into())),
        _ if running > 0 => (Status::Running, Some(format!("{running} running").into())),
        RunnableState::Live { version } => (Status::Idle, Some(format!("v{version}").into())),
        _ => (Status::Idle, None),
    }
}

/// When a schedule next runs a pipeline: "02:00" within a day, else the weekday ("Wed").
pub(crate) fn next_run_label(now: i64, at: i64) -> String {
    let offset = base::time::utc_offset(at);
    if at - now < 24 * 60 * 60 * 1000 {
        base::time::clock_time(at, offset)
    } else {
        base::time::weekday_short(base::time::civil(at, offset).weekday).to_string()
    }
}

/// The app mark on its accent tile (`.logo`): 20px in the sidebar, 24px on the rail.
pub(crate) fn logo(size: f32) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(RADIUS_MD))
        .bg(ACCENT_STRONG)
        .child(icon(IconName::Mark, 12., ON_ACCENT))
}

/// `.sb-lbl`: a section label with a link at the right ("Pipelines 3", "Chats View all 2,418").
fn section_head(
    label: &'static str,
    probe: &'static str,
    link_text: String,
    route: Route,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let faded = motion::hover_fade_state(
        probe,
        Look::bg(TRANSPARENT).fg(TEXT_MUTED),
        Look::bg(TRANSPARENT).fg(TEXT),
        window,
        cx,
    );
    let link = div()
        .id(probe)
        .debug_selector(move || probe.into())
        .flex_none()
        .tabular()
        .cursor_pointer()
        .on_click(move |_, _, cx| hub::navigate(route.clone(), cx));
    row()
        .flex_none()
        .gap(px(SPACE_1_5))
        .h(px(SPACE_6))
        .px(px(SPACE_2))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(label)
        .child(grow())
        .child(faded.apply(link).child(SharedString::from(link_text)).probe(probe))
}

/// `.sb-sub`: a small label inside a section ("Pinned", "Recent").
fn sub_head(label: &'static str, pin: bool) -> impl IntoElement {
    row()
        .flex_none()
        .gap(px(SPACE_1_5))
        .h(px(22.))
        .pt(px(SPACE_1))
        .px(px(SPACE_2))
        .type_style(CAPTION)
        .text_color(TEXT_MUTED)
        .when(pin, |d| d.child(icon(IconName::Pin, 12., TEXT_MUTED)))
        .child(label)
}
