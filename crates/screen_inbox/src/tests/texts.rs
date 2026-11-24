//! Content text in the detail can be selected with the mouse and copied, the queue's keys
//! keep working around it, and in a narrow detail pane long text stays inside its box.

use gpui::{
    Bounds, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point, px,
    size,
};
use hub::InboxFocus;
use hub::api::{InboxId, InboxKind, Link, Outcome, RunId};
use hub::fixtures::World;
use ui::prelude::*;

use super::{NOW, focus, keys, leak, open_with, rows};

/// The owner's failure: one long line with paths and a hash.
const ERROR: &str = "Merge mockup failed: `main` is checked out in /home/user/Pictures/designs/pipeliner-mockups with changes the merge would overwrite; nothing was merged (`git reset --keep --quiet 937a0a362fc0fea6262662a4c74670004693c29e` failed (exit 128): fatal: Cannot do a keep reset in the middle of a merge.)";

/// A failed merge of run-1431, its error as the summary.
fn failure(world: &mut World, title: &str, summary: &str, links: Vec<Link>) {
    let mut item = world.inbox[0].clone();
    item.id = InboxId::new("inbox-fail");
    item.kind = InboxKind::Failure;
    item.outcome = Some(Outcome::Failed);
    item.title = title.to_string();
    item.summary = summary.to_string();
    item.run = Some(RunId::new("run-1431"));
    item.links = links;
    item.created = NOW;
    world.inbox.push(item);
}

fn item(id: &str) -> Option<InboxFocus> {
    Some(InboxFocus::Item(InboxId::new(id)))
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector))
        .unwrap_or_else(|| panic!("`{selector}` was not laid out"))
}

/// The advance of one character at `style`'s size (the test text system is monospaced).
fn char_width(cx: &mut VisualTestContext, style: TypeStyle) -> f32 {
    cx.update(|window, _| {
        let run = window.text_style().to_run(1);
        let line = window
            .text_system()
            .shape_line("m".into(), px(style.size), &[run], None);
        f32::from(line.width)
    })
}

/// The point just right of the start of character `offset` of the first line of a text whose
/// first line starts at `origin`.
fn at(origin: Point<Pixels>, width: f32, style: TypeStyle, offset: usize) -> Point<Pixels> {
    point(
        origin.x + px(width * offset as f32 + 1.),
        origin.y + px(style.line / 2.),
    )
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

fn close(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() < 0.5
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let fits = |a: Pixels, b: Pixels| f32::from(a) <= f32::from(b) + 0.5;
    fits(outer.left(), inner.left())
        && fits(outer.top(), inner.top())
        && fits(inner.right(), outer.right())
        && fits(inner.bottom(), outer.bottom())
}

#[gpui::test]
async fn a_dragged_part_of_the_failure_is_copied(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-fail"), |w| {
        failure(w, "Merge failed", ERROR, Vec::new())
    });
    let text = bounds(cx, "item-error-text");
    // The first line sits level with the button: it starts half the difference down.
    let origin = text.origin + point(px(0.), px((CONTROL_HEIGHT_SM - BODY.line) / 2.));
    let width = char_width(cx, BODY);
    let (from, to) = ("Merge ".len(), "Merge mockup failed".len());
    drag(
        cx,
        at(origin, width, BODY, from),
        at(origin, width, BODY, to),
    );
    let selected = t.view.read_with(cx, |v, cx| {
        v.texts
            .view("error")
            .and_then(|md| md.read(cx).selected_text())
    });
    assert_eq!(selected.as_deref(), Some("mockup failed"));
    keys(cx, "ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("mockup failed"));
    assert_eq!(focus(&t, cx), item("inbox-fail"), "copying moved nothing");

    // The queue's keys still work with the text holding the selection.
    let list = rows(&t, cx);
    let at_row = list.iter().position(|id| id == "inbox-fail");
    let (key, next) = match at_row {
        Some(ix) if ix + 1 < list.len() => ("j", list[ix + 1].clone()),
        Some(ix) if ix > 0 => ("k", list[ix - 1].clone()),
        _ => panic!("the failure is not listed alone: {list:?}"),
    };
    keys(cx, key);
    assert_eq!(focus(&t, cx), item(&next));
    // The keys stay with the queue after the text went with its item.
    keys(cx, if key == "j" { "k" } else { "j" });
    assert_eq!(focus(&t, cx), item("inbox-fail"));
}

#[gpui::test]
async fn a_dragged_part_of_a_question_is_copied(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-1435"), |_| {});
    let text = bounds(cx, "item-summary");
    let width = char_width(cx, BODY);
    drag(
        cx,
        at(text.origin, width, BODY, 0),
        at(text.origin, width, BODY, "Which log".len()),
    );
    keys(cx, "ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("Which log"));
    assert_eq!(focus(&t, cx), item("inbox-1435"));
}

#[gpui::test]
async fn the_title_can_be_copied(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, item("inbox-1435"), |_| {});
    let text = bounds(cx, "item-title");
    let width = char_width(cx, TITLE_LG);
    drag(
        cx,
        at(text.origin, width, TITLE_LG, "Which ".len()),
        at(text.origin, width, TITLE_LG, "Which log sinks".len()),
    );
    keys(cx, "ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("log sinks"));
}

#[gpui::test]
async fn long_text_stays_inside_a_narrow_detail(cx: &mut TestAppContext) {
    let path = format!("/home/user/{}", "deep/".repeat(38));
    let hash = "937a0a362fc0fea6262662a4c74670004693c29e".repeat(2);
    let summary = format!("{ERROR} {path} {hash}");
    let title = "x".repeat(160);
    let url = format!("https://github.com/acme/{}/pull/2280", "y".repeat(160));
    let links = vec![Link {
        label: "PR #2280".to_string(),
        url,
    }];
    let (_, cx) = open_with(cx, item("inbox-fail"), move |w| {
        failure(w, &title, &summary, links)
    });
    cx.simulate_resize(size(px(700.), px(1400.)));
    // Effects are flushed, and the window drawn at its new size, on the next update.
    cx.update(|_, _| {});
    // The detail pane is 400px: the queue takes 300.
    let content = 400. - 2. * SPACE_6;
    let banner = bounds(cx, "item-error");
    assert!(close(banner.size.width, px(content)), "{banner:?}");
    assert!(close(banner.left(), px(300. + SPACE_6)), "{banner:?}");

    // The text takes all of the room between the icon and the button.
    let icon = bounds(cx, "item-error-icon");
    let text = bounds(cx, "item-error-text");
    let button = bounds(cx, "item-open-run");
    assert!(
        close(text.left(), icon.right() + px(SPACE_3)),
        "{icon:?} {text:?}"
    );
    assert!(
        close(text.right() + px(SPACE_3), button.left()),
        "{text:?} {button:?}"
    );
    assert!(
        close(button.right(), banner.right() - px(SPACE_3_5 + 1.)),
        "{button:?} {banner:?}"
    );
    assert!(text.size.width > px(150.), "{text:?}");
    // It wraps: many lines, all inside the banner.
    assert!(text.size.height > px(BODY.line * 8.), "{text:?}");
    for part in [icon, text, button] {
        assert!(inside(part, banner), "{part:?} is outside {banner:?}");
    }

    let title = bounds(cx, "item-title");
    assert!(close(title.size.width, px(content)), "{title:?}");
    assert!(title.size.height > px(TITLE_LG.line * 2.), "{title:?}");

    let card = bounds(cx, "item-pull-request");
    assert!(close(card.size.width, px(content)), "{card:?}");
    for part in [
        "item-pull-request-title",
        "item-pull-request-branches",
        "item-pull-request-url",
    ] {
        let b = bounds(cx, part);
        assert!(inside(b, card), "{part} {b:?} is outside {card:?}");
    }
    let address = bounds(cx, "item-pull-request-url");
    assert!(address.size.height > px(MONO_SM.line * 2.), "{address:?}");
    let links = bounds(cx, "item-links");
    assert!(
        f32::from(links.right()) <= 700. - SPACE_6 + 0.5,
        "{links:?}"
    );
}

#[gpui::test]
async fn a_review_fits_a_narrow_detail(cx: &mut TestAppContext) {
    let (_, cx) = open_with(
        cx,
        Some(InboxFocus::Review(hub::api::ReviewId::new(super::REVIEW))),
        |_| {},
    );
    cx.simulate_resize(size(px(700.), px(1400.)));
    cx.update(|_, _| {});
    let content = 400. - 2. * SPACE_6;
    let title = bounds(cx, "review-title");
    assert!(close(title.size.width, px(content)), "{title:?}");
    let table = bounds(cx, "review-table");
    let approach = bounds(cx, "review-approach");
    assert!(inside(approach, table), "{approach:?} is outside {table:?}");
    let trail = bounds(cx, "review-provenance");
    assert!(
        f32::from(trail.right()) <= 700. - SPACE_6 + 0.5,
        "{trail:?}"
    );
}
