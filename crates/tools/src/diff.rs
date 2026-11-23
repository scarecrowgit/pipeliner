//! Line diffs: [`model::FileDiff`] for people (hunks with line numbers on both sides) and
//! [`unified`] text for the model.
//!
//! A line is compared with its line ending, as git does: adding the missing newline at the end
//! of a file, or turning CRLF into LF, shows as a changed line. Shown text has the ending
//! removed.

use std::fmt::Write as _;
use std::time::Duration;

use model::{DiffHunk, DiffLine, DiffLineKind, FileDiff};
use similar::{ChangeTag, TextDiff};

/// Unchanged lines shown around each change.
pub const CONTEXT_LINES: usize = 3;
/// After this long the diff algorithm settles for a coarser but still correct result.
const DIFF_TIMEOUT: Duration = Duration::from_millis(500);

/// The diff from `old` to `new` for the file at `path` (as shown, relative to the workdir).
/// `created` marks a file that did not exist before.
pub fn file_diff(path: &str, old: &str, new: &str, created: bool) -> FileDiff {
    let diff = TextDiff::configure().timeout(DIFF_TIMEOUT).diff_lines(old, new);
    let mut out = FileDiff { path: path.to_string(), created, ..FileDiff::default() };
    for group in diff.grouped_ops(CONTEXT_LINES) {
        let mut hunk = DiffHunk::default();
        for op in &group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => DiffLineKind::Context,
                    ChangeTag::Delete => {
                        out.removed += 1;
                        DiffLineKind::Del
                    }
                    ChangeTag::Insert => {
                        out.added += 1;
                        DiffLineKind::Add
                    }
                };
                hunk.lines.push(DiffLine {
                    kind,
                    old_no: change.old_index().map(line_no),
                    new_no: change.new_index().map(line_no),
                    text: strip_eol(change.value()).to_string(),
                    no_newline: change.missing_newline(),
                });
            }
        }
        if !hunk.lines.is_empty() {
            out.hunks.push(hunk);
        }
    }
    out
}

/// The diff as unified text (`---`, `+++`, `@@ -a,b +c,d @@`), for a tool result.
pub fn unified(diff: &FileDiff) -> String {
    let mut out = String::new();
    let old_name = if diff.created { "/dev/null".to_string() } else { format!("a/{}", diff.path) };
    let _ = writeln!(out, "--- {old_name}");
    let _ = writeln!(out, "+++ b/{}", diff.path);
    for hunk in &diff.hunks {
        let (old_start, old_len) = span(hunk.lines.iter().filter_map(|l| l.old_no));
        let (new_start, new_len) = span(hunk.lines.iter().filter_map(|l| l.new_no));
        let _ = writeln!(out, "@@ -{old_start},{old_len} +{new_start},{new_len} @@");
        for line in &hunk.lines {
            let sign = match line.kind {
                DiffLineKind::Context => ' ',
                DiffLineKind::Add => '+',
                DiffLineKind::Del => '-',
            };
            let _ = writeln!(out, "{sign}{}", line.text);
            if line.no_newline {
                let _ = writeln!(out, "\\ No newline at end of file");
            }
        }
    }
    out
}

/// First line number and count of a hunk side; `0,0` for a side with no lines.
fn span(numbers: impl Iterator<Item = u32>) -> (u32, u32) {
    let mut first = None;
    let mut count = 0;
    for n in numbers {
        first.get_or_insert(n);
        count += 1;
    }
    (first.unwrap_or(0), count)
}

/// A 0-based line index as a 1-based line number, saturating for absurdly long files.
fn line_no(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

fn strip_eol(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}
