//! Each interactive component rendered in a test window, found by its selector (its id),
//! clicked, and checked for what its callback received. GPUI's test platform has no real text
//! shaping, so these test behaviour, not metrics.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{AnyElement, Context, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div, prelude::*};

use crate::prelude::*;

/// A view that draws whatever `build` makes, so a test can put one component in a window.
struct Probe {
    build: Box<dyn Fn() -> AnyElement>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        root().flex().flex_col().child(div().p_4().child((self.build)()))
    }
}

fn window(cx: &mut TestAppContext, build: impl Fn() -> AnyElement + 'static) -> &mut VisualTestContext {
    let (_, cx) = cx.add_window_view(|_, _| Probe { build: Box::new(build) });
    cx
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn a_button_fires_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let cx = window(cx, move || {
        let f = f.clone();
        Button::new("go", "Deploy v15").primary().kbd("⌘↵").on_click(move |_, _, _| f.set(f.get() + 1)).into_any_element()
    });
    click(cx, "go");
    assert_eq!(fired.get(), 1);
}

#[gpui::test]
fn a_disabled_button_does_not_fire(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let cx = window(cx, move || {
        let f = f.clone();
        Button::new("go", "Deploy v15").disabled(true).on_click(move |_, _, _| f.set(f.get() + 1)).into_any_element()
    });
    click(cx, "go");
    assert_eq!(fired.get(), 0);
}

#[gpui::test]
fn an_icon_button_fires_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    let cx = window(cx, move || {
        let f = f.clone();
        IconButton::new("more", IconName::More).bordered().tooltip("More").on_click(move |_, _, _| f.set(true)).into_any_element()
    });
    click(cx, "more");
    assert!(fired.get());
}

#[gpui::test]
fn a_toggle_asks_for_the_opposite_value(cx: &mut TestAppContext) {
    for initial in [false, true] {
        let got = Rc::new(Cell::new(None));
        let g = got.clone();
        let cx = window(cx, move || {
            let g = g.clone();
            Toggle::new("auto", initial).on_change(move |v, _, _| g.set(Some(v))).into_any_element()
        });
        click(cx, "auto");
        assert_eq!(got.get(), Some(!initial));
    }
}

#[gpui::test]
fn a_disabled_toggle_does_not_change(cx: &mut TestAppContext) {
    let got = Rc::new(Cell::new(None));
    let g = got.clone();
    let cx = window(cx, move || {
        let g = g.clone();
        Toggle::new("auto", false).disabled(true).on_change(move |v, _, _| g.set(Some(v))).into_any_element()
    });
    click(cx, "auto");
    assert_eq!(got.get(), None);
}

#[gpui::test]
fn a_checkbox_asks_for_the_opposite_value_from_its_label_too(cx: &mut TestAppContext) {
    let got = Rc::new(Cell::new(None));
    let g = got.clone();
    let cx = window(cx, move || {
        let g = g.clone();
        Checkbox::new("tests", false).label("Run tests").on_change(move |v, _, _| g.set(Some(v))).into_any_element()
    });
    click(cx, "tests");
    assert_eq!(got.get(), Some(true));
}

#[gpui::test]
fn a_radio_fires_only_while_unselected(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(0));
    let (a, b) = (fired.clone(), fired.clone());
    let cx = window(cx, move || {
        let (a, b) = (a.clone(), b.clone());
        row()
            .gap_4()
            .child(Radio::new("off", false).label("Manual").on_select(move |_, _, _| a.set(a.get() + 1)))
            .child(Radio::new("on", true).label("Auto").on_select(move |_, _, _| b.set(b.get() + 10)))
            .into_any_element()
    });
    click(cx, "on");
    assert_eq!(fired.get(), 0, "the selected radio is inert");
    click(cx, "off");
    assert_eq!(fired.get(), 1);
}

#[gpui::test]
fn a_stepper_steps_and_stops_at_its_bounds(cx: &mut TestAppContext) {
    let got = Rc::new(RefCell::new(Vec::new()));
    for (value, press, expect) in [(3, "steps-inc", Some(4)), (3, "steps-dec", Some(2)), (1, "steps-dec", None), (9, "steps-inc", None)] {
        got.borrow_mut().clear();
        let g = got.clone();
        let cx = window(cx, move || {
            let g = g.clone();
            Stepper::new("steps", value).range(1, 9).on_change(move |v, _, _| g.borrow_mut().push(v)).into_any_element()
        });
        click(cx, press);
        assert_eq!(got.borrow().first().copied(), expect, "value {value}, pressing {press}");
    }
}

#[gpui::test]
fn tabs_report_the_clicked_index(cx: &mut TestAppContext) {
    let got = Rc::new(Cell::new(None));
    let g = got.clone();
    let cx = window(cx, move || {
        let g = g.clone();
        Tabs::new("views").tab("Flow").tab_n("Runs", "12").tab("Settings").selected(0).on_select(move |ix, _, _| g.set(Some(ix))).into_any_element()
    });
    click(cx, "views-1");
    assert_eq!(got.get(), Some(1));
    click(cx, "views-2");
    assert_eq!(got.get(), Some(2));
}

#[gpui::test]
fn segmented_reports_the_clicked_index(cx: &mut TestAppContext) {
    let got = Rc::new(Cell::new(None));
    let g = got.clone();
    let cx = window(cx, move || {
        let g = g.clone();
        Segmented::new("range").option("Day").option("Week").option("Month").selected(1).small().on_select(move |ix, _, _| g.set(Some(ix))).into_any_element()
    });
    click(cx, "range-2");
    assert_eq!(got.get(), Some(2));
}

#[gpui::test]
fn a_chip_fires_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let cx = window(cx, move || {
        let f = f.clone();
        Chip::new("needs-you", "Needs you").count("4").pressed(true).on_click(move |_, _, _| f.set(f.get() + 1)).into_any_element()
    });
    click(cx, "needs-you");
    assert_eq!(fired.get(), 1);
}

#[gpui::test]
fn a_menu_item_fires_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let cx = window(cx, move || {
        let f = f.clone();
        col()
            .child(PopLabel::new("Project"))
            .child(MenuItem::new("switch", "Switch project").checked(true).hint("⌘P").on_click(move |_, _, _| f.set(f.get() + 1)))
            .child(PopSeparator)
            .child(MenuItem::new("delete", "Delete").danger())
            .into_any_element()
    });
    click(cx, "switch");
    assert_eq!(fired.get(), 1);
}

#[gpui::test]
fn nav_and_list_items_fire_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(RefCell::new(Vec::new()));
    let f = fired.clone();
    let cx = window(cx, move || {
        let (a, b) = (f.clone(), f.clone());
        col()
            .child(NavItem::new("inbox", IconName::Inbox, "Inbox").count("4").on_click(move |_, _, _| a.borrow_mut().push("inbox")))
            .child(ListItem::new("chat", "Refund webhook timeout").dot(Status::Running).meta("20m").on_click(move |_, _, _| b.borrow_mut().push("chat")))
            .into_any_element()
    });
    click(cx, "inbox");
    click(cx, "chat");
    assert_eq!(*fired.borrow(), vec!["inbox", "chat"]);
}

#[gpui::test]
fn a_table_row_fires_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let cx = window(cx, move || {
        let f = f.clone();
        Table::new("runs")
            .column(Column::new("Issue").fixed(120.))
            .column(Column::new("Title"))
            .column(Column::new("Cost").fixed(80.).right())
            .group("Today", Some("2".into()))
            .row(TableRow::new("r1").mono("PAY-1423").strong("Fix double charge").text("$0.21").selected(true))
            .row(TableRow::new("r2").mono("PAY-1424").text("Refund timeout").text("$0.09").on_click(move |_, _, _| f.set(f.get() + 1)))
            .into_any_element()
    });
    click(cx, "r2");
    assert_eq!(fired.get(), 1);
}

#[gpui::test]
fn a_select_box_and_link_fire_on_click(cx: &mut TestAppContext) {
    let fired = Rc::new(RefCell::new(Vec::new()));
    let f = fired.clone();
    let cx = window(cx, move || {
        let (a, b) = (f.clone(), f.clone());
        col()
            .child(SelectBox::new("model", "Sonnet").icon(IconName::Agent).on_click(move |_, _, _| a.borrow_mut().push("select")))
            .child(Link::new("change", "Change").on_click(move |_, _, _| b.borrow_mut().push("link")))
            .into_any_element()
    });
    click(cx, "model");
    click(cx, "change");
    assert_eq!(*fired.borrow(), vec!["select", "link"]);
}

#[gpui::test]
fn static_components_render_in_a_window(cx: &mut TestAppContext) {
    let cx = window(cx, || {
        col()
            .gap_2()
            .child(Badge::new("Live").tone(BadgeTone::Success).dot(Status::Live))
            .child(CountBadge::new("4"))
            .child(Tag::new("main").mono())
            .child(Kbd::new("⌘K"))
            .child(KeyHints::new().hint(["↑", "↓"], "move").hint(["↵"], "open"))
            .child(Progress::new(0.4).tone(ProgressTone::Info))
            .child(SpawnChip::new("Spawns Reproducer · 2 of 5"))
            .child(RuleChip::new("type", "=", "Bug").edited(true))
            .child(Avatar::new("AR"))
            .child(Tile::new(IconName::Agent).tone(TileTone::Spawn).large())
            .child(Code::new("git bisect"))
            .child(Banner::new().tone(BannerTone::Warning).child("Waiting for you"))
            .child(StatStrip::new().stat(Stat::new("Spend", "$3.81").suffix("of $10.00")).stat(Stat::new("Queued", "37")))
            .child(Card::new().header(CardHeader::new("Needs you").dot(Status::Waiting).count("4")).child("Body").footer(CardFooter::new().child("Open inbox")))
            .child(Section::new("Workdir").meta("Change").child(KeyValue::new().row("git", "2 uncommitted")))
            .child(Field::new("Name").child(InputFrame::new("name").placeholder("Name").icon(IconName::Search).kbd("⌘K").focused(true)))
            .child(Note::new("Saved"))
            .child(Divider::new())
            .into_any_element()
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("name").is_some());
}
