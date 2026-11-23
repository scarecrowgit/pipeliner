//! The view in a test window. The test platform shapes every character one fixed advance
//! wide, so positions come from the laid-out units rather than from guesses.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyView, Context, Entity, IntoElement, Modifiers, MouseButton, Render, TestAppContext,
    VisualTestContext, Window, div, prelude::*, px,
};

use super::SAMPLE;
use crate::markdown::element::unit_point;
use crate::markdown::{Markdown, bind_keys};

struct Host {
    view: AnyView,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().child(div().w(px(640.)).child(self.view.clone()))
    }
}

type Log = Rc<RefCell<Vec<String>>>;

/// A window showing `text`; logs clicked links and copied code.
fn open<'a>(
    cx: &'a mut TestAppContext,
    text: &'static str,
) -> (Entity<Markdown>, Log, Log, &'a mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        bind_keys(cx);
    });
    let (links, copies): (Log, Log) = Default::default();
    let (l, c) = (links.clone(), copies.clone());
    let slot: Rc<RefCell<Option<Entity<Markdown>>>> = Rc::default();
    let s = slot.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        let md = cx.new(|cx| {
            Markdown::new(text, cx)
                .mentions(["Fixer", "Codex"])
                .on_link(move |url, _, _| l.borrow_mut().push(url.to_string()))
                .on_copy_code(move |code, _, _| c.borrow_mut().push(code.to_string()))
        });
        *s.borrow_mut() = Some(md.clone());
        Host { view: md.into() }
    });
    cx.run_until_parked();
    let Some(md) = slot.borrow_mut().take() else {
        panic!("view not built")
    };
    (md, links, copies, cx)
}

fn point_of(
    md: &Entity<Markdown>,
    cx: &mut VisualTestContext,
    unit: usize,
    offset: usize,
) -> gpui::Point<gpui::Pixels> {
    md.read_with(cx, |m, _| unit_point(&m.shared.borrow(), unit, offset))
        .unwrap_or_else(|| panic!("unit {unit} offset {offset} not laid out"))
}

#[gpui::test]
fn every_unit_is_laid_out_once(cx: &mut TestAppContext) {
    let (md, _, _, cx) = open(cx, SAMPLE);
    md.read_with(cx, |m, _| {
        let frame: Vec<usize> = m.shared.borrow().frame.iter().map(|f| f.unit).collect();
        assert_eq!(frame, (0..m.units.len()).collect::<Vec<_>>());
        for (i, u) in m.units.iter().enumerate() {
            let code = crate::markdown::render::is_code(&m.units, i);
            assert_eq!(
                code,
                matches!(u.kind, crate::markdown::rich::UnitKind::Code { .. })
            );
        }
    });
}

#[gpui::test]
fn clicking_a_link_calls_on_link(cx: &mut TestAppContext) {
    let (md, links, _, cx) = open(cx, "Read [the docs](https://example.com/docs) first.");
    let at = point_of(&md, cx, 0, "Read t".len());
    cx.simulate_mouse_move(at, None, Modifiers::none());
    md.read_with(cx, |m, _| {
        assert_eq!(m.shared.borrow().hover_link, Some((0, 0)))
    });
    cx.simulate_click(at, Modifiers::none());
    assert_eq!(links.borrow().as_slice(), ["https://example.com/docs"]);
    // Plain text is not a link.
    let plain = point_of(&md, cx, 0, 1);
    cx.simulate_click(plain, Modifiers::none());
    assert_eq!(links.borrow().len(), 1);
}

#[gpui::test]
fn copy_button_copies_the_code(cx: &mut TestAppContext) {
    let (_, _, copies, cx) = open(cx, "Run:\n\n```sh\nbun test src/charges\n```\n");
    let Some(b) = cx.debug_bounds("md-copy-0") else {
        panic!("no copy button")
    };
    cx.simulate_click(b.center(), Modifiers::none());
    assert_eq!(copies.borrow().as_slice(), ["bun test src/charges"]);
    let clip = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("bun test src/charges"));
}

#[gpui::test]
fn drag_selects_across_blocks_and_copies(cx: &mut TestAppContext) {
    let (md, _, _, cx) = open(
        cx,
        "First paragraph here.\n\n- item one\n- item two\n\nLast words.",
    );
    let from = point_of(&md, cx, 0, "First ".len());
    let to = point_of(&md, cx, 3, "Last".len());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    let got = md.read_with(cx, |m, _| m.selected_text());
    assert_eq!(
        got.as_deref(),
        Some("paragraph here.\n\n- item one\n- item two\n\nLast")
    );
    cx.simulate_keystrokes("ctrl-c");
    let clip = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip, got);
    // A press on nothing clears it.
    cx.simulate_click(gpui::point(px(900.), px(900.)), Modifiers::none());
    assert_eq!(md.read_with(cx, |m, _| m.selected_text()), None);
}

#[gpui::test]
fn double_click_selects_a_word_and_select_all_everything(cx: &mut TestAppContext) {
    let (md, _, _, cx) = open(cx, "alpha beta gamma\n\nsecond");
    let at = point_of(&md, cx, 0, "alpha be".len());
    cx.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    assert_eq!(
        md.read_with(cx, |m, _| m.selected_text()).as_deref(),
        Some("beta")
    );
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(
        md.read_with(cx, |m, _| m.selected_text()).as_deref(),
        Some("alpha beta gamma\n\nsecond")
    );
}

#[gpui::test]
fn streaming_text_updates_the_view(cx: &mut TestAppContext) {
    let (md, _, _, cx) = open(cx, "Intro\n\n```rust\nfn main() {");
    md.update(cx, |m, cx| {
        m.set_text("Intro\n\n```rust\nfn main() {}\n```\n\nDone.", cx)
    });
    cx.run_until_parked();
    md.read_with(cx, |m, _| {
        assert_eq!(m.document().blocks.len(), 3);
        assert_eq!(m.shared.borrow().frame.len(), 3);
    });
}
