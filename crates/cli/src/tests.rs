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

#[test]
fn run_sweep_reports_what_would_go() {
    let temp = Temp::new();
    let data = temp.0.join("data").to_string_lossy().into_owned();
    let (code, out, err) = cli(&["--data-dir", &data, "run", "sweep", "--dry-run"], "", &temp.0, Vec::new);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "nothing to remove");
    assert_eq!(crate::args_work::parse_run(&["sweep"]).unwrap(), crate::args::RunCmd::Sweep { dry_run: false });
    assert!(crate::args_work::parse_run(&["sweep", "extra"]).is_err());
}

/// Runs git in `dir` with an isolated config.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Issue source → coder → review → merge, in `repo`.
fn review_graph(repo: &Path) -> engine::api::GraphDef {
    use engine::api::*;
    let node = |id: &str, kind: NodeKind| NodeDef { id: id.into(), name: id.to_uppercase(), pos: Default::default(), kind };
    let route = |from: &str, port: &str, to: &str| EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind: EdgeKind::Route };
    GraphDef {
        schema: 1,
        id: PipelineId::new("review-flow"),
        name: "Review flow".into(),
        description: String::new(),
        limits: Default::default(),
        workdirs: vec![WorkdirDef {
            id: "code".into(),
            name: "Code".into(),
            path: repo.to_path_buf(),
            worktree: true,
            base_branch: "main".into(),
            ..Default::default()
        }],
        nodes: vec![
            node("source", NodeKind::Source(SourceNode::default())),
            node(
                "coder",
                NodeKind::Agent(AgentNode {
                    role: "You code.".into(),
                    model: Some(ModelRef::parse("scripted/scripted-model")),
                    workdir: "code".into(),
                    variants: 1,
                    ..Default::default()
                }),
            ),
            node("review", NodeKind::Review(ReviewNode { send_back_to: Some("coder".into()), skip_if_empty: false })),
            node("merge", NodeKind::Action(ActionNode::Merge { workdir: "code".into() })),
        ],
        edges: vec![route("source", "out", "coder"), route("coder", "out", "review"), route("review", "approved", "merge")],
    }
}

fn coder_writes() -> Vec<Reply> {
    vec![
        Reply::tool("w1", "write_file", json!({"path": "fix.txt", "content": "fixed\n"})),
        Reply::tool("f1", "step_done", json!({"summary": "Wrote fix.txt."})),
    ]
}

#[test]
fn an_issue_is_filed_started_reviewed_and_shipped_from_the_command_line() {
    use engine::api::CatalogApi;
    let temp = Temp::new();
    let repo = temp.0.join("app");
    std::fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    let data = temp.0.join("data");
    {
        let engine = Engine::open(base::paths::Paths::under(&data), Arc::new(ScriptedLlm::new()), Arc::new(SystemClock)).expect("engine");
        engine.save_pipeline(&engine::api::Place::Library, review_graph(&repo)).expect("save");
        engine::api::OpsApi::shutdown(&engine);
    }
    let data = data.to_string_lossy().into_owned();
    let d = |rest: &[&str]| -> Vec<String> { ["--data-dir", data.as_str()].iter().chain(rest).map(|s| s.to_string()).collect() };
    let call = |rest: &[&str], replies: fn() -> Vec<Reply>| {
        let argv = d(rest);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        cli(&argv, "", &temp.0, replies)
    };

    let (code, out, err) = call(&["pipeline", "problems", "review-flow"], Vec::new);
    assert_eq!(code, 0, "{out}{err}");
    let (code, out, err) = call(&["pipeline", "deploy", "review-flow", "--note", "first"], Vec::new);
    assert_eq!((code, out.trim()), (0, "review-flow v1 is live"), "{err}");
    let (code, out, err) = call(&["issue", "new", "Fix", "it", "--pipeline", "review-flow", "--body", "Please."], Vec::new);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "HAR-1 Fix it");

    let (code, out, err) = call(&["issue", "start", "HAR-1"], coder_writes);
    assert_eq!(code, 0, "out: {out}\nerr: {err}");
    assert!(out.contains("waiting for review"), "{out}");
    let review = out.lines().find_map(|l| l.strip_prefix("review ")).and_then(|l| l.split_whitespace().next()).expect("review id").to_string();

    let (code, out, _) = call(&["review", "show", &review], Vec::new);
    assert_eq!(code, 0);
    assert!(out.contains("fix.txt"), "{out}");
    let (code, out, _) = call(&["review", "list"], Vec::new);
    assert!(code == 0 && out.contains(&review), "{out}");

    let (code, out, err) = call(&["review", "ship", &review], Vec::new);
    assert_eq!(code, 0, "out: {out}\nerr: {err}");
    assert!(out.contains("· done"), "{out}");
    assert_eq!(git(&repo, &["show", "main:fix.txt"]), "fixed\n");

    let (code, out, _) = call(&["run", "list"], Vec::new);
    assert!(code == 0 && out.contains("done") && out.contains("HAR-1"), "{out}");
    let (code, out, _) = call(&["issue", "show", "HAR-1"], Vec::new);
    assert!(code == 0 && out.contains("Done"), "{out}");
    let (code, _, err) = call(&["run", "show", "nope"], Vec::new);
    assert_eq!(code, 1);
    assert!(err.contains("there is no run nope"), "{err}");
}
