use model::DiffLineKind::{Add, Context, Del};
use model::{DiffLine, DiffLineKind};

use crate::diff::{file_diff, unified};

fn rows(lines: &[DiffLine]) -> Vec<(DiffLineKind, Option<u32>, Option<u32>, &str)> {
    lines.iter().map(|l| (l.kind, l.old_no, l.new_no, l.text.as_str())).collect()
}

#[test]
fn hunks_have_context_and_line_numbers_on_both_sides() {
    let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
    // Line 2 removed, line 10 changed.
    let new_text: String = (1..=20)
        .filter(|&i| i != 2)
        .map(|i| if i == 10 { "ten\n".to_string() } else { format!("{i}\n") })
        .collect();
    let d = file_diff("f.txt", &old, &new_text, false);
    assert_eq!((d.added, d.removed), (1, 2));
    assert_eq!(d.hunks.len(), 2);
    assert_eq!(
        rows(&d.hunks[0].lines),
        vec![
            (Context, Some(1), Some(1), "1"),
            (Del, Some(2), None, "2"),
            (Context, Some(3), Some(2), "3"),
            (Context, Some(4), Some(3), "4"),
            (Context, Some(5), Some(4), "5"),
        ]
    );
    assert_eq!(
        rows(&d.hunks[1].lines),
        vec![
            (Context, Some(7), Some(6), "7"),
            (Context, Some(8), Some(7), "8"),
            (Context, Some(9), Some(8), "9"),
            (Del, Some(10), None, "10"),
            (Add, None, Some(9), "ten"),
            (Context, Some(11), Some(10), "11"),
            (Context, Some(12), Some(11), "12"),
            (Context, Some(13), Some(12), "13"),
        ]
    );
    let text = unified(&d);
    assert!(text.starts_with("--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,4 @@\n 1\n-2\n 3\n"), "{text}");
    assert!(text.contains("@@ -7,7 +6,7 @@\n 7\n 8\n 9\n-10\n+ten\n 11\n"), "{text}");
}

#[test]
fn identical_texts_have_no_hunks() {
    let d = file_diff("a", "x\ny\n", "x\ny\n", false);
    assert!(d.hunks.is_empty());
    assert_eq!((d.added, d.removed), (0, 0));
}

#[test]
fn new_files_diff_against_nothing() {
    let d = file_diff("n.rs", "", "a\nb", true);
    assert!(d.created);
    assert_eq!((d.added, d.removed), (2, 0));
    assert_eq!(rows(&d.hunks[0].lines), vec![(Add, None, Some(1), "a"), (Add, None, Some(2), "b")]);
    assert!(unified(&d).starts_with("--- /dev/null\n+++ b/n.rs\n@@ -0,0 +1,2 @@\n+a\n+b\n"));
}

#[test]
fn missing_trailing_newline_counts_as_a_change_of_the_last_line() {
    let d = file_diff("a", "x\ny", "x\ny\n", false);
    assert_eq!((d.added, d.removed), (1, 1));
    assert_eq!(rows(&d.hunks[0].lines), vec![(Context, Some(1), Some(1), "x"), (Del, Some(2), None, "y"), (Add, None, Some(2), "y")]);
    // Appending to a file without a trailing newline.
    let d = file_diff("a", "x", "x\nz", false);
    assert_eq!((d.added, d.removed), (2, 1));
}

#[test]
fn crlf_lines_compare_and_show_without_endings() {
    let d = file_diff("w", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n", false);
    assert_eq!((d.added, d.removed), (1, 1));
    assert_eq!(
        rows(&d.hunks[0].lines),
        vec![(Context, Some(1), Some(1), "a"), (Del, Some(2), None, "b"), (Add, None, Some(2), "B"), (Context, Some(3), Some(3), "c")]
    );
    // Converting endings changes every line.
    let d = file_diff("w", "a\r\nb\r\n", "a\nb\n", false);
    assert_eq!((d.added, d.removed), (2, 2));
}
