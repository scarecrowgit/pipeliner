//! The comparison table: one column per variant, one row per measure, the chosen variant's
//! column on the accent glow.

use gpui::{AnyElement, Context, ElementId, Hsla, IntoElement, Window, div, prelude::*, px};
use hub::api::{Review, Variant, variant_letter};
use ui::dev::Probe;
use ui::prelude::*;

use crate::review::{letter_tile, tests_color, tests_label};
use crate::texts::{Look, cell};
use crate::view::InboxView;

/// Width of the row labels ("Approach", "Lines"…).
const LABEL_WIDTH: f32 = 104.;
/// Files listed in a cell before "N more files".
const FILES_SHOWN: usize = 6;

/// The rows of the table, top to bottom.
const ROWS: [&str; 6] = ["Approach", "Lines", "Files", "Tests", "Time", "Cost"];

impl InboxView {
    /// The variants side by side.
    pub(crate) fn render_table(
        &mut self,
        review: &Review,
        chosen: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let file = self.review.as_ref().map_or(0, |r| r.file);
        let n = review.variants.len();
        let mut head = div()
            .flex()
            .flex_row()
            .border_b_1()
            .border_color(LINE)
            .child(div().flex_none().w(px(LABEL_WIDTH)));
        for (col_ix, variant) in review.variants.iter().enumerate() {
            head = head.child(self.head_cell(
                variant,
                chosen == Some(variant.index),
                col_ix + 1 == n,
                window,
                cx,
            ));
        }
        let mut rows: Vec<AnyElement> = vec![head.into_any_element()];
        for (row_ix, label) in ROWS.iter().enumerate() {
            let mut line = div()
                .flex()
                .flex_row()
                .when(row_ix + 1 < ROWS.len(), |d| {
                    d.border_b_1().border_color(LINE_SUBTLE)
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(LABEL_WIDTH))
                        .pl(px(SPACE_4))
                        .pr(px(SPACE_3))
                        .py(px(SPACE_2_5))
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .child(*label),
                );
            for (col_ix, variant) in review.variants.iter().enumerate() {
                let selected = chosen == Some(variant.index);
                let content = self.cell_content(row_ix, variant, selected, file, cx);
                let bg = cell_bg(row_ix + 1, variant.index, selected, window, cx);
                line = line.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .py(px(SPACE_2_5))
                        .pl(px(SPACE_3))
                        .pr(px(if col_ix + 1 == n { SPACE_4 } else { SPACE_3 }))
                        .bg(bg)
                        .child(content),
                );
            }
            rows.push(line.into_any_element());
        }
        div()
            .debug_selector(|| "review-table".into())
            .child(
                Card::new().flush().child(
                    col()
                        .overflow_hidden()
                        .rounded(px(RADIUS_XL))
                        .children(rows),
                ),
            )
            .into_any_element()
    }

    /// A variant's head: letter, name, then "Selected" or "Select B 2". The whole cell picks.
    fn head_cell(
        &mut self,
        variant: &Variant,
        selected: bool,
        last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let index = variant.index;
        let letter = variant_letter(index);
        let name = format!("review-variant-{index}");
        let selector = name.clone();
        let bg = cell_bg(0, index, selected, window, cx);
        let action = if selected {
            Badge::new("Selected")
                .tone(BadgeTone::Accent)
                .icon(IconName::Check)
                .into_any_element()
        } else {
            Button::new(
                ElementId::Name(format!("review-select-{index}").into()),
                format!("Select {letter}"),
            )
            .secondary()
            .small()
            .kbd(index.to_string())
            .on_click(cx.listener(move |this, _, _, cx| this.pick_variant(index, cx)))
            .into_any_element()
        };
        div()
            .id(ElementId::Name(name.clone().into()))
            .debug_selector(move || selector)
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .p(px(SPACE_3))
            .when(last, |d| d.pr(px(SPACE_4)))
            .bg(bg)
            .cursor_pointer()
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(ACCENT),
                )
            })
            .child(
                row()
                    .items_start()
                    .gap(px(SPACE_2_5))
                    .child(letter_tile(index, selected, false))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pt(px(2.))
                            .type_style(BODY_STRONG)
                            .child(variant.name.clone()),
                    ),
            )
            .child(row().mt(px(SPACE_2_5)).child(action))
            .on_click(cx.listener(move |this, _, _, cx| this.pick_variant(index, cx)))
            .probe(name)
            .into_any_element()
    }

    /// What one cell of row `row_ix` shows for `variant`.
    fn cell_content(
        &mut self,
        row_ix: usize,
        variant: &Variant,
        selected: bool,
        file: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match row_ix {
            0 => {
                let approach = self.texts.get_in(
                    format!("approach-{}", variant.index),
                    variant.approach.clone(),
                    // Written by the variant's agent, as Markdown.
                    Look::markdown(BODY, TEXT_SECONDARY),
                    variant.worktree.clone(),
                    cx,
                );
                cell("review-approach", approach).into_any_element()
            }
            1 => counts(variant.added, variant.removed).into_any_element(),
            2 => self.files_cell(variant, selected, file, cx),
            3 => {
                let (text, ok) = tests_label(variant);
                div()
                    .text_color(tests_color(ok))
                    .child(text)
                    .into_any_element()
            }
            4 => div()
                .type_style(MONO)
                .tabular()
                .child(base::fmt::duration(variant.secs))
                .into_any_element(),
            _ => div()
                .type_style(MONO)
                .tabular()
                .child(base::fmt::cost(variant.cost_usd))
                .into_any_element(),
        }
    }

    /// The changed files of a variant; a click shows that file's diff.
    fn files_cell(
        &mut self,
        variant: &Variant,
        selected: bool,
        file: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if variant.files.is_empty() {
            return div()
                .text_color(TEXT_MUTED)
                .child("No files changed")
                .into_any_element();
        }
        let index = variant.index;
        let mut list = col().gap(px(SPACE_0_5)).mx(px(-SPACE_1_5));
        for (ix, change) in variant.files.iter().take(FILES_SHOWN).enumerate() {
            let current = selected && ix == file;
            let id = ElementId::Name(format!("review-file-{index}-{ix}").into());
            let name = format!("review-file-{ix}");
            let probe = name.clone();
            let el = div()
                .id(id.clone())
                .when(selected, |d| d.debug_selector(move || name))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .h(px(24.))
                .px(px(SPACE_1_5))
                .rounded(px(RADIUS_MD))
                .cursor_pointer()
                .type_style(MONO)
                .text_color(if current { TEXT } else { TEXT_SECONDARY })
                .child(truncate(short_path(&change.path)))
                .child(counts(change.added, change.removed))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_file(index, ix, cx)));
            let entry = motion::fade_row(el, id, current, SURFACE_RAISED);
            list = if selected {
                list.child(entry.probe(probe))
            } else {
                list.child(entry)
            };
        }
        let more = variant.files.len().saturating_sub(FILES_SHOWN);
        if more > 0 {
            list = list.child(
                div()
                    .px(px(SPACE_1_5))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(format!(
                        "{} more",
                        base::fmt::plural(more as u64, "file", "files")
                    )),
            );
        }
        list.into_any_element()
    }
}

/// `+18 −4` in the diff colours.
pub(crate) fn counts(added: u32, removed: u32) -> impl IntoElement {
    row()
        .flex_none()
        .gap(px(SPACE_1))
        .type_style(MONO)
        .tabular()
        .child(
            div()
                .text_color(SUCCESS)
                .child(format!("+{}", base::fmt::count(added.into()))),
        )
        .child(
            div()
                .text_color(DANGER)
                .child(format!("−{}", base::fmt::count(removed.into()))),
        )
}

/// The last two parts of a path, which is what fits in a column: `charges/retry.ts`.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// The background of one cell: the accent glow in the chosen column, fading.
fn cell_bg(
    row_ix: usize,
    variant: u32,
    selected: bool,
    window: &mut Window,
    cx: &mut Context<InboxView>,
) -> Hsla {
    motion::fade_color(
        ElementId::Name(format!("review-cell-{row_ix}-{variant}").into()),
        if selected { ACCENT_GLOW } else { TRANSPARENT },
        window,
        cx,
    )
}
