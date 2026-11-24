//! The commands end to end against an engine on a scripted model, in a temp directory.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base::time::SystemClock;
use engine::Engine;
use engine::testing::{Reply, ScriptedLlm};
use serde_json::json;

use crate::app::{Deps, run};

/// A temp directory removed on drop.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("harness-cli-test-{}", base::id::new_id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir.canonicalize().expect("canonical"))
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Buf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("buf")).into_owned()
    }
}

impl Write for Buf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("buf").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs the CLI with `input` typed, on a model that answers with `replies`.
fn cli(argv: &[&str], input: &str, cwd: &Path, replies: fn() -> Vec<Reply>) -> (i32, String, String) {
    let (out, err) = (Buf::default(), Buf::default());
    let deps = Deps {
        open: Box::new(move |paths| {
            let llm = ScriptedLlm::new().replies(replies());
            Engine::open(paths, Arc::new(llm), Arc::new(SystemClock))
        }),
        input: Box::new(Cursor::new(input.to_string().into_bytes())),
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        cwd: cwd.to_path_buf(),
        env: Box::new(|_| None),
    };
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let code = run(&argv, deps);
    (code, out.text(), err.text())
}

fn write_note() -> Vec<Reply> {
    vec![
        Reply::tool("t1", "write_file", json!({"path": "note.txt", "content": "hi\n"})),
        Reply::text("Wrote note.txt."),
    ]
}

fn write_note_refused() -> Vec<Reply> {
    vec![
        Reply::tool("t1", "write_file", json!({"path": "note.txt", "content": "hi\n"})),
        Reply::text("Okay, not writing it."),
    ]
}

#[test]
fn a_one_shot_chat_streams_the_answer_and_runs_tools_with_yes() {
    let temp = Temp::new();
    let (data, work) = (temp.0.join("data"), temp.0.join("work"));
    std::fs::create_dir_all(&work).expect("work");
    let data = data.to_string_lossy().into_owned();
    let argv = ["--data-dir", &data, "chat", "--yes", "--dir", "work", "Write a note"];
    let (code, out, err) = cli(&argv, "", &temp.0, write_note);

    assert_eq!(code, 0, "out: {out}\nerr: {err}");
    assert_eq!(std::fs::read_to_string(work.join("note.txt")).expect("note"), "hi\n");
    assert!(out.contains("→ ") && out.contains("note.txt"), "{out}");
    assert!(out.contains("✓"), "{out}");
    assert!(out.contains("Wrote note.txt."), "{out}");
    assert!(err.starts_with("chat "), "{err}");
    assert!(!err.contains("Allow?"));
}

#[test]
fn the_loop_asks_for_permission_on_the_terminal_and_sessions_lists_the_chat() {
    let temp = Temp::new();
    let data = temp.0.join("data").to_string_lossy().into_owned();
    let (code, out, err) = cli(&["--data-dir", &data, "chat"], "Write a note\nn\nquit\n", &temp.0, write_note_refused);

    assert_eq!(code, 0, "out: {out}\nerr: {err}");
    assert!(err.contains("Allow? [y]es / [n]o / [a]lways"), "{err}");
    assert!(out.contains("✗"), "the refused call shows as failed: {out}");
    assert!(out.contains("Okay, not writing it."), "{out}");
    assert!(!temp.0.join("note.txt").exists());

    let (code, out, _) = cli(&["--data-dir", &data, "sessions"], "", &temp.0, Vec::new);
    assert_eq!(code, 0);
    assert!(out.contains("Write a note") && out.contains("idle"), "{out}");
    let (code, out, _) = cli(&["--data-dir", &data, "models"], "", &temp.0, Vec::new);
    assert_eq!(code, 0);
    assert!(out.contains("scripted/scripted-model"), "{out}");
    let (code, out, _) = cli(&["--data-dir", &data, "import", "--dry-run"], "", &temp.0, Vec::new);
    assert_eq!(code, 0);
    assert!(out.contains("dry run"), "{out}");
}

#[test]
fn help_and_mistakes() {
    let temp = Temp::new();
    let (code, out, _) = cli(&["help"], "", &temp.0, Vec::new);
    assert_eq!(code, 0);
    assert!(out.contains("Usage: harness-cli"));
    let (code, _, err) = cli(&["chat", "--bogus"], "", &temp.0, Vec::new);
    assert_eq!(code, 2);
    assert!(err.contains("unknown option --bogus"));
}
