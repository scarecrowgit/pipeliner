//! The diff of the chosen file of the chosen variant, read on demand, with its loading and
//! error states.

use std::sync::Arc;

use gpui::{AnyElement, Context, ElementId, IntoElement, SharedString, div, prelude::*, px};
use hub::LoadState;
use hub::api::{FileDiff, Variant};
use ui::prelude::*;
use ui_rich::diff_view::{DiffCard, DiffFile, DiffHunk, DiffLine, DiffLineKind};

use crate::review::letter_tile;
use crate::table::counts;
use crate::view::InboxView;

/// How tall the diff gets before it scrolls inside.
const DIFF_MAX_HEIGHT: f32 = 560.;

/// The kit's diff input from the model's.
pub(crate) fn to_diff_file(file: &FileDiff) -> DiffFile {
    DiffFile {
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        added: file.added,
        removed: file.removed,
        created: file.created,
        deleted: file.deleted,
        binary: file.binary,
        hunks: file
            .hunks
            .iter()
            .map(|hunk| DiffHunk {
                lines: hunk
                    .lines
                    .iter()
                    .map(|line| DiffLine {
                        kind: match line.kind {
                            hub::api::DiffLineKind::Context => DiffLineKind::Context,
                            hub::api::DiffLineKind::Add => DiffLineKind::Add,
                            hub::api::DiffLineKind::Del => DiffLineKind::Del,
                        },
                        old_no: line.old_no,
                        new_no: line.new_no,
                        text: line.text.clone(),
                        no_newline: line.no_newline,
                    })
                    .collect(),
            })
            .collect(),
    }
}

impl InboxView {
    /// The diff card of the chosen file: the diff, or a card saying it is loading or failed.
    pub(crate) fn render_diff(&mut self, variant: &Variant, cx: &mut Context<Self>) -> AnyElement {
        let Some(open) = self.review.as_ref() else {
            return div().into_any_element();
        };
        let total = variant.files.len();
        let Some(change) = variant.files.get(open.file).cloned() else {
            return frame(variant.index, None, "No files changed", false);
        };
        let file_ix = open.file;
        let index = variant.index;
        let loaded = open
            .model
            .read(cx)
            .diff(index, &change.path)
            .map(|d| (d.value().cloned(), d.state().clone()));
        match loaded {
            Some((Some(diff), _)) => {
                let id = ElementId::Name(format!("review-diff-{index}-{file_ix}").into());
                let file: Arc<DiffFile> = Arc::new(to_diff_file(&diff));
                div()
                    .debug_selector(|| "review-diff".into())
                    .child(
                        DiffCard::new(id, file)
                            .lead(letter_tile(index, true, true))
                            .max_height(DIFF_MAX_HEIGHT)
                            .pager(
                                file_ix,
                                total,
                                cx.listener(move |this, _, _, cx| {
                                    this.pick_file(index, file_ix + 1, cx)
                                }),
                            ),
                    )
                    .into_any_element()
            }
            Some((None, LoadState::Failed(error))) => frame(
                index,
                Some((change.path.clone(), change.added, change.removed)),
                format!("Could not load the diff: {error}"),
                true,
            ),
            _ => frame(
                index,
                Some((change.path.clone(), change.added, change.removed)),
                "Loading the diff…",
                false,
            ),
        }
    }
}

/// A diff card without a diff: its header and one line of text.
fn frame(
    index: u32,
    file: Option<(String, u32, u32)>,
    text: impl Into<SharedString>,
    error: bool,
) -> AnyElement {
    let header = row()
        .flex_none()
        .gap(px(SPACE_2))
        .h(px(40.))
        .pl(px(SPACE_3_5))
        .pr(px(SPACE_2))
        .border_b_1()
        .border_color(LINE)
        .child(letter_tile(index, true, true))
        .when_some(file, |d, (path, added, removed)| {
            d.child(
                div()
                    .min_w_0()
                    .flex_shrink()
                    .type_style(MONO)
                    .child(truncate(path)),
            )
            .child(counts(added, removed))
        });
    let selector = if error {
        "review-diff-error"
    } else {
        "review-diff-loading"
    };
    div()
        .debug_selector(move || selector.into())
        .child(
            Card::new().flush().child(
                col().child(header).child(
                    div()
                        .px(px(SPACE_4))
                        .py(px(SPACE_4))
                        .bg(BG_SIDEBAR)
                        .type_style(LABEL)
                        .text_color(if error { DANGER } else { TEXT_MUTED })
                        .child(text.into()),
                ),
            ),
        )
        .into_any_element()
}
