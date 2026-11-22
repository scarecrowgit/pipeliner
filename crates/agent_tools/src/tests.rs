use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::{DiffKind, Permission, ToolDisplay, ToolOutput, Workspace};

/// Temp workspace removed on drop.
struct TempWs {
    dir: PathBuf,
    ws: Workspace,
}

impl TempWs {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agent_tools_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { ws: Workspace::new(&dir), dir }
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap()
    }

    fn run(&self, tool: &str, input: serde_json::Value) -> ToolOutput {
        self.ws.execute(tool, &input, Arc::new(AtomicBool::new(false)))
    }
}

impl Drop for TempWs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn specs_cover_all_tools() {
    let t = TempWs::new();
    let names: Vec<String> = t.ws.tool_specs().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["read_file", "list_dir", "grep", "edit_file", "write_file", "bash"]);
    for s in t.ws.tool_specs() {
        assert_eq!(s.input_schema["type"], "object");
        assert!(!s.description.is_empty());
    }
    assert_eq!(t.ws.permission("grep"), Permission::Auto);
    assert_eq!(t.ws.permission("bash"), Permission::Ask);
}

#[test]
fn path_escapes_rejected() {
    let t = TempWs::new();
    t.write("a.txt", "hi\n");
    for bad in ["../x", "sub/../../x", "/etc/passwd", "./../a.txt"] {
        let out = t.run("read_file", json!({ "path": bad }));
        assert!(out.is_error, "{bad} should be rejected");
        assert!(matches!(out.display, ToolDisplay::Error { .. }));
        let out = t.run("write_file", json!({ "path": bad, "content": "x" }));
        assert!(out.is_error, "write {bad} should be rejected");
    }
    // `..` that stays inside is fine; absolute path inside root is fine.
    assert!(!t.run("read_file", json!({ "path": "sub/../a.txt" })).is_error);
    let abs = t.dir.join("a.txt");
    assert!(!t.run("read_file", json!({ "path": abs.to_str().unwrap() })).is_error);
}

#[cfg(unix)]
#[test]
fn symlink_outside_rejected() {
    let t = TempWs::new();
    let outside = TempWs::new();
    outside.write("secret.txt", "secret\n");
    std::os::unix::fs::symlink(&outside.dir, t.dir.join("link")).unwrap();
    std::os::unix::fs::symlink(outside.dir.join("secret.txt"), t.dir.join("f")).unwrap();
    assert!(t.run("read_file", json!({ "path": "link/secret.txt" })).is_error);
    assert!(t.run("read_file", json!({ "path": "f" })).is_error);
    assert!(t.run("write_file", json!({ "path": "link/new.txt", "content": "x" })).is_error);
    assert!(!outside.dir.join("new.txt").exists());
    // Dangling symlink: can't be used to create a file outside.
    std::os::unix::fs::symlink(outside.dir.join("nope.txt"), t.dir.join("dangle")).unwrap();
    assert!(t.run("write_file", json!({ "path": "dangle", "content": "x" })).is_error);
    assert!(!outside.dir.join("nope.txt").exists());
    // Symlink inside the root is fine.
    t.write("real.txt", "ok\n");
    std::os::unix::fs::symlink(t.dir.join("real.txt"), t.dir.join("alias")).unwrap();
    assert!(!t.run("read_file", json!({ "path": "alias" })).is_error);
}

#[test]
fn read_offset_limit() {
    let t = TempWs::new();
    let content: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    t.write("f.txt", &content);
    let out = t.run("read_file", json!({ "path": "f.txt", "offset": 3, "limit": 2 }));
    assert!(!out.is_error);
    assert!(out.content.starts_with("     3\tline 3\n     4\tline 4\n"), "{}", out.content);
    assert!(out.content.contains("Showing lines 3-4 of 10"));
    assert!(out.content.contains("offset=5"));
    assert_eq!(out.display, ToolDisplay::Read { path: "f.txt".into(), lines: 2 });

    let out = t.run("read_file", json!({ "path": "f.txt" }));
    assert_eq!(out.content.lines().count(), 10);
    assert!(!out.content.contains("Showing"));

    assert!(t.run("read_file", json!({ "path": "f.txt", "offset": 50 })).is_error);
    assert!(t.run("read_file", json!({ "path": "missing.txt" })).is_error);
    assert!(t.run("read_file", json!({})).is_error);
    assert!(t.run("read_file", json!({ "path": 5 })).is_error);
}

#[test]
fn read_refuses_binary() {
    let t = TempWs::new();
    std::fs::write(t.dir.join("b.bin"), [1u8, 2, 0, 3]).unwrap();
    let out = t.run("read_file", json!({ "path": "b.bin" }));
    assert!(out.is_error);
    assert!(out.content.contains("binary"));
}

#[test]
fn list_dir_sorted_and_skips() {
    let t = TempWs::new();
    t.write("b.txt", "");
    t.write("a/x.txt", "");
    t.write("target/out", "");
    t.write("node_modules/m/i.js", "");
    t.write(".git/HEAD", "ref: refs/heads/main\n");
    let out = t.run("list_dir", json!({}));
    assert_eq!(out.content, "a/\nb.txt\n");
    assert_eq!(out.display, ToolDisplay::List { path: ".".into(), entries: 2 });
    let out = t.run("list_dir", json!({ "path": "a" }));
    assert_eq!(out.content, "x.txt\n");
    assert!(t.run("list_dir", json!({ "path": "b.txt" })).is_error);
}

#[test]
fn grep_respects_gitignore_and_glob() {
    let t = TempWs::new();
    t.write(".gitignore", "ignored/\n*.log\n");
    t.write("src/main.rs", "fn main() {\n    hello();\n}\n");
    t.write("src/lib.rs", "pub fn hello() {}\n");
    t.write("notes.md", "hello world\n");
    t.write("ignored/x.rs", "hello\n");
    t.write("debug.log", "hello\n");
    t.write(".hidden/h.rs", "hello\n");

    let out = t.run("grep", json!({ "pattern": "hello" }));
    assert!(!out.is_error);
    assert!(out.content.contains("src/main.rs:2:     hello();"), "{}", out.content);
    assert!(out.content.contains("src/lib.rs:1: pub fn hello() {}"));
    assert!(out.content.contains("notes.md:1: hello world"));
    assert!(!out.content.contains("ignored/"));
    assert!(!out.content.contains("debug.log"));
    assert!(!out.content.contains(".hidden"));
    assert_eq!(out.display, ToolDisplay::Grep { pattern: "hello".into(), matches: 3, files: 3 });

    let out = t.run("grep", json!({ "pattern": "hello", "glob": "*.rs" }));
    assert_eq!(out.display, ToolDisplay::Grep { pattern: "hello".into(), matches: 2, files: 2 });
    let out = t.run("grep", json!({ "pattern": "hel+o", "path": "src/lib.rs" }));
    assert_eq!(out.display, ToolDisplay::Grep { pattern: "hel+o".into(), matches: 1, files: 1 });

    let out = t.run("grep", json!({ "pattern": "nothing_here" }));
    assert!(!out.is_error);
    assert!(out.content.contains("No matches"));
    assert!(t.run("grep", json!({ "pattern": "(" })).is_error);

    let cancel = Arc::new(AtomicBool::new(true));
    let out = t.ws.execute("grep", &json!({ "pattern": "hello" }), cancel);
    assert!(out.is_error);
    assert!(out.content.contains("cancelled"));
}

#[test]
fn grep_caps_matches() {
    let t = TempWs::new();
    t.write("many.txt", &"x\n".repeat(500));
    let out = t.run("grep", json!({ "pattern": "x" }));
    assert!(matches!(out.display, ToolDisplay::Grep { matches: 200, files: 1, .. }));
    assert!(out.content.contains("stopped at 200"));
}

#[test]
fn edit_unique_nonunique_notfound() {
    let t = TempWs::new();
    t.write("f.rs", "fn a() {}\nfn b() {}\nfn a() {}\n");
    let out = t.run("edit_file", json!({ "path": "f.rs", "old_string": "fn a() {}", "new_string": "x" }));
    assert!(out.is_error);
    assert!(out.content.contains("2 times"), "{}", out.content);
    let out = t.run("edit_file", json!({ "path": "f.rs", "old_string": "fn c()", "new_string": "x" }));
    assert!(out.is_error);
    assert!(out.content.contains("not found"));
    assert!(t.run("edit_file", json!({ "path": "nope.rs", "old_string": "a", "new_string": "b" })).is_error);
    assert!(t.run("edit_file", json!({ "path": "f.rs", "old_string": "", "new_string": "b" })).is_error);
    assert!(t.run("edit_file", json!({ "path": "f.rs", "old_string": "fn b" })).is_error);
    assert_eq!(t.read("f.rs"), "fn a() {}\nfn b() {}\nfn a() {}\n");

    let input = json!({ "path": "f.rs", "old_string": "fn b() {}", "new_string": "fn bb() {}" });
    let preview = t.ws.preview("edit_file", &input);
    assert_eq!(t.read("f.rs"), "fn a() {}\nfn b() {}\nfn a() {}\n", "preview must not write");
    let out = t.run("edit_file", input);
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.display, preview);
    assert_eq!(t.read("f.rs"), "fn a() {}\nfn bb() {}\nfn a() {}\n");
    let ToolDisplay::Diff { path, added, removed, lines, created } = out.display else { panic!() };
    assert_eq!((path.as_str(), added, removed, created), ("f.rs", 1, 1, false));
    assert_eq!(crate::diff::render(&lines), " 1 fn a() {}\n-2 fn b() {}\n+2 fn bb() {}\n 3 fn a() {}");
}

#[test]
fn edit_preserves_crlf() {
    let t = TempWs::new();
    t.write("w.txt", "one\r\ntwo\r\nthree\r\n");
    let out = t.run("edit_file", json!({ "path": "w.txt", "old_string": "one\ntwo", "new_string": "1\n2\n2.5" }));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(t.read("w.txt"), "1\r\n2\r\n2.5\r\nthree\r\n");
}

#[test]
fn diff_hunk_line_numbers() {
    let t = TempWs::new();
    let content: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    t.write("big.txt", &content);
    let out = t.run("edit_file", json!({ "path": "big.txt", "old_string": "l20\n", "new_string": "l20\nnew\n" }));
    let ToolDisplay::Diff { lines, added, removed, .. } = out.display else { panic!("{:?}", out.display) };
    assert_eq!((added, removed), (1, 0));
    let kinds: Vec<(DiffKind, usize)> = lines.iter().map(|l| (l.kind.clone(), l.line_no)).collect();
    assert_eq!(
        kinds,
        vec![
            (DiffKind::Context, 18),
            (DiffKind::Context, 19),
            (DiffKind::Context, 20),
            (DiffKind::Add, 21),
            (DiffKind::Context, 22),
            (DiffKind::Context, 23),
            (DiffKind::Context, 24),
        ]
    );
    assert_eq!(lines[3].text, "new");
}

#[test]
fn write_new_and_overwrite() {
    let t = TempWs::new();
    let input = json!({ "path": "deep/dir/new.txt", "content": "a\nb\n" });
    let preview = t.ws.preview("write_file", &input);
    assert!(!t.dir.join("deep").exists(), "preview must not write");
    let out = t.run("write_file", input);
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.display, preview);
    assert_eq!(t.read("deep/dir/new.txt"), "a\nb\n");
    assert!(matches!(out.display, ToolDisplay::Diff { created: true, added: 2, removed: 0, .. }));

    let out = t.run("write_file", json!({ "path": "deep/dir/new.txt", "content": "a\nc\n" }));
    assert!(matches!(out.display, ToolDisplay::Diff { created: false, added: 1, removed: 1, .. }));
    assert_eq!(t.read("deep/dir/new.txt"), "a\nc\n");
    assert!(t.run("write_file", json!({ "path": "deep", "content": "x" })).is_error);
}

#[test]
fn bash_output_and_exit_code() {
    let t = TempWs::new();
    t.write("marker.txt", "");
    let out = t.run("bash", json!({ "command": "ls; echo err >&2; exit 3" }));
    assert!(out.is_error);
    let ToolDisplay::Bash { output, exit_code, timed_out, .. } = &out.display else { panic!() };
    assert!(output.contains("marker.txt"));
    assert!(output.contains("err"));
    assert_eq!(*exit_code, Some(3));
    assert!(!timed_out);
    assert!(out.content.contains("[exit code: 3]"));

    let out = t.run("bash", json!({ "command": "printf hi" }));
    assert!(!out.is_error);
    assert!(matches!(&out.display, ToolDisplay::Bash { exit_code: Some(0), output, .. } if output == "hi"));

    assert!(t.run("bash", json!({})).is_error);
    let p = t.ws.preview("bash", &json!({ "command": "rm -rf x" }));
    assert!(matches!(p, ToolDisplay::Bash { ref command, exit_code: None, .. } if command == "rm -rf x"));
}

#[test]
fn bash_caps_output() {
    let t = TempWs::new();
    let out = t.run("bash", json!({ "command": "yes abcdefghij | head -c 200000; echo; echo THE_END" }));
    let ToolDisplay::Bash { output, .. } = &out.display else { panic!() };
    assert!(output.len() < 32 * 1024, "{}", output.len());
    assert!(output.contains("truncated"));
    assert!(output.trim_end().ends_with("THE_END"));
    assert!(output.starts_with("abcdefghij"));
}

#[test]
fn bash_timeout_kills() {
    let t = TempWs::new();
    let start = Instant::now();
    let out = t.run("bash", json!({ "command": "echo start; sleep 30; echo never", "timeout_secs": 1 }));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(out.is_error);
    let ToolDisplay::Bash { output, timed_out, .. } = &out.display else { panic!() };
    assert!(*timed_out);
    assert!(output.contains("start"));
    assert!(!output.contains("never"));
    assert!(out.content.contains("timed out"));
}

#[test]
fn bash_cancel_kills() {
    let t = TempWs::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c2.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    let out = t.ws.execute("bash", &json!({ "command": "sleep 30" }), cancel);
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(out.is_error);
    assert!(out.content.contains("cancelled"));
}

#[test]
fn unknown_tool_and_bad_input() {
    let t = TempWs::new();
    assert!(t.run("frobnicate", json!({})).is_error);
    assert!(matches!(t.ws.preview("frobnicate", &json!({})), ToolDisplay::Error { .. }));
    assert!(t.run("read_file", json!(null)).is_error);
    assert!(matches!(t.ws.preview("edit_file", &json!({ "path": "x" })), ToolDisplay::Error { .. }));
}

fn set_head(dir: &Path, head: &str) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), head).unwrap();
}

#[test]
fn context_files_and_label() {
    let t = TempWs::new();
    set_head(&t.dir, "ref: refs/heads/feature/x\n");
    t.write(".gitignore", "build/\n");
    t.write("src/a.rs", "");
    t.write("b.txt", "");
    t.write("build/out.o", "");
    t.write("target/debug/x", "");
    t.write("AGENTS.md", &format!("Be nice.\n{}", "z".repeat(20_000)));

    let (name, branch) = t.ws.repo_label();
    assert_eq!(name, t.dir.file_name().unwrap().to_string_lossy());
    assert_eq!(branch.as_deref(), Some("feature/x"));

    assert_eq!(t.ws.files(100), vec!["AGENTS.md", "b.txt", "src/a.rs"]);
    assert_eq!(t.ws.files(2), vec!["AGENTS.md", "b.txt"]);

    let ctx = t.ws.context_summary();
    assert!(ctx.contains("Git branch: feature/x"));
    assert!(ctx.contains("  src/\n"));
    assert!(ctx.contains("Be nice."));
    assert!(ctx.contains("AGENTS.md truncated"));
    assert!(ctx.len() < 10_000);

    set_head(&t.dir, "0123456789abcdef0123456789abcdef01234567\n");
    assert_eq!(t.ws.repo_label().1.as_deref(), Some("0123456"));
}
