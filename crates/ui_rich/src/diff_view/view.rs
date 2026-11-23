//! [`DiffView`]: the rows of one file's diff, drawn through `uniform_list` so a diff of
//! thousands of lines only builds the rows on screen.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, HighlightStyle, IntoElement, ListHorizontalSizingBehavior,
    RenderOnce, SharedString, StyledText, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};
use ui::theme::*;

use super::model::{DiffFile, DiffLineKind, DiffRow, diff_rows};

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
    /// file and kept while it is shown.
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

/// Rows of the file last shown, and the scroll position, kept across frames.
struct DiffState {
    file: Option<Arc<DiffFile>>,
    rows: Rc<Vec<DiffRow>>,
    scroll: UniformListScrollHandle,
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
            window.use_keyed_state("diff-state", cx, |_, _| DiffState {
                file: None,
                rows: Rc::default(),
                scroll: UniformListScrollHandle::new(),
            })
        });
        let file = self.file.clone();
        let (rows, scroll) = state.update(cx, |s, _| {
            let same = s
                .file
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &file) || **f == *file);
            if !same {
                s.rows = Rc::new(diff_rows(&file));
                s.file = Some(file);
            }
            (s.rows.clone(), s.scroll.clone())
        });
        let count = self.limit.map_or(rows.len(), |l| l.min(rows.len()));
        let longest = rows[..count]
            .iter()
            .enumerate()
            .max_by_key(|(_, r)| match r {
                DiffRow::Line { text, .. } => text.len(),
                _ => 0,
            })
            .map(|(i, _)| i);
        let height = (count as f32 * ROW_HEIGHT + 2. * PAD_Y).min(self.max_height);
        let handle = scroll.clone();
        let list = uniform_list(
            ElementId::NamedChild(Box::new(self.id.clone()), "rows".into()),
            count,
            move |range, _, _| {
                // Rows move with the horizontal scroll; the gutter moves back by as much.
                let x = (-f32::from(handle.0.borrow().base_handle.offset().x)).max(0.);
                range
                    .filter_map(|ix| rows.get(ix).map(|r| row(r, x)))
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
        if self.fill {
            list.flex_1().min_h(px(0.)).into_any_element()
        } else {
            list.flex_none().h(px(height)).into_any_element()
        }
    }
}

/// One drawn row; `gutter_x` keeps the gutter at the left edge while the row scrolls.
fn row(r: &DiffRow, gutter_x: f32) -> AnyElement {
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
                .map(|r| {
                    (
                        r.clone(),
                        HighlightStyle {
                            background_color: Some(mark),
                            ..Default::default()
                        },
                    )
                })
                .collect::<Vec<_>>();
            let body = StyledText::new(SharedString::from(text.clone())).with_highlights(marks);
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
