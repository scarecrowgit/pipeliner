//! Diff row model, intra-line emphasis, and the card in a test window.

use std::sync::Arc;

use gpui::{Context, IntoElement, Modifiers, Render, TestAppContext, Window, div, prelude::*, px};

use super::*;

mod selection;

fn line(kind: DiffLineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old_no: old,
        new_no: new,
        text: text.into(),
        no_newline: false,
    }
}

fn ctx(o: u32, n: u32, t: &str) -> DiffLine {
    line(DiffLineKind::Context, Some(o), Some(n), t)
}

fn add(n: u32, t: &str) -> DiffLine {
    line(DiffLineKind::Add, None, Some(n), t)
}

fn del(o: u32, t: &str) -> DiffLine {
    line(DiffLineKind::Del, Some(o), None, t)
}

/// The mockup's retry diff and a second hunk further down.
fn retry() -> DiffFile {
    let h1 = DiffHunk {
        lines: vec![
            ctx(
                44,
                44,
                "export async function retryCharge(charge: Charge) {",
            ),
            del(
                45,
                "  const res = await gateway.charge(charge.amount, charge.card);",
            ),
            add(
                45,
                "  const key = charge.idempotencyKey ?? deriveKey(charge);",
            ),
            add(
                46,
                "  const res = await gateway.charge(charge.amount, charge.card, { idempotencyKey: key });",
            ),
            ctx(46, 47, "  if (res.status === 'timeout') {"),
            del(47, "    return retryCharge(charge);"),
            add(
                48,
                "    return retryCharge({ ...charge, idempotencyKey: key });",
            ),
            ctx(48, 49, "  }"),
        ],
    };
    let mut last = add(91, "export { retryCharge };");
    last.no_newline = true;
    let h2 = DiffHunk {
        lines: vec![ctx(60, 61, "}"), last],
    };
    DiffFile {
        path: "src/charges/retry.ts".into(),
        added: 4,
        removed: 2,
        hunks: vec![h1, h2],
        ..Default::default()
    }
}

#[test]
fn gutter_shows_new_numbers_and_old_for_removed_lines() {
    let rows = diff_rows(&retry());
    let numbers: Vec<Option<u32>> = rows
        .iter()
        .filter_map(|r| match r {
            DiffRow::Line { number, .. } => Some(*number),
            _ => None,
        })
        .collect();
    assert_eq!(
        numbers[..8],
        [
            Some(44),
            Some(45),
            Some(45),
            Some(46),
            Some(47),
            Some(47),
            Some(48),
            Some(49)
        ]
    );
}

#[test]
fn separators_count_skipped_lines_and_no_newline_follows_its_line() {
    let rows = diff_rows(&retry());
    assert_eq!(rows.len(), 8 + 1 + 2 + 1);
    assert_eq!(rows[8], DiffRow::Separator { skipped: Some(11) });
    assert_eq!(rows.last(), Some(&DiffRow::NoNewline));
    // Unknown numbers give a separator without a count.
    let mut f = retry();
    f.hunks[1]
        .lines
        .iter_mut()
        .for_each(|l| (l.old_no, l.new_no) = (None, None));
    assert_eq!(diff_rows(&f)[8], DiffRow::Separator { skipped: None });
}

#[test]
fn empty_states() {
    let binary = DiffFile {
        binary: true,
        ..retry()
    };
    assert_eq!(binary.empty_state(), Some(DiffEmpty::Binary));
    assert!(diff_rows(&binary).is_empty());
    let deleted = DiffFile {
        deleted: true,
        hunks: Vec::new(),
        ..Default::default()
    };
    assert_eq!(
        deleted.empty_state().map(DiffEmpty::label),
        Some("File deleted")
    );
    let nothing = DiffFile {
        hunks: vec![DiffHunk::default()],
        ..Default::default()
    };
    assert_eq!(
        nothing.empty_state().map(DiffEmpty::label),
        Some("No changes")
    );
    assert_eq!(retry().empty_state(), None);
}

#[test]
fn display_text_expands_tabs_and_cuts_long_lines() {
    let f = DiffFile {
        hunks: vec![DiffHunk {
            lines: vec![add(1, "\tx\r"), add(2, &"é".repeat(MAX_LINE))],
        }],
        ..Default::default()
    };
    let rows = diff_rows(&f);
    let text = |i: usize| match &rows[i] {
        DiffRow::Line { text, .. } => text.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(text(0), "    x");
    assert!(text(1).len() <= MAX_LINE + '…'.len_utf8() && text(1).ends_with('…'));
}

#[test]
fn collapse_limits() {
    assert_eq!(visible_rows(30, Some(12), false), (12, true));
    assert_eq!(visible_rows(30, Some(12), true), (30, false));
    assert_eq!(visible_rows(10, Some(12), false), (10, false));
    assert_eq!(visible_rows(10, None, false), (10, false));
}

#[test]
fn emphasis_marks_the_changed_words() {
    let old = "    return retryCharge(charge);";
    let new = "    return retryCharge({ ...charge, idempotencyKey: key });";
    let Some((a, b)) = emphasis(old, new) else {
        panic!("no emphasis")
    };
    assert!(a.is_empty(), "nothing removed from the old line: {a:?}");
    let marked: Vec<&str> = b.iter().map(|r| &new[r.clone()]).collect();
    assert_eq!(marked, ["{ ...", ", idempotencyKey: key }"]);

    let (a, b) = emphasis("let x = foo(1);", "let x = bar(1);").unwrap_or_default();
    assert_eq!(
        (
            &"let x = foo(1);"[a[0].clone()],
            &"let x = bar(1);"[b[0].clone()]
        ),
        ("foo", "bar")
    );
}

#[test]
fn emphasis_skips_equal_different_and_long_lines() {
    assert_eq!(emphasis("same", "same"), None);
    assert_eq!(emphasis("alpha beta gamma", "one two three"), None);
    let long = "x ".repeat(MAX_EMPHASIS_LINE);
    assert_eq!(emphasis(&long, &format!("{long}y")), None);
}

#[test]
fn rows_pair_removed_and_added_lines() {
    let rows = diff_rows(&retry());
    let marks = |i: usize| match &rows[i] {
        DiffRow::Line { emphasis, .. } => emphasis.len(),
        _ => 0,
    };
    // Line 45 (removed) pairs with the first added line; 47 with 48.
    assert!(marks(6) > 0, "{:?}", rows[6]);
    assert_eq!(marks(0), 0);
}

struct Host {
    files: Vec<Arc<DiffFile>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let big = self.files[0].clone();
        ui::root().child(
            div()
                .w(px(720.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    DiffCard::new("edit", big)
                        .label("edit")
                        .compact()
                        .collapsible()
                        .collapse_after(12),
                )
                .children(
                    self.files[1..].iter().enumerate().map(|(i, f)| {
                        DiffCard::new(("file", i), f.clone()).pager(i, 3, |_, _, _| {})
                    }),
                ),
        )
    }
}

fn big_file(lines: u32) -> DiffFile {
    let hunk = DiffHunk {
        lines: (1..=lines)
            .map(|n| {
                if n % 3 == 0 {
                    add(n, &format!("let v{n} = compute({n});"))
                } else {
                    ctx(n, n, "    context();")
                }
            })
            .collect(),
    };
    DiffFile {
        path: "src/big.rs".into(),
        added: lines / 3,
        hunks: vec![hunk],
        ..Default::default()
    }
}

#[gpui::test]
fn cards_render_collapse_and_expand(cx: &mut TestAppContext) {
    ui::motion::set_reduced(true);
    cx.update(ui::init);
    let files = vec![
        Arc::new(big_file(5000)),
        Arc::new(DiffFile {
            binary: true,
            path: "logo.png".into(),
            ..Default::default()
        }),
        Arc::new(DiffFile {
            deleted: true,
            path: "old.rs".into(),
            removed: 3,
            ..Default::default()
        }),
    ];
    let (_, cx) = cx.add_window_view(move |_, _| Host { files });
    cx.run_until_parked();
    // Stale debug bounds outlive their element, so watch the next card move instead.
    let next_y = |cx: &mut gpui::VisualTestContext| {
        cx.run_until_parked();
        cx.debug_bounds("file-0-next")
            .map(|b| f32::from(b.origin.y))
            .unwrap_or(-1.)
    };
    let limited = next_y(cx);
    let Some(show) = cx.debug_bounds("edit-show-all") else {
        panic!("long diff is not limited")
    };
    cx.simulate_click(show.center(), Modifiers::none());
    let expanded = next_y(cx);
    assert!(
        expanded > limited + 100.,
        "expanded: {limited} -> {expanded}"
    );
    let Some(toggle) = cx.debug_bounds("edit-toggle") else {
        panic!("no toggle")
    };
    cx.simulate_click(toggle.center(), Modifiers::none());
    let collapsed = next_y(cx);
    assert!(collapsed < limited, "collapsed: {limited} -> {collapsed}");
}

/// A list row that may scroll away: the card's state lives here, not in the card.
struct HeldHost {
    file: Arc<DiffFile>,
    state: DiffCardState,
    shown: bool,
}

impl Render for HeldHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let card = self.shown.then(|| {
            let me = cx.entity();
            DiffCard::new("held", self.file.clone())
                .collapsible()
                .collapse_after(12)
                .state(self.state, move |next, _, cx| {
                    me.update(cx, |h, cx| {
                        h.state = next;
                        cx.notify();
                    })
                })
        });
        ui::root().child(div().w(px(720.)).flex().flex_col().children(card))
    }
}

#[gpui::test]
fn a_card_can_leave_its_fold_state_with_the_caller(cx: &mut TestAppContext) {
    ui::motion::set_reduced(true);
    cx.update(ui::init);
    let file = Arc::new(big_file(300));
    let (host, cx) = cx.add_window_view(move |_, _| HeldHost { file, state: DiffCardState::default(), shown: true });
    cx.run_until_parked();
    let click = |cx: &mut gpui::VisualTestContext, selector: &'static str| {
        let b = cx.debug_bounds(selector).unwrap_or_else(|| panic!("no `{selector}`"));
        cx.simulate_click(b.center(), Modifiers::none());
        cx.run_until_parked();
    };
    click(cx, "held-show-all");
    assert_eq!(host.read_with(cx, |h, _| h.state), DiffCardState { open: true, expanded: true });
    // The row scrolls out and back: the card is new, the state is the caller's.
    host.update(cx, |h, cx| {
        h.shown = false;
        cx.notify();
    });
    cx.run_until_parked();
    host.update(cx, |h, cx| {
        h.shown = true;
        cx.notify();
    });
    cx.run_until_parked();
    click(cx, "held-toggle");
    assert_eq!(host.read_with(cx, |h, _| h.state), DiffCardState { open: false, expanded: true });
}
