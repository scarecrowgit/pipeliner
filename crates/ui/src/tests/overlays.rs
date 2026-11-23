//! The overlay host in a test window: popovers, menus, dialogs and toasts opened the way a
//! screen opens them, driven by simulated clicks and keys.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, KeyUpEvent, Keystroke, Modifiers, Render,
    TestAppContext, VisualTestContext, Window, div, point, prelude::*, px,
};

use crate::OverlayHost;
use crate::keys;
use crate::overlay::current_layer;
use crate::prelude::*;

pub(super) type Log = Rc<RefCell<Vec<String>>>;

fn push(log: &Log, s: &str) {
    log.borrow_mut().push(s.to_string());
}

/// A page with triggers at fixed places, so popovers never cover what a test clicks.
struct Page {
    log: Log,
    custom: FocusHandle,
    d1: FocusHandle,
    d2: FocusHandle,
    outside: FocusHandle,
}

fn menu_a(log: Log) -> impl Fn(&mut Window, &mut App) -> Menu + 'static {
    move |_, _| {
        let row = |id: &'static str, label: &'static str| {
            let log = log.clone();
            MenuItem::new(id, label).on_click(move |_, _, _| push(&log, id))
        };
        Menu::new("menu-a")
            .width(240.)
            .label("New")
            .item(row("agent", "Agent"))
            .item(row("archive", "Archive").coming_soon())
            .separator()
            .item(row("orchestrator", "Orchestrator"))
            .item(row("pipeline", "Pipeline"))
    }
}

fn simple_popover(name: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
    move |_, _| Popover::new().width(200.).title(name).child(div().h(px(40.))).into_any_element()
}

fn open_dialog(page: &Page, window: &mut Window, cx: &mut App) {
    let (d1, d2, log) = (page.d1.clone(), page.d2.clone(), page.log.clone());
    overlays(window, cx).open_dialog(
        "dlg",
        move |_, _| {
            assert!(current_layer().is_some(), "built inside a layer");
            let log = log.clone();
            Dialog::new("dlg", "Deploy v15")
                .width(400.)
                .top(200.)
                .no_close_button()
                .child(div().id("d1").debug_selector(|| "d1".into()).size(px(20.)).track_focus(&d1.clone().tab_index(0).tab_stop(true)))
                .child(div().id("d2").size(px(20.)).track_focus(&d2.clone().tab_index(0).tab_stop(true)))
                .child(
                    Button::new("in-dlg", "Pick")
                        .on_click(|_, window, cx| {
                            let anchor = Anchor::of("in-dlg", window, cx);
                            overlays(window, cx).open_popover(anchor, Placement::below(), simple_popover("inner"), window, cx);
                        })
                        .popover_anchor("in-dlg"),
                )
                .on_confirm(move |_, _| push(&log, "confirm"))
                .into_any_element()
        },
        window,
        cx,
    );
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        let at = |x: f32, y: f32, el: AnyElement| div().absolute().left(px(x)).top(px(y)).child(el);
        let trigger = |id: &'static str, log: &Log| {
            let log = log.clone();
            Button::new(id, id)
                .on_click(move |_, window, cx| {
                    push(&log, id);
                    let anchor = Anchor::of(id, window, cx);
                    overlays(window, cx).open_menu(anchor, Placement::below(), menu_a(log.clone()), window, cx);
                })
                .popover_anchor(id)
                .into_any_element()
        };
        let under = {
            let log = log.clone();
            Button::new("under", "Under").on_click(move |_, _, _| push(&log, "under")).into_any_element()
        };
        let dlg = Button::new("open-dlg", "Dialog")
            .on_click(cx.listener(|this, _, window, cx| open_dialog(this, window, cx)))
            .into_any_element();
        let custom = div()
            .id("custom")
            .debug_selector(|| "custom".into())
            .size(px(40.))
            .track_focus(&self.custom)
            .on_click(|_, window, cx| {
                overlays(window, cx).open_popover(
                    Anchor::point(point(px(400.), px(400.))),
                    Placement::below(),
                    simple_popover("custom"),
                    window,
                    cx,
                );
            })
            .into_any_element();
        let outside = div().id("outside").size(px(20.)).track_focus(&self.outside.clone().tab_index(0).tab_stop(true)).into_any_element();
        div()
            .size_full()
            .relative()
            .child(at(20., 20., trigger("a", &log)))
            .child(at(300., 20., trigger("b", &log)))
            .child(at(700., 500., under))
            .child(at(700., 20., dlg))
            .child(at(900., 20., custom))
            .child(at(900., 300., outside))
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Page>, VisualTestContext) {
    cx.update(keys::init);
    let log: Log = Rc::default();
    let (host, vcx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|cx| Page {
            log,
            custom: cx.focus_handle(),
            d1: cx.focus_handle(),
            d2: cx.focus_handle(),
            outside: cx.focus_handle(),
        });
        OverlayHost::new(window, cx).with_content(page)
    });
    let mut vcx = vcx.clone();
    let content = vcx.update(|_, cx| host.read(cx).content.clone()).expect("content");
    let page = content.downcast::<Page>().unwrap_or_else(|_| panic!("a page"));
    vcx.run_until_parked();
    (page, vcx)
}

pub(super) fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

pub(super) fn key_up(cx: &mut VisualTestContext, key: &str) {
    cx.simulate_event(KeyUpEvent { keystroke: Keystroke::parse(key).expect("a key") });
    cx.run_until_parked();
}

fn is_open(cx: &mut VisualTestContext, id: &'static str) -> bool {
    cx.update(|window, cx| overlays(window, cx).is_open(id, cx))
}

fn log_of(page: &Entity<Page>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| page.read(cx).log.borrow().clone())
}

#[gpui::test]
fn opening_a_second_popover_closes_the_first(cx: &mut TestAppContext) {
    let (_, mut cx) = setup(cx);
    click(&mut cx, "a");
    assert!(is_open(&mut cx, "a"));
    cx.update(|window, cx| {
        let anchor = Anchor::of("b", window, cx);
        overlays(window, cx).open_menu(anchor, Placement::below(), menu_a(Rc::default()), window, cx);
    });
    cx.run_until_parked();
    assert!(!is_open(&mut cx, "a"), "the first popover closed");
    assert!(is_open(&mut cx, "b"));
    assert_eq!(cx.update(|window, cx| overlays(window, cx).open_count(cx)), 1);
}

#[gpui::test]
fn a_click_away_closes_without_activating_what_is_under_it(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "a");
    assert!(is_open(&mut cx, "a"));
    click(&mut cx, "under");
    assert!(!is_open(&mut cx, "a"));
    assert_eq!(log_of(&page, &mut cx), vec!["a"], "the press only closed the popover");
    // Pressing the trigger of an open popover closes it, it does not reopen it.
    click(&mut cx, "a");
    click(&mut cx, "a");
    assert!(!is_open(&mut cx, "a"));
    assert_eq!(log_of(&page, &mut cx), vec!["a", "a"]);
    // With nothing open the page takes clicks again.
    click(&mut cx, "under");
    assert_eq!(log_of(&page, &mut cx), vec!["a", "a", "under"]);
}

#[gpui::test]
fn a_press_inside_the_popover_keeps_it_open(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "a");
    click(&mut cx, "archive");
    assert!(is_open(&mut cx, "a"), "a disabled row does nothing");
    click(&mut cx, "agent");
    assert!(!is_open(&mut cx, "a"), "choosing a row closes the menu");
    assert_eq!(log_of(&page, &mut cx), vec!["a", "agent"]);
}

#[gpui::test]
fn escape_closes_only_the_top_layer(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "open-dlg");
    assert!(is_open(&mut cx, "dlg"));
    click(&mut cx, "in-dlg");
    assert!(is_open(&mut cx, "in-dlg"), "a popover over the dialog");
    assert_eq!(cx.update(|window, cx| overlays(window, cx).open_count(cx)), 2);
    cx.simulate_keystrokes("escape");
    assert!(!is_open(&mut cx, "in-dlg"));
    assert!(is_open(&mut cx, "dlg"), "the dialog under it stays");
    cx.simulate_keystrokes("ctrl-enter");
    assert_eq!(log_of(&page, &mut cx), vec!["confirm"], "Ctrl-Enter is the dialog's primary action");
    cx.simulate_keystrokes("escape");
    assert!(!is_open(&mut cx, "dlg"));
}

#[gpui::test]
fn escape_closes_the_popover_even_when_focus_is_elsewhere(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "a");
    let outside = cx.update(|_, cx| page.read(cx).outside.clone());
    cx.update(|window, _| window.focus(&outside));
    cx.simulate_keystrokes("escape");
    assert!(!is_open(&mut cx, "a"));
}

#[gpui::test]
fn focus_returns_to_the_trigger(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    let custom = cx.update(|_, cx| page.read(cx).custom.clone());
    click(&mut cx, "custom");
    assert!(!cx.update(|window, _| custom.is_focused(window)), "focus moved into the popover");
    cx.simulate_keystrokes("escape");
    assert!(cx.update(|window, _| custom.is_focused(window)), "and came back");

    // A kit button: Escape gives focus back, so Enter presses it again.
    click(&mut cx, "a");
    cx.simulate_keystrokes("escape");
    assert!(!is_open(&mut cx, "a"));
    key_up(&mut cx, "enter");
    assert!(is_open(&mut cx, "a"), "Enter on the refocused trigger reopened the menu");
    assert_eq!(log_of(&page, &mut cx), vec!["a", "a"]);
}

#[gpui::test]
fn a_dialog_keeps_tab_inside(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    let (d1, d2, outside) = cx.update(|_, cx| {
        let p = page.read(cx);
        (p.d1.clone(), p.d2.clone(), p.outside.clone())
    });
    click(&mut cx, "open-dlg");
    let focused = |cx: &mut VisualTestContext, h: &FocusHandle| cx.update(|window, _| h.is_focused(window));
    cx.simulate_keystrokes("tab");
    assert!(focused(&mut cx, &d1), "Tab goes to the dialog's first control");
    cx.simulate_keystrokes("tab");
    assert!(focused(&mut cx, &d2));
    let mut seen_d1_again = false;
    for _ in 0..4 {
        cx.simulate_keystrokes("tab");
        assert!(!focused(&mut cx, &outside), "Tab never leaves the dialog");
        seen_d1_again |= focused(&mut cx, &d1);
    }
    assert!(seen_d1_again, "Tab wraps round to the first control");
    // Shift-Tab from the first control wraps to the last one.
    cx.update(|window, _| window.focus(&d1));
    cx.simulate_keystrokes("shift-tab");
    assert!(!focused(&mut cx, &outside) && !focused(&mut cx, &d1));
    cx.simulate_keystrokes("shift-tab");
    cx.simulate_keystrokes("shift-tab");
    assert!(focused(&mut cx, &d1), "Shift-Tab cycles back: {:?}", cx.update(|w, cx| w.focused(cx)));
}

#[gpui::test]
fn a_press_on_the_scrim_closes_the_dialog(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "open-dlg");
    cx.simulate_click(point(px(10.), px(700.)), Modifiers::none());
    cx.run_until_parked();
    assert!(!is_open(&mut cx, "dlg"));
    assert!(log_of(&page, &mut cx).is_empty());
}

#[gpui::test]
fn menu_keys_move_wrap_skip_disabled_rows_and_choose(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    // Rows: Agent, Archive (coming soon), Orchestrator, Pipeline.
    let cases: [(&str, &str); 6] = [
        ("down down enter", "orchestrator"),          // skips the disabled row
        ("down down down down enter", "agent"),       // wraps past the end
        ("up enter", "pipeline"),                     // from nothing, up is the last row
        ("down up enter", "pipeline"),                // wraps past the start
        ("end home enter", "agent"),
        ("o enter", "orchestrator"),                  // a letter jumps
    ];
    for (keys, chosen) in cases {
        click(&mut cx, "a");
        assert!(is_open(&mut cx, "a"));
        cx.simulate_keystrokes(keys);
        assert!(!is_open(&mut cx, "a"), "Enter chose a row and closed the menu ({keys})");
        let log = log_of(&page, &mut cx);
        assert_eq!(log.last().map(String::as_str), Some(chosen), "{keys}");
    }
    // A letter that matches only the disabled row selects nothing; Escape closes, choosing nothing.
    click(&mut cx, "a");
    let before = log_of(&page, &mut cx).len();
    cx.simulate_keystrokes("x enter escape");
    assert!(!is_open(&mut cx, "a"));
    assert_eq!(log_of(&page, &mut cx).len(), before);
}

#[gpui::test]
fn a_toast_goes_after_four_seconds_unless_hovered(cx: &mut TestAppContext) {
    let (_, mut cx) = setup(cx);
    let has = |cx: &mut VisualTestContext| cx.update(|window, cx| overlays(window, cx).has_toast(cx));
    cx.update(|window, cx| overlays(window, cx).toast(Toast::new("Release captain archived"), cx));
    cx.run_until_parked();
    assert!(has(&mut cx));
    cx.executor().advance_clock(Duration::from_millis(3900));
    cx.run_until_parked();
    assert!(has(&mut cx), "still up before 4s");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(!has(&mut cx), "gone after 4s");

    // Hovered, it waits; when the pointer leaves it finishes its time.
    cx.update(|window, cx| overlays(window, cx).toast(Toast::new("Saved"), cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    let toast = cx.debug_bounds("toast").expect("the toast is drawn");
    cx.simulate_mouse_move(toast.center(), None, Modifiers::none());
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert!(has(&mut cx), "paused while hovered");
    cx.simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::none());
    cx.executor().advance_clock(Duration::from_millis(2900));
    cx.run_until_parked();
    assert!(has(&mut cx), "resumes with what was left");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(!has(&mut cx));
}

#[gpui::test]
fn a_new_toast_replaces_the_old_one_and_undo_fires(cx: &mut TestAppContext) {
    let (_, mut cx) = setup(cx);
    let log: Log = Rc::default();
    let toast = |name: &'static str, log: &Log| {
        let log = log.clone();
        Toast::new(name).detail("2 pipelines have no owner now").undo(move |_, _| push(&log, name))
    };
    cx.update(|window, cx| overlays(window, cx).toast(toast("first", &log), cx));
    cx.update(|window, cx| overlays(window, cx).toast(toast("second", &log), cx));
    cx.run_until_parked();
    click(&mut cx, "toast-undo");
    assert_eq!(*log.borrow(), vec!["second"], "Undo is the current toast's");
    assert!(!cx.update(|window, cx| overlays(window, cx).has_toast(cx)));

    cx.update(|window, cx| overlays(window, cx).toast(toast("third", &log), cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(*log.borrow(), vec!["second", "third"], "Ctrl-Z is Undo while the toast is up");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(log.borrow().len(), 2, "once");

    cx.update(|window, cx| overlays(window, cx).toast(toast("fourth", &log), cx));
    cx.run_until_parked();
    click(&mut cx, "toast-dismiss");
    assert!(!cx.update(|window, cx| overlays(window, cx).has_toast(cx)));
    assert_eq!(log.borrow().len(), 2, "dismiss is not undo");
}

#[gpui::test]
fn without_a_host_overlay_calls_do_nothing(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Blank);
    cx.update(|window, cx| {
        let o = overlays(window, cx);
        assert!(!o.is_mounted(cx));
        assert!(o.open_dialog("x", |_, _| div().into_any_element(), window, cx).is_none());
        assert!(!o.is_open("x", cx));
    });
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
