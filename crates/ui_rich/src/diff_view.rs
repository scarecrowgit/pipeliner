//! Diffs as the mockups draw them.
//!
//! - [`DiffFile`], [`DiffHunk`], [`DiffLine`]: the input, field for field what
//!   `model::FileDiff` has (this crate does not depend on `model`; callers convert).
//! - [`diff_rows`]: the GPUI-free row model (gutter numbers, hunk separators, the
//!   no-newline note, intra-line [`emphasis`]).
//! - [`DiffView`]: the rows, through `uniform_list`, on the sidebar ground; the code can be
//!   selected with the mouse and copied (the code only: no numbers, no `+`/`−`).
//! - [`DiffCard`]: the card around a [`DiffView`] with its header, footer, collapse and
//!   "Show all N lines".

mod card;
mod highlight;
mod model;
mod select;
mod view;

#[cfg(test)]
mod tests;

pub use card::{DiffCard, DiffCardState};
pub use highlight::{MAX_EMPHASIS_LINE, Marks, emphasis};
pub use model::{
    DiffEmpty, DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffRow, MAX_LINE, diff_rows,
    visible_rows,
};
pub use view::{DiffView, GUTTER_WIDTH, ROW_HEIGHT, SIGN_WIDTH};
