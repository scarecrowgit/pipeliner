//! What the finished screens asked of the kit: probes that pass through rows, tabs and
//! options; sortable headers; hover actions; buttons that leave focus alone; a caller-owned
//! view as a dialog layer; and the new builders rendering at all.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AnyElement, Context, FocusHandle, Modifiers, Render, TestAppContext, VisualTestContext, Window,
    div, prelude::*, px,
};

use crate::overlay::current_layer;
use crate::prelude::*;
use crate::{OverlayHost, keys};

/// A view that draws whatever `build` makes.
struct Stage {
    build: Box<dyn Fn() -> AnyElement>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        root().flex().flex_col().child(div().p_4().w(px(640.)).child((self.build)()))
    }
}

fn stage(cx: &mut TestAppContext, build: impl Fn() -> AnyElement + 'static) -> &mut VisualTestContext {
    let (_, cx) = cx.add_window_view(|_, _| Stage { build: Box::new(build) });
    cx.run_until_parked();
    cx
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn rows_tabs_and_options_carry_their_own_probes(cx: &mut TestAppContext) {
    crate::dev::enable_for_tests();
    let cx = stage(cx, || {
        col()
            .child(
                Menu::new("pm")
                    .autofocus(false)
                    .item(MenuItem::new("pm-agent", "Agent").probe_name("menu-agent"))
                    .item(MenuItem::new("pm-archive", "Archive").disabled(true).tooltip("Nothing to archive").probe_name("menu-archive"))
                    .item(MenuItem::new("pm-plain", "Plain")),
            )
            .child(MenuItem::new("alone", "Alone").probe_name("menu-alone"))
            .child(Tabs::new("pt").tab("Waiting").tab_probe("tab-waiting").tab_n("All", "3").tab_icon(IconName::Inbox).tab_probe("tab-all"))
            .child(Segmented::new("ps").option("Read").option_probe("seg-read").icon_option(IconName::Edit, "Write").option_probe("seg-write"))
            .child(OptionGroup::new("po").option(OptionCard::new("po-a", "Worktree").probe_name("opt-worktree")))
            .into_any_element()
    });
    let names = [
        "menu-agent", "menu-archive", "pm-plain", "menu-alone", "tab-waiting", "tab-all", "seg-read", "seg-write",
        "opt-worktree",
    ];
    for name in names {
        assert!(crate::dev::probe_drawn(name), "no probe `{name}`");
    }
    assert!(!crate::dev::probe_drawn("pm-agent"), "a named row is probed by its name only");
    // The row ids still work as selectors.
    assert!(cx.debug_bounds("pm-archive").is_some());
}

#[test]
fn a_header_click_flips_the_sorted_column_and_starts_others_ascending() {
    use SortDirection::*;
    assert_eq!(next_sort(None, 2), (2, Ascending));
    assert_eq!(next_sort(Some((2, Ascending)), 2), (2, Descending));
    assert_eq!(next_sort(Some((2, Descending)), 2), (2, Ascending));
    assert_eq!(next_sort(Some((2, Descending)), 0), (0, Ascending));
}

#[gpui::test]
fn a_sortable_header_reports_the_sort_it_asks_for(cx: &mut TestAppContext) {
    let got: Rc<RefCell<Vec<(usize, SortDirection)>>> = Rc::default();
    let g = got.clone();
    let cx = stage(cx, move || {
        let g = g.clone();
        Table::new("tbl")
            .column(Column::new("Key").fixed(104.).sortable())
            .column(Column::new("Title"))
            .column(Column::new("Updated").fixed(84.).right().sortable())
            .sorted(0, SortDirection::Ascending)
            .on_sort(move |c, d, _, _| g.borrow_mut().push((c, d)))
            .row(TableRow::new("r1").mono("PAY-1").text("Retry").text("2m"))
            .into_any_element()
    });
    assert!(cx.debug_bounds("tbl-head-1").is_none(), "a plain header is not a button");
    click(cx, "tbl-head-0");
    click(cx, "tbl-head-2");
    assert_eq!(got.borrow().as_slice(), &[(0, SortDirection::Descending), (2, SortDirection::Ascending)]);
}

#[gpui::test]
fn hover_actions_replace_the_meta_and_keep_their_clicks(cx: &mut TestAppContext) {
    let (pins, opens) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let (p, o) = (pins.clone(), opens.clone());
    let cx = stage(cx, move || {
        let (p, o) = (p.clone(), o.clone());
        ListItem::new("chat", "Fix PAY-1423")
            .dot(Status::Live)
            .meta("12m")
            .hover_actions(IconButton::new("chat-pin", IconName::Pin).small().on_click(move |_, _, _| p.set(p.get() + 1)))
            .on_click(move |_, _, _| o.set(o.get() + 1))
            .into_any_element()
    });
    assert!(cx.debug_bounds("chat-pin").is_none(), "hidden until hovered");
    let row = cx.debug_bounds("chat").expect("the row");
    cx.simulate_mouse_move(row.center(), None, Modifiers::none());
    cx.run_until_parked();
    click(cx, "chat-pin");
    assert_eq!((pins.get(), opens.get()), (1, 0), "the pin fired, the row did not open");
    click(cx, "chat");
    assert_eq!(opens.get(), 1);
}

/// A field that holds focus next to buttons that do or do not take it.
struct FocusPage {
    field: FocusHandle,
    clicks: Rc<Cell<usize>>,
}

impl Render for FocusPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let tap = |c: &Rc<Cell<usize>>| {
            let c = c.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, _: &mut gpui::App| c.set(c.get() + 1)
        };
        root()
            .flex()
            .gap_4()
            .child(div().id("field").size(px(40.)).track_focus(&self.field.clone().tab_index(0).tab_stop(true)))
            .child(Button::new("keep", "Undo").icon(IconName::Undo).focus_on_click(false).on_click(tap(&self.clicks)))
            .child(IconButton::new("keep-icon", IconName::Redo).focus_on_click(false).on_click(tap(&self.clicks)))
            .child(Button::new("take", "New").trailing_icon(IconName::ChevronDown).on_click(tap(&self.clicks)))
    }
}

#[gpui::test]
fn a_button_that_keeps_focus_fires_without_taking_it(cx: &mut TestAppContext) {
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let (page, cx) = cx.add_window_view(move |_, cx| FocusPage { field: cx.focus_handle(), clicks: c });
    let field = page.read_with(cx, |p, _| p.field.clone());
    cx.update(|window, _| window.focus(&field));
    let on_field = |cx: &mut VisualTestContext| cx.update(|window, _| field.is_focused(window));
    click(cx, "keep");
    click(cx, "keep-icon");
    assert_eq!(clicks.get(), 2);
    assert!(on_field(cx), "focus stayed on the field");
    click(cx, "take");
    assert_eq!(clicks.get(), 3);
    assert!(!on_field(cx), "an ordinary button takes focus");
}

/// A view that renders a [`Dialog`], opened as a layer by [`Overlays::open_dialog_view`].
struct DialogView {
    saw_layer: Rc<Cell<bool>>,
}

impl Render for DialogView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.saw_layer.set(current_layer().is_some());
        Dialog::new("vdlg", "Search chats").width(400.).child(div().size(px(20.))).cancel_button()
    }
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

#[gpui::test]
fn a_dialog_view_closes_through_its_own_buttons_and_reports_it(cx: &mut TestAppContext) {
    cx.update(keys::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|_| Blank);
        OverlayHost::new(window, cx).with_content(page)
    });
    let saw_layer = Rc::new(Cell::new(false));
    let closed = Rc::new(Cell::new(0));
    let open = |cx: &mut VisualTestContext| {
        let (s, c) = (saw_layer.clone(), closed.clone());
        cx.update(|window, cx| {
            let view = cx.new(|_| DialogView { saw_layer: s });
            overlays(window, cx).open_dialog_view("vdlg", view, move |_, _| c.set(c.get() + 1), window, cx);
        });
        cx.run_until_parked();
    };
    let is_open = |cx: &mut VisualTestContext| cx.update(|window, cx| overlays(window, cx).is_open("vdlg", cx));

    open(cx);
    assert!(is_open(cx));
    assert!(saw_layer.get(), "the view rendered inside its layer");
    click(cx, "vdlg-close");
    assert!(!is_open(cx), "the close button closed the layer");
    assert_eq!(closed.get(), 1);

    open(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!is_open(cx));
    assert_eq!(closed.get(), 2, "Escape reports the close too");
}

#[gpui::test]
fn the_new_builders_render(cx: &mut TestAppContext) {
    use gpui::{FontWeight, HighlightStyle};
    assert_eq!(HintTone::Info.color(), INFO);
    assert_eq!(HintTone::default().color(), TEXT_MUTED);
    let bold = HighlightStyle { font_weight: Some(FontWeight::BOLD), color: Some(ACCENT_TEXT), ..Default::default() };
    let cx = stage(cx, move || {
        col()
            .gap_2()
            .child(MenuItem::new("mi", "Reproducer").matched(2).hint("running").hint_tone(HintTone::Info))
            // 2 falls inside "é": the mark is dropped, the row still draws.
            .child(MenuItem::new("mi-bad", "Résumé").matched(2))
            .child(MenuItem::new("mi-suffix", "Fixer").label_suffix("opus-5.5").matched(1).hint("idle"))
            .child(div().w(px(80.)).flex().font_family(FONT_MONO).child(truncate("~/code/acme/payments-api\nsecond line")))
            .child(SelectBox::new("sel", "opus-5.5").mono().small())
            .child(SelectBox::new("sel-bad", "gone-model").mono().danger(true))
            .child(div().w(px(120.)).flex().child(truncate_middle("~/code/acme/payments-api/fix-PAY-1423", BODY.line)))
            .child(div().w(px(120.)).flex().child(truncate_styled("retry the charge", [(0..5, bold), (3..9, bold)], BODY.line)))
            .into_any_element()
    });
    for selector in ["mi", "mi-bad", "mi-suffix", "sel", "sel-bad"] {
        assert!(cx.debug_bounds(selector).is_some(), "`{selector}` was not laid out");
    }
}
