//! The diff input types and the row model drawn from them, without GPUI: one row per
//! line, separators between hunks, the no-newline note, intra-line emphasis.

use std::ops::Range;

use super::highlight::emphasis;

/// One file's change, as `model::FileDiff` has it (field for field, so callers convert
/// trivially).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffFile {
    /// Path of the file (after a rename).
    pub path: String,
    /// The path before a rename.
    pub old_path: Option<String>,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
    /// The file did not exist before.
    pub created: bool,
    /// The file no longer exists.
    pub deleted: bool,
    /// Not text: there are no hunks to show.
    pub binary: bool,
    /// The changed regions.
    pub hunks: Vec<DiffHunk>,
}

/// A run of changed lines with their context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffHunk {
    /// The lines, in order.
    pub lines: Vec<DiffLine>,
}

/// What a diff line is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DiffLineKind {
    /// Unchanged.
    #[default]
    Context,
    /// Added.
    Add,
    /// Removed.
    Del,
}

/// One line of a hunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffLine {
    /// Added, removed or unchanged.
    pub kind: DiffLineKind,
    /// Line number in the old file (none for an added line).
    pub old_no: Option<u32>,
    /// Line number in the new file (none for a removed line).
    pub new_no: Option<u32>,
    /// The text, without its newline.
    pub text: String,
    /// The line is the last of its file and has no newline after it.
    pub no_newline: bool,
}

/// Lines longer than this (in bytes) are shown cut, ending in `…`.
pub const MAX_LINE: usize = 2000;

/// One row of a drawn diff.
#[derive(Clone, Debug, PartialEq)]
pub enum DiffRow {
    /// A line of a hunk.
    Line {
        /// Added, removed or unchanged.
        kind: DiffLineKind,
        /// The gutter number: the new line number, or the old one for a removed line.
        number: Option<u32>,
        /// Display text: tabs expanded, cut at [`MAX_LINE`].
        text: String,
        /// Byte ranges of `text` that changed against its paired line.
        emphasis: Vec<Range<usize>>,
    },
    /// Between two hunks: `⋯` and how many lines it skips, when known.
    Separator {
        /// Unchanged lines between the hunks.
        skipped: Option<u32>,
    },
    /// `\ No newline at end of file` after the line it is about.
    NoNewline,
}

/// What a diff with no rows shows instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffEmpty {
    /// "Binary file".
    Binary,
    /// "File deleted".
    Deleted,
    /// "No changes".
    NoChanges,
}

impl DiffEmpty {
    /// The text shown.
    pub fn label(self) -> &'static str {
        match self {
            DiffEmpty::Binary => "Binary file",
            DiffEmpty::Deleted => "File deleted",
            DiffEmpty::NoChanges => "No changes",
        }
    }
}

impl DiffFile {
    /// Why the file has nothing to draw, if it has not.
    pub fn empty_state(&self) -> Option<DiffEmpty> {
        if self.binary {
            return Some(DiffEmpty::Binary);
        }
        if self.hunks.iter().all(|h| h.lines.is_empty()) {
            return Some(if self.deleted {
                DiffEmpty::Deleted
            } else {
                DiffEmpty::NoChanges
            });
        }
        None
    }

    /// Number of line rows (what "Show all N lines" counts).
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

/// The rows of `file`, in drawing order.
pub fn diff_rows(file: &DiffFile) -> Vec<DiffRow> {
    diff_rows_with_sources(file).0
}

/// Where a line row's text comes from: `file.hunks[hunk].lines[line]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineRef {
    pub hunk: usize,
    pub line: usize,
}

/// The rows of `file` and, row for row, the source line behind each line row (what a copy
/// of the row gives back: tabs and the full text rather than the shown one).
pub(crate) fn diff_rows_with_sources(file: &DiffFile) -> (Vec<DiffRow>, Vec<Option<LineRef>>) {
    if file.empty_state().is_some() {
        return (Vec::new(), Vec::new());
    }
    let cap = file.line_count() + file.hunks.len();
    let mut rows = Vec::with_capacity(cap);
    let mut sources = Vec::with_capacity(cap);
    let mut prev: Option<&DiffHunk> = None;
    for (h, hunk) in file
        .hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| !h.lines.is_empty())
    {
        if let Some(p) = prev {
            rows.push(DiffRow::Separator {
                skipped: skipped_between(p, hunk),
            });
            sources.push(None);
        }
        prev = Some(hunk);
        let texts: Vec<String> = hunk.lines.iter().map(|l| display_text(&l.text)).collect();
        let marks = pair_emphasis(&hunk.lines, &texts);
        for (l, ((line, text), emphasis)) in hunk.lines.iter().zip(texts).zip(marks).enumerate() {
            let number = match line.kind {
                DiffLineKind::Del => line.old_no,
                _ => line.new_no.or(line.old_no),
            };
            rows.push(DiffRow::Line {
                kind: line.kind,
                number,
                text,
                emphasis,
            });
            sources.push(Some(LineRef { hunk: h, line: l }));
            if line.no_newline {
                rows.push(DiffRow::NoNewline);
                sources.push(None);
            }
        }
    }
    (rows, sources)
}

/// The rows shown with a limit of `limit` rows when not `expanded`, and how many line rows
/// are hidden behind "Show all".
pub fn visible_rows(total: usize, limit: Option<usize>, expanded: bool) -> (usize, bool) {
    match limit {
        Some(l) if !expanded && total > l => (l, true),
        _ => (total, false),
    }
}

/// Lines between two hunks: the old (else new) numbers either side.
fn skipped_between(a: &DiffHunk, b: &DiffHunk) -> Option<u32> {
    let last = |h: &DiffHunk, old: bool| {
        h.lines
            .iter()
            .rev()
            .find_map(|l| if old { l.old_no } else { l.new_no })
    };
    let first = |h: &DiffHunk, old: bool| {
        h.lines
            .iter()
            .find_map(|l| if old { l.old_no } else { l.new_no })
    };
    [true, false].into_iter().find_map(|old| {
        let (end, start) = (last(a, old)?, first(b, old)?);
        start.checked_sub(end)?.checked_sub(1)
    })
}

/// How many spaces a tab is shown as (GPUI has no tab stops).
pub(crate) const TAB_WIDTH: usize = 4;

/// Tabs expanded to [`TAB_WIDTH`] spaces, a trailing `\r` dropped, cut at [`MAX_LINE`] bytes.
pub(crate) fn display_text(text: &str) -> String {
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut out = text.replace('\t', &" ".repeat(TAB_WIDTH));
    if out.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

/// Emphasis for every line of a hunk: each run of removed lines followed directly by added
/// lines is paired line by line.
fn pair_emphasis(lines: &[DiffLine], texts: &[String]) -> Vec<Vec<Range<usize>>> {
    let mut out = vec![Vec::new(); lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != DiffLineKind::Del {
            i += 1;
            continue;
        }
        let dels = i;
        while i < lines.len() && lines[i].kind == DiffLineKind::Del {
            i += 1;
        }
        let adds = i;
        while i < lines.len() && lines[i].kind == DiffLineKind::Add {
            i += 1;
        }
        for k in 0..(adds - dels).min(i - adds) {
            let (o, n) = (dels + k, adds + k);
            if let Some((a, b)) = emphasis(&texts[o], &texts[n]) {
                out[o] = a;
                out[n] = b;
            }
        }
    }
    out
}
