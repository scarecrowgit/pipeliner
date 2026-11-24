//! The transcript in a test window over the fixture backend: blocks of every kind, streaming,
//! following and scrolling, paging, permission answers, folding and sub-agent links.

mod blocks;
mod heights;
mod permissions;
mod scrolling;

use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, Modifiers, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, TestAppContext,
    TouchPhase, VisualTestContext, Window, div, point, prelude::*, px, size,
};
use hub::Hub;
use hub::fixtures::{FixtureApi, World};
use hub::model::{AgentEvent, Event, SessionId};
use hub::transcript::BlockKind;

use super::TranscriptView;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;
/// The "Fix double charge on retry" chat of the mockup.
pub(crate) const FIX: &str = "chat-1423";
/// A 96-message chat, for paging.
pub(crate) const LEDGER: &str = "chat-ledger";

/// A window root holding only the transcript.
struct Stage {
    view: Entity<TranscriptView>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().flex().child(div().flex_1().h_full().flex().flex_col().child(self.view.clone()))
    }
}

/// An open transcript and the fixture behind it.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub view: Entity<TranscriptView>,
}

/// The transcript of `session` over the Acme world, loaded, in a window of `height` px.
pub(crate) fn open<'a>(cx: &'a mut TestAppContext, session: &str, height: f32) -> (Open, &'a mut VisualTestContext) {
    open_with(cx, session, height, |_| {})
}

/// [`open`] over the Acme world as `edit` changed it.
pub(crate) fn open_with<'a>(
    cx: &'a mut TestAppContext,
    session: &str,
    height: f32,
    edit: impl FnOnce(&mut World),
) -> (Open, &'a mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    ui::motion::set_reduced(true);
    let mut world = World::acme(NOW);
    edit(&mut world);
    let api = Arc::new(FixtureApi::new(world));
    let _hub = cx.update(|cx| Hub::new(api.clone(), cx));
    let id = SessionId::new(session);
    let (stage, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| TranscriptView::new(id, window, cx));
        Stage { view }
    });
    cx.simulate_resize(size(px(1200.), px(height)));
    let view = stage.read_with(cx, |stage, _| stage.view.clone());
    view.update(cx, |view, cx| view.set_now(NOW, cx));
    settle(cx);
    (Open { api, view }, cx)
}

/// Runs everything pending and draws a frame.
pub(crate) fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    cx.run_until_parked();
}

/// Sends agent events of `session` and lets them land.
pub(crate) fn emit(open: &Open, cx: &mut VisualTestContext, session: &str, events: Vec<AgentEvent>) {
    open.api.emit_all(events.into_iter().map(|event| Event::Agent { session: SessionId::new(session), event }));
    settle(cx);
}

/// Kinds of the model's blocks, in order.
pub(crate) fn kinds(open: &Open, cx: &mut VisualTestContext) -> Vec<&'static str> {
    open.view.read_with(cx, |view, cx| {
        view.model
            .read(cx)
            .blocks()
            .iter()
            .map(|block| match &block.kind {
                BlockKind::User(_) => "user",
                BlockKind::Assistant(_) => "assistant",
                BlockKind::Thinking(_) => "thinking",
                BlockKind::Tool(_) => "tool",
                BlockKind::Subagent(_) => "subagent",
                BlockKind::Todos(_) => "todos",
                BlockKind::Permission(_) => "permission",
                BlockKind::Compaction(_) => "compaction",
                BlockKind::Notice(_) => "notice",
                BlockKind::Failure(_) => "failure",
            })
            .collect()
    })
}

/// Rows of the list and blocks of the model: the list always has one more (the footer).
pub(crate) fn counts(open: &Open, cx: &mut VisualTestContext) -> (usize, usize) {
    open.view.read_with(cx, |view, cx| (view.list.item_count(), view.model.read(cx).blocks().len()))
}

/// The selector of row `index`, for `debug_bounds`.
pub(crate) fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// Whether element `name` was laid out in the last frame.
pub(crate) fn drawn(cx: &mut VisualTestContext, name: String) -> bool {
    cx.debug_bounds(selector(name)).is_some()
}

/// Clicks the element drawn under `name`.
pub(crate) fn click(cx: &mut VisualTestContext, name: &str) {
    let bounds = cx.debug_bounds(selector(name.to_string())).unwrap_or_else(|| panic!("`{name}` was not drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    settle(cx);
}

/// Turns the wheel over the middle of the window: positive `dy` moves toward older rows.
pub(crate) fn wheel(cx: &mut VisualTestContext, dy: f32) {
    let position: Point<Pixels> = point(px(600.), px(300.));
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    settle(cx);
}
