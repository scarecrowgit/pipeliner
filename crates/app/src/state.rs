//! Application state shared by every view: the engine, the current route and navigation
//! history, a store `revision` that bumps on every (coalesced) store change, the current
//! selection and the toast queue.
//!
//! There is exactly one [`AppState`] entity; it is installed as a global, so any view can
//! reach it with [`AppState::global`] or the free helpers in this module
//! ([`navigate`], [`back`], [`toast`], [`engine`], [`store`], [`run_engine`]).

// Shared API for screens: some tokens / helpers are not used by every build of the screens.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Task};
use harness_core::{Change, Id, Store};
use harness_engine::Engine;

use crate::theme;

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// Where the main area points. Ids select an item inside the screen (`None` = list / first).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    Review {
        task: Option<Id>,
    },
    Chat {
        chat: Option<Id>,
    },
    Board,
    Timeline,
    Pipelines {
        pipeline: Option<Id>,
    },
    Agents {
        agent: Option<Id>,
    },
    Memory,
    Policies,
    Settings,
    /// A task's thread (any task: pipeline step, chat task, spawned sub-agent).
    Task {
        task: Id,
    },
}

/// The screen a route renders; the shell keeps one cached screen entity per kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenKind {
    Home,
    Review,
    Chat,
    Board,
    Timeline,
    Pipelines,
    Agents,
    Memory,
    Policies,
    Settings,
    Task,
}

impl ScreenKind {
    pub fn title(self) -> &'static str {
        match self {
            ScreenKind::Home => "Home",
            ScreenKind::Review => "Review",
            ScreenKind::Chat => "Chat",
            ScreenKind::Board => "Board",
            ScreenKind::Timeline => "Timeline",
            ScreenKind::Pipelines => "Pipelines",
            ScreenKind::Agents => "Agents",
            ScreenKind::Memory => "Memory",
            ScreenKind::Policies => "Policies",
            ScreenKind::Settings => "Settings",
            ScreenKind::Task => "Task",
        }
    }
}

impl Route {
    pub fn kind(&self) -> ScreenKind {
        match self {
            Route::Home => ScreenKind::Home,
            Route::Review { .. } => ScreenKind::Review,
            Route::Chat { .. } => ScreenKind::Chat,
            Route::Board => ScreenKind::Board,
            Route::Timeline => ScreenKind::Timeline,
            Route::Pipelines { .. } => ScreenKind::Pipelines,
            Route::Agents { .. } => ScreenKind::Agents,
            Route::Memory => ScreenKind::Memory,
            Route::Policies => ScreenKind::Policies,
            Route::Settings => ScreenKind::Settings,
            Route::Task { .. } => ScreenKind::Task,
        }
    }

    pub fn title(&self) -> &'static str {
        self.kind().title()
    }

    /// Parses a screen name (`board`, `review`, ...) into its list route. Used by the
    /// `PIPELINER_ROUTE` environment variable to open the app on a given screen.
    pub fn from_name(name: &str) -> Option<Route> {
        Some(match name.trim().to_lowercase().as_str() {
            "home" => Route::Home,
            "review" => Route::Review { task: None },
            "chat" => Route::Chat { chat: None },
            "board" => Route::Board,
            "timeline" => Route::Timeline,
            "pipelines" => Route::Pipelines { pipeline: None },
            "agents" => Route::Agents { agent: None },
            "memory" => Route::Memory,
            "policies" => Route::Policies,
            "settings" => Route::Settings,
            _ => return None,
        })
    }

    /// Routes reachable with ctrl-1..9, in sidebar order.
    pub fn numbered() -> [Route; 9] {
        [
            Route::Home,
            Route::Review { task: None },
            Route::Chat { chat: None },
            Route::Board,
            Route::Timeline,
            Route::Pipelines { pipeline: None },
            Route::Agents { agent: None },
            Route::Memory,
            Route::Policies,
        ]
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Emitted by [`AppState`]. Screens `cx.subscribe(&AppState::global(cx), ...)` to react.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// The store changed (coalesced). `changes` lists what changed since the last event;
    /// it contains [`Change::External`] when another process wrote (re-query everything).
    StoreChanged {
        revision: u64,
        changes: Arc<Vec<Change>>,
    },
    /// The route changed (also emitted when only the route's id changed).
    Navigated(Route),
    /// ctrl/cmd-K: the shell opens (or closes) the command palette.
    PaletteRequested,
    /// Escape while an overlay (palette) is open: close it instead of going back.
    CloseOverlay,
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Danger,
}

impl ToastKind {
    pub fn color(self) -> gpui::Hsla {
        match self {
            ToastKind::Info => theme::INFO,
            ToastKind::Success => theme::SUCCESS,
            ToastKind::Warning => theme::WARNING,
            ToastKind::Danger => theme::DANGER,
        }
    }
    fn lifetime(self) -> Duration {
        match self {
            ToastKind::Danger | ToastKind::Warning => Duration::from_millis(6000),
            _ => Duration::from_millis(3500),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
    /// Set when the exit animation is playing; removed right after.
    pub leaving: bool,
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// Last selected ids. Updated from routes that carry an id; screens may also set them
/// (e.g. the Board remembers the card you opened).
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub task: Option<Id>,
    pub run: Option<Id>,
    pub chat: Option<Id>,
    pub pipeline: Option<Id>,
    pub agent: Option<Id>,
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

pub struct AppState {
    pub engine: Engine,
    route: Route,
    history: Vec<Route>,
    /// Increments on every navigation (drives the route enter animation).
    nav_seq: u64,
    /// Increments on every coalesced batch of store changes.
    revision: u64,
    pub selection: Selection,
    /// Set by the shell while the palette is open (Escape closes it instead of going back).
    pub overlay_open: bool,
    toasts: Vec<Toast>,
    next_toast: u64,
    _store_task: Task<()>,
}

struct GlobalAppState(Entity<AppState>);
impl Global for GlobalAppState {}

impl EventEmitter<AppEvent> for AppState {}

/// Minimum time between two `StoreChanged` notifications.
const COALESCE: Duration = Duration::from_millis(50);

impl AppState {
    /// Creates the state entity, installs it as a global and starts consuming store changes.
    pub fn init(engine: Engine, cx: &mut App) -> Entity<AppState> {
        let state = cx.new(|cx| AppState::new(engine, cx));
        cx.set_global(GlobalAppState(state.clone()));
        state
    }

    fn new(engine: Engine, cx: &mut Context<Self>) -> Self {
        let rx = engine.store().subscribe();
        let store_task = cx.spawn(async move |this, cx| {
            let mut last_emit = Instant::now() - COALESCE;
            loop {
                let Ok(first) = rx.recv().await else { break };
                let mut changes = vec![first];
                // Coalesce bursts (streaming tokens, bulk writes): wait out the rest of the
                // window, then drain everything that queued up meanwhile.
                let since = last_emit.elapsed();
                if since < COALESCE {
                    cx.background_executor().timer(COALESCE - since).await;
                }
                while let Ok(c) = rx.try_recv() {
                    if !changes.contains(&c) {
                        changes.push(c);
                    }
                }
                last_emit = Instant::now();
                let changes = Arc::new(changes);
                let alive = this.update(cx, |state, cx| {
                    state.revision += 1;
                    cx.emit(AppEvent::StoreChanged {
                        revision: state.revision,
                        changes,
                    });
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let route = std::env::var("PIPELINER_ROUTE")
            .ok()
            .and_then(|r| Route::from_name(&r))
            .unwrap_or(Route::Home);
        AppState {
            engine,
            route,
            history: Vec::new(),
            nav_seq: 0,
            revision: 0,
            selection: Selection::default(),
            overlay_open: false,
            toasts: Vec::new(),
            next_toast: 1,
            _store_task: store_task,
        }
    }

    /// The single app state entity.
    pub fn global(cx: &App) -> Entity<AppState> {
        cx.global::<GlobalAppState>().0.clone()
    }

    pub fn route(&self) -> &Route {
        &self.route
    }
    pub fn nav_seq(&self) -> u64 {
        self.nav_seq
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty()
    }
    pub fn store(&self) -> &Arc<Store> {
        self.engine.store()
    }
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Goes to `route`, pushing the current one on the back stack.
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == self.route {
            return;
        }
        let prev = std::mem::replace(&mut self.route, route.clone());
        // Only animate when the screen changes; id changes inside a screen are instant.
        if prev.kind() != route.kind() {
            self.nav_seq += 1;
        }
        self.history.push(prev);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.remember(&route);
        cx.emit(AppEvent::Navigated(route));
        cx.notify();
    }

    /// Replaces the current route without touching history (e.g. selecting a list item).
    pub fn replace_route(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == self.route {
            return;
        }
        if self.route.kind() != route.kind() {
            self.nav_seq += 1;
        }
        self.route = route.clone();
        self.remember(&route);
        cx.emit(AppEvent::Navigated(route));
        cx.notify();
    }

    /// Asks the shell to toggle the command palette.
    pub fn request_palette(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::PaletteRequested);
    }

    /// Escape: closes an open overlay, else goes back.
    pub fn escape(&mut self, cx: &mut Context<Self>) {
        if self.overlay_open {
            cx.emit(AppEvent::CloseOverlay);
        } else {
            self.back(cx);
        }
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        let Some(route) = self.history.pop() else {
            return;
        };
        if route.kind() != self.route.kind() {
            self.nav_seq += 1;
        }
        self.route = route.clone();
        self.remember(&route);
        cx.emit(AppEvent::Navigated(route));
        cx.notify();
    }

    fn remember(&mut self, route: &Route) {
        match route {
            Route::Review { task: Some(t) } | Route::Task { task: t } => {
                self.selection.task = Some(t.clone())
            }
            Route::Chat { chat: Some(c) } => self.selection.chat = Some(c.clone()),
            Route::Pipelines { pipeline: Some(p) } => self.selection.pipeline = Some(p.clone()),
            Route::Agents { agent: Some(a) } => self.selection.agent = Some(a.clone()),
            _ => {}
        }
    }

    /// Shows a toast bottom-right; it dismisses itself.
    pub fn toast(
        &mut self,
        kind: ToastKind,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast {
            id,
            kind,
            text: text.into(),
            leaving: false,
        });
        // Keep the stack short.
        while self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        cx.notify();
        let lifetime = kind.lifetime();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(lifetime).await;
            let _ = this.update(cx, |s, cx| s.dismiss_toast(id, cx));
        })
        .detach();
    }

    /// Plays the exit animation, then removes the toast.
    pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(t) = self.toasts.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if t.leaving {
            return;
        }
        t.leaving = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(theme::DUR_EXIT).await;
            let _ = this.update(cx, |s, cx| {
                s.toasts.retain(|t| t.id != id);
                cx.notify();
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------------------
// Free helpers (usable from any view's listener with `cx: &mut App` / `&mut Context<T>`)
// ---------------------------------------------------------------------------

/// The engine handle (cheap clone).
pub fn engine(cx: &App) -> Engine {
    AppState::global(cx).read(cx).engine.clone()
}

/// The store (cheap clone of the `Arc`).
pub fn store(cx: &App) -> Arc<Store> {
    AppState::global(cx).read(cx).engine.store().clone()
}

/// Current store revision (compare with a cached value to know whether to re-query).
pub fn revision(cx: &App) -> u64 {
    AppState::global(cx).read(cx).revision
}

/// Current route.
pub fn route(cx: &App) -> Route {
    AppState::global(cx).read(cx).route.clone()
}

pub fn navigate(route: Route, cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.navigate(route, cx));
}

pub fn replace_route(route: Route, cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.replace_route(route, cx));
}

pub fn back(cx: &mut App) {
    AppState::global(cx).update(cx, |s, cx| s.back(cx));
}

/// Opens a task's thread.
pub fn open_task(task: impl Into<Id>, cx: &mut App) {
    navigate(Route::Task { task: task.into() }, cx);
}

pub fn toast(kind: ToastKind, text: impl Into<SharedString>, cx: &mut App) {
    let text = text.into();
    AppState::global(cx).update(cx, |s, cx| s.toast(kind, text, cx));
}

/// Runs a (possibly slow, blocking) engine call on a background thread.
/// Errors are shown as a danger toast and yield `None`. `.detach()` the task for
/// fire-and-forget, or await it inside `cx.spawn` to use the result.
///
/// ```ignore
/// state::run_engine(cx, move |e| e.deploy(&id)).detach();
/// ```
pub fn run_engine<R: Send + 'static>(
    cx: &mut App,
    f: impl FnOnce(&Engine) -> anyhow::Result<R> + Send + 'static,
) -> Task<Option<R>> {
    let engine = engine(cx);
    let job = cx.background_spawn(async move { f(&engine) });
    let state = AppState::global(cx);
    cx.spawn(async move |cx| match job.await {
        Ok(r) => Some(r),
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = state.update(cx, |s, cx| s.toast(ToastKind::Danger, msg, cx));
            None
        }
    })
}
