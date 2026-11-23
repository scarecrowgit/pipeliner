//! Keyboard use of the focusable components: Tab reaches each of them in order, Enter and
//! Space press them, arrows move inside groups, the stepper takes Up, Down and digits.

use gpui::{App, Context, Entity, Render, TestAppContext, VisualTestContext, WeakEntity, Window, prelude::*};

use super::overlays::{Log, click, key_up};
use crate::OverlayHost;
use crate::keys;
use crate::prelude::*;

#[derive(Default)]
struct Controls {
    log: Log,
    toggle: bool,
    check: bool,
    tab: usize,
    seg: usize,
    radio: usize,
    option: usize,
    steps: i32,
}

fn note(log: &Log, s: impl Into<String>) {
    log.borrow_mut().push(s.into());
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me: WeakEntity<Self> = cx.entity().downgrade();
        let set = move |f: fn(&mut Controls, usize)| {
            let me = me.clone();
            move |ix: usize, _: &mut Window, cx: &mut App| {
                me.update(cx, |this, cx| {
                    f(this, ix);
                    cx.notify();
                })
                .ok();
            }
        };
        let log = self.log.clone();
        let l = |s: &'static str| {
            let log = log.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, _: &mut App| note(&log, s)
        };
        let (lt, lk) = (log.clone(), log.clone());
        let me2 = cx.entity().downgrade();
        col()
            .p_4()
            .gap_2()
            .child(Button::new("btn", "Deploy").on_click(l("button")))
            .child(IconButton::new("ibtn", IconName::More).tooltip("More").on_click(l("icon button")))
            .child(Chip::new("chip", "Needs you").on_click(l("chip")))
            .child(Toggle::new("toggle", self.toggle).on_change(move |v, _, _| note(&lt, format!("toggle {v}"))))
            .child(Checkbox::new("check", self.check).label("Run tests").on_change(move |v, _, _| note(&lk, format!("check {v}"))))
            .child(Radio::new("radio", false).label("Manual").on_select(l("radio")))
            .child(SelectBox::new("select", "Sonnet").on_click(l("select")))
            .child(Button::new("off", "Disabled").disabled(true).on_click(l("disabled")))
            .child(Tabs::new("tabs").tab("Flow").tab("Runs").tab("Versions").selected(self.tab).on_select(set(|c, i| c.tab = i)))
            .child(Segmented::new("seg").option("Day").option("Week").selected(self.seg).on_select(set(|c, i| c.seg = i)))
            .child(RadioGroup::new("group").option("A").option("B").option("C").selected(self.radio).on_select(set(|c, i| c.radio = i)))
            .child(
                OptionGroup::new("opts")
                    .option(OptionCard::new("o1", "All new issues").description("37 queued"))
                    .option(OptionCard::new("o2", "1 in 10").description("for 24 hours"))
                    .selected(self.option)
                    .on_select(set(|c, i| c.option = i)),
            )
            .child(Stepper::new("steps", self.steps).range(1, 20).on_change(move |v, _, cx| {
                me2.update(cx, |this, cx| {
                    this.steps = v;
                    cx.notify();
                })
                .ok();
            }))
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Controls>, VisualTestContext) {
    cx.update(keys::init);
    let (host, vcx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|_| Controls { steps: 3, ..Default::default() });
        OverlayHost::new(window, cx).with_content(page)
    });
    let mut vcx = vcx.clone();
    let content = vcx.update(|_, cx| host.read(cx).content.clone()).expect("content");
    (content.downcast::<Controls>().unwrap_or_else(|_| panic!("controls")), vcx)
}

fn log(page: &Entity<Controls>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| std::mem::take(&mut *page.read(cx).log.borrow_mut()))
}

#[gpui::test]
fn tab_reaches_each_control_and_enter_or_space_presses_it(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    let expect = [
        ("enter", "button"),
        ("space", "icon button"),
        ("enter", "chip"),
        ("space", "toggle true"),
        ("space", "check true"),
        ("enter", "radio"),
        ("enter", "select"),
    ];
    for (key, what) in expect {
        cx.simulate_keystrokes("tab");
        key_up(&mut cx, key);
        assert_eq!(log(&page, &mut cx), vec![what.to_string()], "{key} after Tab");
    }
    // The disabled button is skipped: the next stop is the selected tab.
    cx.simulate_keystrokes("tab right");
    assert_eq!(cx.update(|_, cx| page.read(cx).tab), 1, "Tab skipped the disabled button");
    assert!(cx.update(|_, cx| crate::focus::focus_visible(cx)), "the keyboard shows the ring");
}

#[gpui::test]
fn arrows_move_inside_tabs_and_segmented_with_the_focus(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "tabs-0");
    let tab = |cx: &mut VisualTestContext| cx.update(|_, cx| page.read(cx).tab);
    cx.simulate_keystrokes("right");
    assert_eq!(tab(&mut cx), 1);
    cx.simulate_keystrokes("right right");
    assert_eq!(tab(&mut cx), 0, "wraps");
    cx.simulate_keystrokes("left");
    assert_eq!(tab(&mut cx), 2);
    cx.simulate_keystrokes("home");
    assert_eq!(tab(&mut cx), 0);
    cx.simulate_keystrokes("end");
    assert_eq!(tab(&mut cx), 2);
    // One tab stop per group: Tab leaves the tabs for the segmented control.
    cx.simulate_keystrokes("tab right");
    assert_eq!(cx.update(|_, cx| page.read(cx).seg), 1);
    assert_eq!(tab(&mut cx), 2, "the tabs did not move");
}

#[gpui::test]
fn arrows_move_inside_radio_and_option_groups(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    click(&mut cx, "group-0");
    cx.simulate_keystrokes("down");
    assert_eq!(cx.update(|_, cx| page.read(cx).radio), 1);
    cx.simulate_keystrokes("up up");
    assert_eq!(cx.update(|_, cx| page.read(cx).radio), 2, "wraps");
    click(&mut cx, "o1");
    cx.simulate_keystrokes("down");
    assert_eq!(cx.update(|_, cx| page.read(cx).option), 1);
    cx.simulate_keystrokes("down");
    assert_eq!(cx.update(|_, cx| page.read(cx).option), 0);
}

#[gpui::test]
fn a_stepper_takes_up_down_and_digits(cx: &mut TestAppContext) {
    let (page, mut cx) = setup(cx);
    let steps = |cx: &mut VisualTestContext| cx.update(|_, cx| page.read(cx).steps);
    click(&mut cx, "steps");
    cx.simulate_keystrokes("up");
    assert_eq!(steps(&mut cx), 4);
    cx.simulate_keystrokes("down down");
    assert_eq!(steps(&mut cx), 2);
    cx.simulate_keystrokes("7");
    assert_eq!(steps(&mut cx), 7);
    cx.simulate_keystrokes("1 2");
    assert_eq!(steps(&mut cx), 12, "two quick digits make a two-digit value");
    cx.simulate_keystrokes("9 9");
    assert_eq!(steps(&mut cx), 9, "99 is out of range, so the last digit counts");
    cx.simulate_keystrokes("0");
    assert_eq!(steps(&mut cx), 9, "0 is below the minimum: ignored");
}
