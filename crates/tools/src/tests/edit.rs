use model::{DiffLineKind, FileChange, ToolDisplay};
use serde_json::json;

use super::Fixture;
use crate::api::Tool;
use crate::{EditFile, WriteFile};

fn diff_of(display: &ToolDisplay) -> &model::FileDiff {
    match display {
        ToolDisplay::Diff(diff) => diff,
        other => panic!("expected a diff, got {other:?}"),
    }
}

#[test]
fn edit_replaces_one_exact_match() {
    let f = Fixture::new();
    f.write("src/lib.rs", "a\nb\nc\n");
    let out = f.run(&EditFile, json!({"path": "src/lib.rs", "old_string": "b\n", "new_string": "B\nB2\n"}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(f.read("src/lib.rs"), "a\nB\nB2\nc\n");
    assert_eq!(out.files, vec![FileChange { path: "src/lib.rs".into(), added: 2, removed: 1 }]);
    let diff = diff_of(&out.display);
    assert!(!diff.created);
    assert_eq!((diff.added, diff.removed), (2, 1));
    assert_eq!(EditFile.subject(&f.ctx(), &json!({"path": "src//./lib.rs"})), "src/lib.rs");
}

#[test]
fn edit_errors_say_what_to_fix() {
    let f = Fixture::new();
    f.write("a.rs", "    let x = 1;\nlet y = 1;\nlet y = 1;\n");
    let none = f.run(&EditFile, json!({"path": "a.rs", "old_string": "let x = 2;", "new_string": "z"}));
    assert!(none.is_error && none.content.contains("not found"), "{}", none.content);
    let near = f.run(&EditFile, json!({"path": "a.rs", "old_string": "let x = 1;\n  ", "new_string": "z"}));
    assert!(near.content.contains("at line 1"), "{}", near.content);
    let many = f.run(&EditFile, json!({"path": "a.rs", "old_string": "let y = 1;", "new_string": "z"}));
    assert!(many.is_error && many.content.contains("2 times") && many.content.contains("replace_all"));
    let missing = f.run(&EditFile, json!({"path": "nope.rs", "old_string": "a", "new_string": "b"}));
    assert!(missing.is_error && missing.content.contains("write_file"));
    let same = f.run(&EditFile, json!({"path": "a.rs", "old_string": "a", "new_string": "a"}));
    assert!(same.is_error);
    let empty = f.run(&EditFile, json!({"path": "a.rs", "old_string": "", "new_string": "a"}));
    assert!(empty.is_error);
    let wrong_type = f.run(&EditFile, json!({"path": "a.rs", "old_string": 5, "new_string": "a"}));
    assert!(wrong_type.content.contains("`old_string` must be a string"), "{}", wrong_type.content);
    assert_eq!(f.read("a.rs"), "    let x = 1;\nlet y = 1;\nlet y = 1;\n");
}

#[test]
fn edit_replace_all() {
    let f = Fixture::new();
    f.write("a.rs", "x x\nx\n");
    let out = f.run(&EditFile, json!({"path": "a.rs", "old_string": "x", "new_string": "y", "replace_all": true}));
    assert!(!out.is_error && out.content.contains("3 occurrences"), "{}", out.content);
    assert_eq!(f.read("a.rs"), "y y\ny\n");
}

#[test]
fn edit_keeps_crlf_line_endings() {
    let f = Fixture::new();
    f.write("w.txt", "one\r\ntwo\r\nthree\r\n");
    let out = f.run(&EditFile, json!({"path": "w.txt", "old_string": "one\ntwo", "new_string": "one\nTWO\nmore"}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(f.read("w.txt"), "one\r\nTWO\r\nmore\r\nthree\r\n");
    let diff = diff_of(&out.display);
    assert_eq!((diff.added, diff.removed), (2, 1));
}

#[test]
fn edit_refuses_binary_files() {
    let f = Fixture::new();
    std::fs::write(f.path("b.bin"), b"ab\0cd").unwrap();
    let out = f.run(&EditFile, json!({"path": "b.bin", "old_string": "ab", "new_string": "x"}));
    assert!(out.is_error && out.content.contains("binary"));
}

#[test]
fn write_creates_files_and_parents() {
    let f = Fixture::new();
    let out = f.run(&WriteFile, json!({"path": "new/deep/a.txt", "content": "1\n2\n"}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(f.read("new/deep/a.txt"), "1\n2\n");
    let diff = diff_of(&out.display);
    assert!(diff.created);
    assert_eq!((diff.added, diff.removed), (2, 0));
    assert_eq!(diff.hunks[0].lines[0].new_no, Some(1));
    assert_eq!(out.files, vec![FileChange { path: "new/deep/a.txt".into(), added: 2, removed: 0 }]);
    assert!(out.content.starts_with("Created new/deep/a.txt (2 lines)"), "{}", out.content);
}

#[test]
fn write_overwrites_with_a_diff() {
    let f = Fixture::new();
    f.write("a.txt", "keep\nold\n");
    let out = f.run(&WriteFile, json!({"path": "a.txt", "content": "keep\nnew\n"}));
    let diff = diff_of(&out.display);
    assert!(!diff.created);
    assert_eq!((diff.added, diff.removed), (1, 1));
    assert_eq!(f.read("a.txt"), "keep\nnew\n");
    // Writing the same content again changes nothing.
    let again = f.run(&WriteFile, json!({"path": "a.txt", "content": "keep\nnew\n"}));
    assert!(again.files.is_empty());
    let root = f.run(&WriteFile, json!({"path": ".", "content": "x"}));
    assert!(root.is_error);
    std::fs::create_dir(f.path("dir")).unwrap();
    assert!(f.run(&WriteFile, json!({"path": "dir", "content": "x"})).is_error);
}

#[test]
fn previews_show_the_diff_without_writing() {
    let f = Fixture::new();
    f.write("a.txt", "a\nb\n");
    let ctx = f.ctx();
    let edit = EditFile.preview(&ctx, &json!({"path": "a.txt", "old_string": "b", "new_string": "c"}));
    let diff = diff_of(&edit);
    assert_eq!((diff.added, diff.removed), (1, 1));
    let lines: Vec<_> = diff.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
    assert_eq!(lines, vec![(DiffLineKind::Context, "a"), (DiffLineKind::Del, "b"), (DiffLineKind::Add, "c")]);
    let write = WriteFile.preview(&ctx, &json!({"path": "sub/new.txt", "content": "x\n"}));
    assert!(diff_of(&write).created);
    assert_eq!(f.read("a.txt"), "a\nb\n");
    assert!(!f.path("sub").exists());
    // A preview of a bad call explains instead of failing.
    let bad = EditFile.preview(&ctx, &json!({"path": "a.txt", "old_string": "zzz", "new_string": "c"}));
    assert!(matches!(bad, ToolDisplay::Text { ref body, .. } if body.contains("not found")));
}
