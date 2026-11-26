//! Screens: one file per screen, each a gpui `Entity` view that the shell creates lazily
//! (first visit) and keeps alive, so its state (scroll, selection, inputs) survives
//! navigation. Each file is self-contained: a screen author edits only their file.
//!
//! # The pattern
//!
//! ```ignore
//! use crate::screens::prelude::*;
//!
//! pub struct BoardScreen {
//!     data: Option<BoardData>,      // None until the first load: render skeletons
//!     load: Option<Task<()>>,       // the in-flight load (dropping it cancels the apply)
//!     _subs: Vec<Subscription>,
//! }
//!
//! impl BoardScreen {
//!     /// Called once by the shell (`screens::create`). Keep this signature.
//!     pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
//!         let app = AppState::global(cx);
//!         let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _window, cx| match ev {
//!             // Re-query on store changes. `changes` says what changed if you want to filter
//!             // (e.g. only `Change::Message { thread_id }` of the open thread);
//!             // `Change::External` means "everything may have changed".
//!             AppEvent::StoreChanged { .. } => this.reload(cx),
//!             // Route ids (Review { task }, Chat { chat }, ...) select items in your screen.
//!             AppEvent::Navigated(Route::Board) => {}
//!             _ => {}
//!         });
//!         let mut this = BoardScreen { data: None, load: None, _subs: vec![sub] };
//!         this.reload(cx);
//!         this
//!     }
//!
//!     fn reload(&mut self, cx: &mut Context<Self>) {
//!         // Query the store on a background thread, apply on the UI thread, re-render.
//!         self.load = Some(load(cx, |store| BoardData::query(store), |this, data, _cx| this.data = Some(data)));
//!     }
//! }
//!
//! impl Render for BoardScreen {
//!     fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
//!         // render() must be cheap: read cached `self.data`, never query the store here.
//!         screen().child(PageHeader::new("Board").action(Button::new("new", "New task").primary()))
//!             .child(match &self.data {
//!                 None => body().child(skeleton_rows("board", 4)),
//!                 Some(d) if d.is_empty() => body().child(EmptyState::new(AppIcon::Board, "No tasks yet", "...")),
//!                 Some(d) => body().child(/* real content */),
//!             })
//!     }
//! }
//! ```
//!
//! ## What you can use
//!
//! * **State / engine** (`crate::state`): `state::store(cx)` (`Arc<Store>`, typed queries),
//!   `state::engine(cx)` (`Engine`: send, start_run, decide, deploy, ...),
//!   `state::run_engine(cx, |e| e.decide(&id, kind))` runs an engine call on a background
//!   thread and toasts errors — use it for every engine call that might block.
//! * **Navigation**: `state::navigate(Route::Review { task: Some(id) }, cx)`,
//!   `state::open_task(id, cx)` (task thread), `state::back(cx)`,
//!   `state::replace_route(..)` to change the id without adding history.
//!   Read the current route with `state::route(cx)`.
//! * **Toasts**: `state::toast(ToastKind::Success, "Deployed v15", cx)`.
//! * **Look**: colors, spacing, radii, motion from `crate::theme` (`theme::SURFACE`,
//!   `theme::S4`, `theme::RADIUS_XL`, `theme::DUR_ENTER`, `theme::ease_out`), text styles via
//!   `TextStyles` (`.text_body()`, `.text_label()`, `.text_mono()`, `.text_stat()`,
//!   `.fg_muted()`...). Never hardcode a hex color in a screen.
//! * **Components** from `crate::ui` (see its module docs): Button, IconButton, Badge,
//!   StatusPill/StatusDot, card/panel, SectionHeader, PageHeader, Tabs, EmptyState,
//!   Skeleton, ProgressBar, MeterRow, BarChart, Sparkline, DiffView, markdown, CodeBlock,
//!   `ui::input::{text_input, composer, field, composer_field}`, `ui::select`.
//! * **Layout helpers** below: [`screen`] (the full-height column every screen returns) and
//!   [`body`] (the scrolling content area with standard padding).
//!
//! ## Rules
//!
//! * No mock or sample data. With nothing to show, render an `EmptyState`.
//! * Memory and Policies are out of scope for this release (they say so).
//! * Screen views are cached by the shell: they only re-render when *they* call
//!   `cx.notify()` (loads do this for you). Time-relative text ("6m ago") needs its own
//!   timer if it must tick while nothing changes.
//! * Animations are part of the design: keep `StatusDot` pulses, `ProgressBar`s, enter
//!   animations. Give animated elements stable, unique ids.

pub mod agents;
pub mod board;
pub mod chat;
pub mod home;
pub mod memory;
pub mod new_task;
pub mod pipelines;
pub mod policies;
pub mod review;
pub mod settings;
pub mod task;
pub mod timeline;

use std::sync::Arc;

use gpui::{
    div, AnyView, AppContext as _, Context, Div, InteractiveElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, Task, Window,
};
use harness_core::Store;

use crate::state::{self, ScreenKind};
use crate::theme;

/// Everything a screen file usually needs.
pub mod prelude {
    #![allow(unused_imports)]
    pub use gpui::prelude::FluentBuilder as _;
    pub use gpui::{
        div, px, relative, AnyElement, App, AppContext as _, Context, Div, ElementId, Entity,
        InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
        StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    };
    pub use harness_core::{Change, Config, Id, Status, Store};

    pub use super::{body, load, screen};
    pub(crate) use crate::state::{self, AppEvent, AppState, Route, ToastKind};
    pub(crate) use crate::theme::{self, TextStyles as _};
    pub(crate) use crate::ui::{self, *};
}

/// Creates the view for a screen kind. Called by the shell once per kind.
pub fn create(kind: ScreenKind, window: &mut Window, cx: &mut gpui::App) -> AnyView {
    match kind {
        ScreenKind::Home => cx.new(|cx| home::HomeScreen::new(window, cx)).into(),
        ScreenKind::Review => cx.new(|cx| review::ReviewScreen::new(window, cx)).into(),
        ScreenKind::Chat => cx.new(|cx| chat::ChatScreen::new(window, cx)).into(),
        ScreenKind::Board => cx.new(|cx| board::BoardScreen::new(window, cx)).into(),
        ScreenKind::Timeline => cx
            .new(|cx| timeline::TimelineScreen::new(window, cx))
            .into(),
        ScreenKind::Pipelines => cx
            .new(|cx| pipelines::PipelinesScreen::new(window, cx))
            .into(),
        ScreenKind::Agents => cx.new(|cx| agents::AgentsScreen::new(window, cx)).into(),
        ScreenKind::Memory => cx.new(|cx| memory::MemoryScreen::new(window, cx)).into(),
        ScreenKind::Policies => cx
            .new(|cx| policies::PoliciesScreen::new(window, cx))
            .into(),
        ScreenKind::Settings => cx
            .new(|cx| settings::SettingsScreen::new(window, cx))
            .into(),
        ScreenKind::Task => cx.new(|cx| task::TaskScreen::new(window, cx)).into(),
    }
}

/// The root of every screen: a full-size column on the app background.
pub fn screen() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::BG)
        .text_color(theme::TEXT)
        .overflow_hidden()
}

/// The scrolling content area under the page header (24px padding, 24px gap).
pub fn body() -> Stateful<Div> {
    div()
        .id("screen-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(theme::S6)
        .p(theme::S6)
}

/// Runs `query` against the store on a background thread, then `apply`s the result on the
/// UI thread and re-renders. Keep the returned task in a field: replacing or dropping it
/// cancels a stale apply.
pub fn load<T: 'static, D: Send + 'static>(
    cx: &mut Context<T>,
    query: impl FnOnce(&Arc<Store>) -> D + Send + 'static,
    apply: impl FnOnce(&mut T, D, &mut Context<T>) + 'static,
) -> Task<()> {
    let store = state::store(cx);
    let job = cx.background_spawn(async move { query(&store) });
    cx.spawn(async move |this, cx| {
        let data = job.await;
        let _ = this.update(cx, |this, cx| {
            apply(this, data, cx);
            cx.notify();
        });
    })
}
