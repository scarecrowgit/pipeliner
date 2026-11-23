use model::ToolDisplay;
use serde_json::json;

use super::Fixture;
use crate::api::Tool;
use crate::{Grep, ListDir, ReadFile};

fn line_result(display: &ToolDisplay) -> (&str, &str, &str) {
    match display {
        ToolDisplay::Line { verb, target, result } => (verb, target, result),
        other => panic!("expected a line display, got {other:?}"),
    }
}

#[test]
fn read_file_numbers_lines() {
    let f = Fixture::new();
    f.write("src/x.rs", "fn a() {}\r\nfn b() {}\n");
    let out = f.run(&ReadFile, json!({"path": "src/x.rs"}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "     1\tfn a() {}\n     2\tfn b() {}\n");
    assert_eq!(line_result(&out.display), ("read", "src/x.rs", "2 lines"));
    assert_eq!(ReadFile.subject(&json!({"path": "./src/x.rs"})), "src/x.rs");
}

#[test]
fn read_file_pages_with_offset_and_limit() {
    let f = Fixture::new();
    let text: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    f.write("a.txt", &text);
    let out = f.run(&ReadFile, json!({"path": "a.txt", "offset": 3, "limit": 2}));
    assert!(out.content.starts_with("     3\tline 3\n     4\tline 4\n"), "{}", out.content);
    assert!(out.content.contains("lines 3-4 of 10") && out.content.contains("offset=5"), "{}", out.content);
    assert_eq!(line_result(&out.display).2, "2 of 10 lines");
    let tail = f.run(&ReadFile, json!({"path": "a.txt", "offset": "9"}));
    assert!(tail.content.contains("    10\tline 10") && !tail.content.contains("offset="), "{}", tail.content);
    let past = f.run(&ReadFile, json!({"path": "a.txt", "offset": 11}));
    assert!(past.is_error && past.content.contains("past the end"), "{}", past.content);
}

#[test]
fn read_file_caps_lines_and_bytes() {
    let f = Fixture::new();
    let text: String = (1..=2500).map(|i| format!("{i}\n")).collect();
    f.write("long.txt", &text);
    let out = f.run(&ReadFile, json!({"path": "long.txt", "limit": 5000}));
    assert!(out.content.contains("  2000\t2000\n") && !out.content.contains("\t2001\n"));
    assert!(out.content.contains("offset=2001"), "{}", out.content);

    let wide = "x".repeat(1000);
    let text: String = (0..1000).map(|_| format!("{wide}\n")).collect();
    f.write("wide.txt", &text);
    let out = f.run(&ReadFile, json!({"path": "wide.txt"}));
    assert!(out.content.len() <= crate::read::READ_MAX_BYTES + 200, "{}", out.content.len());
    assert!(out.content.contains("256 KB") && out.content.contains("to continue"), "{}", &out.content[out.content.len() - 200..]);
}

#[test]
fn read_file_refuses_binary_dirs_and_missing_files() {
    let f = Fixture::new();
    std::fs::write(f.path("img.png"), b"\x89PNG\0\0\0binary").unwrap();
    let bin = f.run(&ReadFile, json!({"path": "img.png"}));
    assert!(bin.is_error && bin.content.contains("binary"), "{}", bin.content);
    std::fs::create_dir(f.path("dir")).unwrap();
    assert!(f.run(&ReadFile, json!({"path": "dir"})).content.contains("list_dir"));
    let missing = f.run(&ReadFile, json!({"path": "nope.rs"}));
    assert!(missing.is_error && missing.content.contains("not found"));
    let no_path = f.run(&ReadFile, json!({}));
    assert!(no_path.is_error && no_path.content.contains("`path`") && no_path.content.contains("Example"));
    f.write("empty.txt", "");
    let empty = f.run(&ReadFile, json!({"path": "empty.txt"}));
    assert!(!empty.is_error && empty.content.contains("empty"));
}

#[test]
fn read_file_accepts_input_sent_as_a_json_string() {
    let f = Fixture::new();
    f.write("a.txt", "hi\n");
    let out = f.run(&ReadFile, json!("{\"path\": \"a.txt\"}"));
    assert!(!out.is_error && out.content.contains("hi"));
}

#[test]
fn list_dir_puts_dirs_first_and_respects_gitignore() {
    let f = Fixture::new();
    f.write(".gitignore", "target/\n*.log\n");
    f.write("b.txt", "");
    f.write("a.txt", "");
    f.write("debug.log", "");
    f.write("zdir/x", "");
    f.write("adir/x", "");
    f.write("target/out", "");
    f.write(".git/HEAD", "");
    let out = f.run(&ListDir, json!({}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "adir/\nzdir/\n.gitignore\na.txt\nb.txt\n");
    assert_eq!(line_result(&out.display), ("list", ".", "5 entries"));
    let sub = f.run(&ListDir, json!({"path": "adir"}));
    assert_eq!(sub.content, "x\n");
    assert!(f.run(&ListDir, json!({"path": "a.txt"})).content.contains("read_file"));
    assert!(f.run(&ListDir, json!({"path": "missing"})).is_error);
}

#[test]
fn list_dir_caps_entries() {
    let f = Fixture::new();
    for i in 0..520 {
        f.write(&format!("f{i:04}"), "");
    }
    let out = f.run(&ListDir, json!({"path": "."}));
    assert_eq!(out.content.lines().filter(|l| l.starts_with('f')).count(), 500);
    assert!(out.content.contains("20 more entries"), "{}", out.content);
    assert_eq!(line_result(&out.display).2, "520 entries");
}

#[test]
fn grep_finds_matches_with_paths_and_lines() {
    let f = Fixture::new();
    f.write(".gitignore", "ignored.rs\n");
    f.write("src/a.rs", "fn main() {}\nlet x = 1;\nfn helper() {}\n");
    f.write("src/b.ts", "function main() {}\n");
    f.write("ignored.rs", "fn main() {}\n");
    f.write(".hidden/c.rs", "fn main() {}\n");
    std::fs::write(f.path("bin.dat"), b"fn main\0").unwrap();
    let out = f.run(&Grep, json!({"pattern": "fn \\w+\\("}));
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.starts_with("src/a.rs:1: fn main() {}\nsrc/a.rs:3: fn helper() {}\n"), "{}", out.content);
    assert!(!out.content.contains("ignored.rs") && !out.content.contains(".hidden") && !out.content.contains("bin.dat"));
    assert_eq!(line_result(&out.display), ("grep", "fn \\w+\\(", "2 matches · 1 file"));

    let all = f.run(&Grep, json!({"pattern": "main"}));
    assert_eq!(line_result(&all.display).2, "2 matches · 2 files");
    let globbed = f.run(&Grep, json!({"pattern": "main", "glob": "*.ts"}));
    assert_eq!(globbed.content.lines().next(), Some("src/b.ts:1: function main() {}"));
    let deep_glob = f.run(&Grep, json!({"pattern": "main", "glob": "src/**/*.rs"}));
    assert_eq!(line_result(&deep_glob.display).2, "1 match · 1 file");
    let in_file = f.run(&Grep, json!({"pattern": "let", "path": "src/a.rs"}));
    assert!(in_file.content.starts_with("src/a.rs:2: let x = 1;"), "{}", in_file.content);
    let none = f.run(&Grep, json!({"pattern": "nothing_here"}));
    assert!(!none.is_error && none.content.contains("No matches"));
    assert_eq!(line_result(&none.display).2, "no matches");
}

#[test]
fn grep_caps_matches_and_reports_bad_input() {
    let f = Fixture::new();
    let text: String = (0..300).map(|i| format!("hit {i}\n")).collect();
    f.write("many.txt", &text);
    let out = f.run(&Grep, json!({"pattern": "hit"}));
    assert_eq!(out.content.lines().filter(|l| l.starts_with("many.txt:")).count(), 200);
    assert!(out.content.contains("stopped at 200"), "{}", out.content);

    let bad = f.run(&Grep, json!({"pattern": "(unclosed"}));
    assert!(bad.is_error && bad.content.contains("regular expression"));
    assert!(f.run(&Grep, json!({"pattern": "x", "glob": "a[" })).is_error);
    assert!(f.run(&Grep, json!({"pattern": "x", "path": "../.."})).is_error);
    assert!(f.run(&Grep, json!({"pattern": "x", "path": "missing"})).content.contains("not found"));
}

#[test]
fn grep_stops_when_cancelled() {
    let f = Fixture::new();
    f.write("a.txt", "x\n");
    f.cancel.cancel();
    let out = f.run(&Grep, json!({"pattern": "x"}));
    assert!(out.is_error && out.content.contains("cancelled"));
}
