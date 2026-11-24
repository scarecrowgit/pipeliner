//! [`DiffView`]: the rows of one file's diff, drawn through `uniform_list` so a diff of
//! thousands of lines only builds the rows on screen.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, ClipboardItem, ElementId, FocusHandle, IntoElement,
    ListHorizontalSizingBehavior, RenderOnce, SharedString, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};
use ui::theme::*;

use super::model::{DiffFile, DiffLineKind, DiffRow, diff_rows_with_sources};
use super::select::{self, CodeLine, Notify, Shared};
use crate::markdown::{Copy, KEY_CONTEXT, SelectAll};

/// Height of a diff row (`MONO` line height).
pub const ROW_HEIGHT: f32 = 20.;
/// Width of the line-number column.
pub const GUTTER_WIDTH: f32 = 44.;
/// Width of the `+`/`−` column.
pub const SIGN_WIDTH: f32 = 16.;
/// Padding above and below the rows (`.diff { padding: 6px 0 }`).
const PAD_Y: f32 = 6.;
/// How tall a diff gets before it scrolls, unless told otherwise.
const DEFAULT_MAX_HEIGHT: f32 = 600.;

/// The rows of one file's diff: mono 12/20 on the sidebar ground, a right-aligned
/// line-number gutter that stays put while long lines scroll sideways, tinted added and
/// removed rows with the changed words emphasised, separators between hunks. A file with
/// nothing to draw shows "Binary file", "File deleted" or "No changes".
///
/// The code can be selected with the mouse (a drag across lines, a double click for a
/// word, a triple click for a line) and copied with Cmd/Ctrl+C, or selected whole with
/// Cmd/Ctrl+A: the Markdown bindings ([`crate::markdown::bind_keys`], part of
/// [`crate::bind_keys`]) serve both. A copy gives the code only, without numbers or
/// markers; see [`DiffView::new`]'s id for where the selection lives. A press anywhere
/// else in the window drops the selection, as in a [`Markdown`](crate::markdown::Markdown)
/// view.
///
/// ```ignore
/// DiffView::new("diff", file.clone()).max_height(480.)
/// DiffView::new("review-diff", file.clone()).fill() // takes the rest of a column
/// ```
#[derive(IntoElement)]
pub struct DiffView {
    id: ElementId,
    file: Arc<DiffFile>,
    limit: Option<usize>,
    max_height: f32,
    fill: bool,
}

impl DiffView {
    /// The diff of `file`. Pass the same `Arc` every frame: the rows are computed once per
    /// file and kept while it is shown. The rows, the scroll position and the selection are
    /// kept under `id` while the view is drawn in consecutive frames.
    pub fn new(id: impl Into<ElementId>, file: impl Into<Arc<DiffFile>>) -> Self {
        Self {
            id: id.into(),
            file: file.into(),
            limit: None,
            max_height: DEFAULT_MAX_HEIGHT,
            fill: false,
        }
    }

    /// Scroll inside past this height (default 600 px).
    pub fn max_height(mut self, px: f32) -> Self {
        self.max_height = px;
        self
    }

    /// Take the remaining height of a flex column and scroll inside it.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// Show only the first `rows` rows.
    pub fn limit(mut self, rows: Option<usize>) -> Self {
        self.limit = rows;
        self
    }
}

/// The file last shown, the scroll position, focus and the selection, kept across frames.
struct DiffState {
    file: Option<Arc<DiffFile>>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    shared: Rc<RefCell<Shared>>,
    /// The widest row among the first `.0` rows.
    longest: Option<(usize, Option<usize>)>,
}

impl RenderOnce for DiffView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if let Some(empty) = self.file.empty_state() {
            return div()
                .flex()
                .items_center()
                .w_full()
                .h(px(CARD_FOOT_HEIGHT))
                .px(px(SPACE_3))
                .bg(BG_SIDEBAR)
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(empty.label())
                .into_any_element();
        }
        let state = window.with_id(self.id.clone(), |window| {
            window.use_keyed_state("diff-state", cx, |_, cx| DiffState {
                file: None,
                scroll: UniformListScrollHandle::new(),
                focus: cx.focus_handle(),
                shared: Rc::default(),
                longest: None,
            })
        });
        let file = self.file.clone();
        let limit = self.limit;
        let (shared, scroll, focus, count, longest) = state.update(cx, |s, _| {
            let same = s
                .file
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &file) || **f == *file);
            if !same {
                let (rows, sources) = diff_rows_with_sources(&file);
                s.shared.borrow_mut().set_file(file.clone(), rows, sources);
                s.file = Some(file);
                s.longest = None;
            }
            let mut sh = s.shared.borrow_mut();
            let count = limit.map_or(sh.rows.len(), |l| l.min(sh.rows.len()));
            sh.shown = count;
            let longest = match s.longest {
                Some((n, l)) if n == count => l,
                _ => {
                    let l = widest(&sh.rows[..count]);
                    s.longest = Some((count, l));
                    l
                }
            };
            drop(sh);
            (
                s.shared.clone(),
                s.scroll.clone(),
                s.focus.clone(),
                count,
                longest,
            )
        });
        select::register(&self.id, &shared);
        let focused = focus.is_focused(window);
        let weak = state.downgrade();
        let notify: Notify = Rc::new(move |cx: &mut App| {
            if let Some(s) = weak.upgrade() {
                s.update(cx, |_, cx| cx.notify());
            }
        });
        let height = (count as f32 * ROW_HEIGHT + 2. * PAD_Y).min(self.max_height);
        let handle = scroll.clone();
        let rows_shared = shared.clone();
        let list = uniform_list(
            ElementId::NamedChild(Box::new(self.id.clone()), "rows".into()),
            count,
            move |range, _, _| {
                // Rows move with the horizontal scroll; the gutter moves back by as much.
                let x = (-f32::from(handle.0.borrow().base_handle.offset().x)).max(0.);
                let rows = rows_shared.borrow().rows.clone();
                range
                    .filter_map(|ix| rows.get(ix).map(|r| row(r, ix, x, &rows_shared, focused)))
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(scroll)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(longest)
        .w_full()
        .py(px(PAD_Y))
        .bg(BG_SIDEBAR)
        .type_style(MONO);
        let (copy_from, all_of) = (shared.clone(), shared.clone());
        let select_all_notify = notify.clone();
        let body = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .track_focus(&focus)
            .key_context(KEY_CONTEXT)
            .on_action(
                move |_: &Copy, _, cx| match copy_from.borrow().selected_text() {
                    Some(t) => cx.write_to_clipboard(ClipboardItem::new_string(t)),
                    None => cx.propagate(),
                },
            )
            .on_action(move |_: &SelectAll, _, cx| {
                all_of.borrow_mut().select_all();
                select_all_notify(cx);
            })
            // A sideways wheel moves the code (the list scrolls first); it must not also
            // scroll the page around the diff, which would take a sideways delta as a
            // vertical one.
            .on_scroll_wheel(|e, window, cx| {
                let d = e.delta.pixel_delta(window.line_height());
                if d.x.abs() > d.y.abs() {
                    cx.stop_propagation();
                }
            })
            .child(select::tracker(shared, focus, notify));
        if self.fill {
            body.flex_1()
                .min_h(px(0.))
                .child(list.flex_1().min_h(px(0.)))
                .into_any_element()
        } else {
            body.flex_none()
                .h(px(height))
                .child(list.flex_none().h(px(height)))
                .into_any_element()
        }
    }
}

/// The index of the widest line row.
fn widest(rows: &[DiffRow]) -> Option<usize> {
    rows.iter()
        .enumerate()
        .max_by_key(|(_, r)| match r {
            DiffRow::Line { text, .. } => text.len(),
            _ => 0,
        })
        .map(|(i, _)| i)
}

/// Drawn row `ix`; `gutter_x` keeps the gutter at the left edge while the row scrolls. A line
/// row's code is selectable.
fn row(
    r: &DiffRow,
    ix: usize,
    gutter_x: f32,
    shared: &Rc<RefCell<Shared>>,
    focused: bool,
) -> AnyElement {
    let (bg, fg, num_fg, number, sign, body): (_, _, _, Option<u32>, &str, AnyElement) = match r {
        DiffRow::Line {
            kind,
            number,
            text,
            emphasis,
        } => {
            let (bg, fg, sign, mark) = match kind {
                DiffLineKind::Add => (
                    DIFF_ADD_BG,
                    DIFF_ADD_TEXT,
                    "+",
                    with_alpha(DIFF_ADD_TEXT, 0.16),
                ),
                DiffLineKind::Del => (
                    DIFF_DEL_BG,
                    DIFF_DEL_TEXT,
                    "−",
                    with_alpha(DIFF_DEL_TEXT, 0.16),
                ),
                DiffLineKind::Context => (BG_SIDEBAR, TEXT, "", TRANSPARENT),
            };
            let num_fg = if *kind == DiffLineKind::Context {
                TEXT_MUTED
            } else {
                fg
            };
            let len = text.len();
            let marks = emphasis
                .iter()
                .filter(|r| r.start < r.end && r.end <= len)
                .filter(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
                .cloned()
                .collect::<Vec<_>>();
            let body = CodeLine::new(
                ix,
                SharedString::from(text.clone()),
                marks,
                mark,
                shared.clone(),
                focused,
            );
            (bg, fg, num_fg, *number, sign, body.into_any_element())
        }
        DiffRow::Separator { skipped } => {
            let label = match skipped {
                Some(n) if *n > 0 => {
                    format!("⋯  {}", base::fmt::plural(u64::from(*n), "line", "lines"))
                }
                _ => "⋯".to_string(),
            };
            (
                SURFACE,
                TEXT_MUTED,
                TEXT_MUTED,
                None,
                "",
                label.into_any_element(),
            )
        }
        DiffRow::NoNewline => (
            BG_SIDEBAR,
            TEXT_MUTED,
            TEXT_MUTED,
            None,
            "",
            "\\ No newline at end of file".into_any_element(),
        ),
    };
    let gutter = GUTTER_WIDTH + SIGN_WIDTH;
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .h(px(ROW_HEIGHT))
        .min_w_full()
        .pr(px(SPACE_3))
        .bg(bg)
        .type_style(MONO)
        .text_color(fg)
        .whitespace_nowrap()
        .child(div().w(px(gutter)).flex_none())
        .child(div().flex_none().child(body))
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(gutter_x))
                .h_full()
                .w(px(gutter))
                .flex()
                .items_center()
                .bg(bg)
                .text_color(num_fg)
                .child(
                    div()
                        .w(px(GUTTER_WIDTH))
                        .pr(px(SPACE_3))
                        .flex()
                        .justify_end()
                        .children(number.map(|n| n.to_string())),
                )
                .child(div().w(px(SIGN_WIDTH)).child(sign)),
        )
        .into_any_element()
}
