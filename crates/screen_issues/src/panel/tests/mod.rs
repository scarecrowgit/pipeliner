//! The issue panel in a test window over the fixture backend, inside an overlay host, driven
//! by simulated clicks and keys.

mod activity;
mod comment;
mod props;
mod run;
mod texts;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, Modifiers, Render, Subscription, TestAppContext, VisualTestContext, Window, div,
    prelude::*, px, size,
};
use hub::fixtures::{FixtureApi, World};
use hub::model::IssueKey;
use hub::{Hub, Route};
use ui::OverlayHost;
use ui_rich::Composer;

use super::{IssuePanel, PanelEvent};

/// Width of the board's frame around the panel, px.
const FRAME: f32 = 400.;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;

/// The board's part: empty ground, the panel at the right, and a count of Close events.
struct Stage {
    panel: Entity<IssuePanel>,
    _sub: Subscription,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .flex()
            .flex_row()
            .child(div().flex_1())
            .child(div().flex_none().w(px(FRAME)).h_full().border_l_1().border_color(ui::theme::LINE).child(self.panel.clone()))
    }
}

/// An open panel.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub panel: Entity<IssuePanel>,
    pub composer: Entity<Composer>,
    pub closes: Rc<Cell<usize>>,
    _hub: Entity<Hub>,
}

pub(crate) fn open<'a>(cx: &'a mut TestAppContext, issue: &str) -> (Open, &'a mut VisualTestContext) {
    open_with(cx, issue, |_| {})
}

/// Like [`open`], after `edit` changed the world.
pub(crate) fn open_with<'a>(
    cx: &'a mut TestAppContext,
    issue: &str,
    edit: impl FnOnce(&mut World),
) -> (Open, &'a mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    api.with_world(edit);
    let hub = cx.update(|cx| Hub::new_fixture(api.clone(), cx));
    let key = IssueKey::new(issue);
    cx.update(|cx| hub::navigate(Route::Issues { issue: Some(key.clone()) }, cx));
    let slot = Rc::new(RefCell::new(None));
    let closes = Rc::new(Cell::new(0));
    let (put, count) = (slot.clone(), closes.clone());
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let panel = cx.new(|cx| IssuePanel::new(key, window, cx));
        *put.borrow_mut() = Some(panel.clone());
        let stage = cx.new(|cx| {
            let sub = cx.subscribe(&panel, move |_, _, event: &PanelEvent, _| {
                if *event == PanelEvent::Close {
                    count.set(count.get() + 1);
                }
            });
            Stage { panel, _sub: sub }
        });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(panel) = slot.borrow_mut().take() else { panic!("the panel was not built") };
    cx.simulate_resize(size(px(1440.), px(2400.)));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let composer = panel.read_with(cx, |p, _| p.composer().clone());
    (Open { api, panel, composer, closes, _hub: hub }, cx)
}

/// Clicks the element drawn with debug selector `selector`.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let bounds = cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// Moves the pointer over the element drawn with debug selector `selector`.
pub(crate) fn hover(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let bounds = cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    cx.run_until_parked();
}

/// An element with debug selector `selector` was drawn. GPUI keeps the bounds of elements
/// from earlier frames, so this only proves absence for elements never drawn before.
pub(crate) fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).is_some()
}

/// `debug_bounds` wants a `'static` selector; tests leak their few.
fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}

/// The details of every recorded call of `name`.
pub(crate) fn calls(api: &FixtureApi, name: &str) -> Vec<String> {
    api.recorded().into_iter().filter(|r| r.name == name).map(|r| r.detail).collect()
}

/// The composer's text.
pub(crate) fn text(t: &Open, cx: &mut VisualTestContext) -> String {
    t.composer.read_with(cx, |c, _| c.text().to_string())
}

/// Gives the composer keyboard focus.
pub(crate) fn focus_composer(t: &Open, cx: &mut VisualTestContext) {
    let composer = t.composer.clone();
    cx.update(|window, cx| composer.read(cx).focus(window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn close_emits_close_and_escape_does_too(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    assert!(shown(cx, "issue-panel"));
    click(cx, "issue-close");
    assert_eq!(t.closes.get(), 1);

    let panel = t.panel.clone();
    cx.update(|window, cx| window.focus(&panel.read(cx).focus.clone()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(t.closes.get(), 2, "Escape on the panel closes it");
}

#[gpui::test]
fn the_panel_fills_its_frame_and_takes_focus_only_when_asked(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    let bounds = cx.debug_bounds("issue-panel").unwrap_or_else(|| panic!("no panel"));
    assert_eq!(f32::from(bounds.size.width), FRAME - 1., "the frame less its border");
    let width = t.panel.read_with(cx, |p, _| p.content_width());
    assert_eq!(width, FRAME - 1. - 2. * ui::theme::SPACE_5, "text is laid out at the measured width");
    let focused = cx.update(|window, cx| t.composer.read(cx).is_focused(window, cx) || t.panel.read(cx).focus.is_focused(window));
    assert!(!focused, "opening the panel leaves focus where it was");

    let panel = t.panel.clone();
    cx.update(|window, cx| panel.read(cx).focus_comment(window, cx));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| t.composer.read(cx).is_focused(window, cx)));
}

#[gpui::test]
fn an_unknown_issue_says_so(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-9999");
    assert!(shown(cx, "issue-panel-state"));
    let missing = t.panel.read_with(cx, |p, cx| matches!(p.shown(cx), super::Shown::Missing));
    assert!(missing, "the issue is reported as not found");
    assert!(!shown(cx, "issue-prop-status"));
}

#[gpui::test]
fn an_issue_of_another_project_is_read_outside_the_board(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, "CHK-7", |w| {
        let mut issue = w.issues[0].clone();
        issue.key = IssueKey::new("CHK-7");
        issue.project = hub::api::ProjectId::new("checkout");
        issue.title = "Checkout copy".into();
        w.issues.push(issue);
    });
    let title = t.panel.read_with(cx, |p, cx| p.current(cx).map(|i| i.title.clone()));
    assert_eq!(title.as_deref(), Some("Checkout copy"));
    assert!(shown(cx, "issue-title"));
}
