//! End-to-end executor tests against a scripted local model server (test-only).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_core::config::Provider;
use harness_core::*;
use harness_engine::Engine;

/// Serves OpenAI-style SSE. `reply` maps the request body to (text, Option<(tool, args)>).
fn serve(
    reply: impl Fn(&str) -> (String, Option<(String, String)>) + Send + Sync + 'static,
) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let reply = Arc::new(reply);
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let reply = reply.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let body = String::from_utf8_lossy(&body).to_string();
                let (text, call) = reply(&body);
                let mut sse = String::new();
                let chunk = serde_json::json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]});
                sse.push_str(&format!("data: {chunk}\n\n"));
                if let Some((name, args)) = call {
                    let c = serde_json::json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":name,"arguments":args}}]},"finish_reason":null}]});
                    sse.push_str(&format!("data: {c}\n\n"));
                }
                let end = serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000,"completion_tokens":100}});
                sse.push_str(&format!("data: {end}\n\ndata: [DONE]\n\n"));
                let mut s = stream;
                let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", sse.len(), sse);
            });
        }
    });
    format!("http://{addr}/v1")
}

fn engine(url: &str) -> Engine {
    let store = Store::open_in_memory().unwrap();
    let config = Config {
        default_model: "t/test-sonnet".into(),
        fast_model: "t/test-haiku".into(),
        providers: vec![Provider {
            id: "t".into(),
            kind: "openai".into(),
            base_url: url.into(),
            api_key: "x".into(),
            api_key_env: String::new(),
        }],
        ..Config::default()
    };
    let e = Engine::new(store, config);
    e.start();
    e
}

fn wait_for(engine: &Engine, run_id: &str, want: Status) -> Run {
    let start = Instant::now();
    loop {
        let r = engine.store().run(run_id).unwrap();
        if r.status == want {
            return r;
        }
        assert!(
            r.status != Status::Failed || want == Status::Failed,
            "run failed: {:?}",
            r.error
        );
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timeout waiting for {want:?}; run is {:?} at {:?}",
            r.status,
            r.frontier
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn graph() -> PipelineGraph {
    let n = |id: &str, kind: NodeKind| Node {
        id: id.into(),
        name: id.into(),
        kind,
        x: 0.0,
        y: 0.0,
    };
    let e = |f: &str, t: &str, p: Option<&str>| Edge {
        from: f.into(),
        to: t.into(),
        port: p.map(String::from),
    };
    PipelineGraph {
        workdirs: vec![],
        nodes: vec![
            n("src", NodeKind::Source(SourceNode::default())),
            n(
                "ask",
                NodeKind::Decision(DecisionNode {
                    question: "needs design?".into(),
                    model: String::new(),
                    workdir: None,
                    max_steps: 4,
                }),
            ),
            n(
                "design",
                NodeKind::Agent(AgentNode {
                    variants: 2,
                    instructions: "DESIGN".into(),
                    ..Default::default()
                }),
            ),
            n("pick", NodeKind::Gate(GateNode::default())),
            n(
                "build",
                NodeKind::Agent(AgentNode {
                    instructions: "BUILD".into(),
                    ..Default::default()
                }),
            ),
        ],
        edges: vec![
            e("src", "ask", None),
            e("ask", "design", Some("yes")),
            e("ask", "build", Some("no")),
            e("design", "pick", None),
            e("pick", "build", None),
        ],
    }
}

fn pipeline(engine: &Engine) -> Pipeline {
    let p = Pipeline {
        id: new_id(),
        name: "t".into(),
        description: String::new(),
        color: 0,
        version: 1,
        draft: None,
        graph: graph(),
        archived: false,
        created_at: now(),
        updated_at: now(),
    };
    engine.store().put_pipeline(&p).unwrap();
    p
}

#[test]
fn template_agents_are_saved_and_referenced() {
    let e = engine("http://127.0.0.1:9/v1");
    let t = e
        .templates()
        .into_iter()
        .find(|t| t.id == "design-gpui")
        .unwrap();
    let p = e.create_pipeline(&t, "").unwrap();
    let agents = e.store().agents();
    assert_eq!(
        agents.len(),
        3,
        "{:?}",
        agents.iter().map(|a| &a.name).collect::<Vec<_>>()
    );
    let NodeKind::Agent(build) = &p.graph.node("build").unwrap().kind else {
        panic!()
    };
    assert!(agents.iter().any(|a| Some(&a.id) == build.agent.as_ref()));
    let coder = build
        .delegate
        .as_ref()
        .and_then(|d| d.agent.clone())
        .unwrap();
    assert_eq!(e.store().agent(&coder).unwrap().name, "Coder");
    // A second pipeline from the same template reuses the saved agents.
    e.create_pipeline(&t, "again").unwrap();
    assert_eq!(e.store().agents().len(), 3);
}

#[test]
fn adopt_agents_saves_inline_steps() {
    let e = engine("http://127.0.0.1:9/v1");
    let p = pipeline(&e);
    let names = e.adopt_agents(&p.id).unwrap();
    assert_eq!(names, vec!["design".to_string(), "build".to_string()]);
    let p = e.store().pipeline(&p.id).unwrap();
    assert_eq!(p.version, 2);
    for id in ["design", "build"] {
        let NodeKind::Agent(a) = &p.graph.node(id).unwrap().kind else {
            panic!()
        };
        assert!(e.store().agent(a.agent.as_ref().unwrap()).is_some());
    }
    assert!(e.adopt_agents(&p.id).unwrap().is_empty());
}

#[test]
fn no_branch_skips_design() {
    let url = serve(|body| {
        if body.contains("needs design?")
            && body.contains("\"answer\"")
            && !body.contains("recorded")
        {
            (
                "".into(),
                Some((
                    "answer".into(),
                    r#"{"yes":false,"reason":"logic only"}"#.into(),
                )),
            )
        } else {
            ("built it".into(), None)
        }
    });
    let e = engine(&url);
    let p = pipeline(&e);
    let r = e.start_run(&p.id, "fix the counter", None).unwrap();
    let r = wait_for(&e, &r.id, Status::Done);
    assert_eq!(r.outputs["ask"].port.as_deref(), Some("no"));
    assert!(!r.outputs.contains_key("design"));
    assert_eq!(r.outputs["build"].summary, "built it");
    assert!(r.cost_usd > 0.0);

    // A finished run can't be re-driven by retrying one of its steps.
    let build = e
        .store()
        .tasks_for_run(&r.id)
        .into_iter()
        .find(|t| t.node_id.as_deref() == Some("build"))
        .unwrap();
    let err = e.retry(&build.id).unwrap_err().to_string();
    assert!(err.contains("has finished"), "{err}");
    assert_eq!(e.store().run(&r.id).unwrap().status, Status::Done);
}

#[test]
fn yes_branch_fans_out_and_gate_send_back_then_pick() {
    let seen_note = Arc::new(Mutex::new(false));
    let sn = seen_note.clone();
    let url = serve(move |body| {
        if body.contains("needs design?")
            && body.contains("\"answer\"")
            && !body.contains("recorded")
        {
            return (
                "".into(),
                Some((
                    "answer".into(),
                    r#"{"yes":true,"reason":"new screen"}"#.into(),
                )),
            );
        }
        if body.contains("make it blue") {
            *sn.lock().unwrap() = true;
        }
        if body.contains("variant B") {
            ("design B".into(), None)
        } else if body.contains("variant A") {
            ("design A".into(), None)
        } else {
            ("built".into(), None)
        }
    });
    let e = engine(&url);
    let p = pipeline(&e);
    let r = e.start_run(&p.id, "new settings screen", None).unwrap();
    wait_for(&e, &r.id, Status::Waiting);
    let gate = e
        .store()
        .waiting()
        .into_iter()
        .find(|t| t.kind == TaskKind::Gate)
        .unwrap();
    let vs = e.store().variants(&gate.id);
    assert_eq!(vs.len(), 2);

    // Send back: design re-runs with the note, gate asks again.
    e.decide(
        &gate.id,
        DecisionKind::SendBack {
            note: "make it blue".into(),
            target: None,
        },
    )
    .unwrap();
    let start = Instant::now();
    let gate2 = loop {
        if let Some(t) = e
            .store()
            .waiting()
            .into_iter()
            .find(|t| t.kind == TaskKind::Gate && t.id != gate.id)
        {
            break t;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no second gate");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(*seen_note.lock().unwrap(), "note did not reach the model");
    e.decide(&gate2.id, DecisionKind::Pick { index: 1 })
        .unwrap();
    let r = wait_for(&e, &r.id, Status::Done);
    assert!(
        r.outputs["pick"].summary.starts_with("You picked B"),
        "{}",
        r.outputs["pick"].summary
    );
    assert!(e
        .store()
        .variants(&gate2.id)
        .iter()
        .any(|v| v.picked && v.index == 1));
}

#[test]
fn reject_cancels_and_restart_resumes_gate() {
    let url = serve(|body| {
        if body.contains("\"answer\"") && !body.contains("recorded") {
            return (
                "".into(),
                Some(("answer".into(), r#"{"yes":true,"reason":"ui"}"#.into())),
            );
        }
        ("ok".into(), None)
    });
    let e = engine(&url);
    let p = pipeline(&e);
    let r = e.start_run(&p.id, "x", None).unwrap();
    wait_for(&e, &r.id, Status::Waiting);
    // Simulate a restart: a fresh engine on the same store resumes the waiting run.
    let e2 = Engine::new(e.store().clone(), e.config());
    e2.start();
    let gate = e2
        .store()
        .waiting()
        .into_iter()
        .find(|t| t.kind == TaskKind::Gate)
        .unwrap();
    e2.decide(
        &gate.id,
        DecisionKind::Reject {
            reason: "no".into(),
        },
    )
    .unwrap();
    wait_for(&e2, &r.id, Status::Cancelled);
}

#[test]
fn manual_task_runs_to_done() {
    let url = serve(|_| ("All done, nothing to change.".into(), None));
    let e = engine(&url);
    let t = e
        .create_task(harness_engine::tasks::NewTask {
            title: "Say hi".into(),
            description: "Reply with a short report.".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.kind, TaskKind::Manual);
    let start = Instant::now();
    let done = loop {
        let t = e.store().task(&t.id).unwrap();
        if t.status == Status::Done || t.status == Status::Failed {
            break t;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timeout: {:?}",
            t.status
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(done.status, Status::Done, "error: {:?}", done.error);
    assert_eq!(done.summary, "All done, nothing to change.");
    assert!(done.finished_at.is_some());
    assert!(done.steps >= 1);
    // The task's thread holds your message and the reply.
    let msgs = e.store().messages(&t.id);
    assert_eq!(msgs[0].content, "Reply with a short report.");
    assert!(msgs.iter().any(|m| m.role == Role::Assistant));
}

#[test]
fn draft_description_returns_served_text() {
    let url = serve(|_| ("Goal: add dark mode\nDone when: it works".into(), None));
    let e = engine(&url);
    let mut streamed = String::new();
    let d = e
        .draft_description(
            harness_engine::tasks::DraftRequest {
                title: "Dark mode".into(),
                ..Default::default()
            },
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |t| streamed = t.to_string(),
        )
        .unwrap();
    assert_eq!(d.text, "Goal: add dark mode\nDone when: it works");
    assert_eq!(d.model, "t/test-haiku");
    assert_eq!(streamed, d.text);
    // Nothing to work from: error.
    assert!(e
        .draft_description(
            harness_engine::tasks::DraftRequest::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .is_err());
}

fn wait_task(e: &Engine, id: &str, want: Status) -> Task {
    let start = Instant::now();
    loop {
        let t = e.store().task(id).unwrap();
        if t.status == want {
            return t;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timeout waiting for {want:?}; task is {:?} ({:?})",
            t.status,
            t.error
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn instructions_after_a_pipeline_step_is_done_continue_its_thread() {
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = bodies.clone();
    let url = serve(move |body| {
        seen.lock().unwrap().push(body.to_string());
        if body.contains("needs design?")
            && body.contains("\"answer\"")
            && !body.contains("recorded")
        {
            (
                "".into(),
                Some((
                    "answer".into(),
                    r#"{"yes":false,"reason":"logic only"}"#.into(),
                )),
            )
        } else if body.contains("make the button blue") {
            ("Button is blue now.".into(), None)
        } else {
            ("built it".into(), None)
        }
    });
    let e = engine(&url);
    let p = pipeline(&e);
    let r = e.start_run(&p.id, "fix the counter", None).unwrap();
    let r = wait_for(&e, &r.id, Status::Done);
    let build = e
        .store()
        .tasks_for_run(&r.id)
        .into_iter()
        .find(|t| t.node_id.as_deref() == Some("build"))
        .unwrap();
    assert!(harness_engine::tasks::can_follow_up(e.store(), &build));
    let before = bodies.lock().unwrap().len();

    e.steer(&build.id, "make the button blue").unwrap();
    // Running again at once, so the UI shows the new turn.
    assert!(e.store().task(&build.id).unwrap().status.is_active());
    let done = wait_task(&e, &build.id, Status::Done);
    assert_eq!(done.summary, "Button is blue now.");
    assert!(done.finished_at.is_some());
    assert!(done.error.is_none());

    // The model saw the earlier report and your instruction, after the step's own prompt.
    let follow: Vec<String> = bodies.lock().unwrap()[before..].to_vec();
    assert_eq!(follow.len(), 1, "{follow:?}");
    let body: serde_json::Value = serde_json::from_str(&follow[0]).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"], "{roles:?}");
    assert!(msgs[1]["content"]
        .as_str()
        .unwrap()
        .contains("fix the counter"));
    assert_eq!(msgs[2]["content"], "built it");
    assert_eq!(msgs[3]["content"], "make the button blue");

    let r = e.store().run(&r.id).unwrap();
    assert_eq!(r.status, Status::Done);
    assert_eq!(r.outputs["build"].summary, "Button is blue now.");
    assert_eq!(
        r.outputs["build"].value["variants"][0]["summary"],
        "Button is blue now."
    );
    let msgs = e.store().messages(&build.id);
    assert!(msgs
        .iter()
        .any(|m| m.role == Role::User && m.content == "make the button blue"));

    // A step that is not an agent can't take instructions.
    let ask = e
        .store()
        .tasks_for_run(&r.id)
        .into_iter()
        .find(|t| t.node_id.as_deref() == Some("ask"))
        .unwrap();
    assert!(e.steer(&ask.id, "hello").is_err());
    // Blank messages do nothing.
    e.steer(&build.id, "   ").unwrap();
    assert_eq!(e.store().task(&build.id).unwrap().status, Status::Done);
}

#[test]
fn instructions_after_a_manual_task_is_done_start_another_turn() {
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = bodies.clone();
    let url = serve(move |body| {
        seen.lock().unwrap().push(body.to_string());
        if body.contains("shorter please") {
            ("Short report.".into(), None)
        } else {
            ("Long report about everything.".into(), None)
        }
    });
    let e = engine(&url);
    let t = e
        .create_task(harness_engine::tasks::NewTask {
            title: "Report".into(),
            description: "Write a report.".into(),
            ..Default::default()
        })
        .unwrap();
    wait_task(&e, &t.id, Status::Done);
    e.steer(&t.id, "shorter please").unwrap();
    let done = wait_task(&e, &t.id, Status::Done);
    assert_eq!(done.summary, "Short report.");

    let last = bodies.lock().unwrap().last().unwrap().clone();
    let body: serde_json::Value = serde_json::from_str(&last).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"], "{roles:?}");
    assert_eq!(msgs[1]["content"], "Write a report.");
    assert_eq!(msgs[2]["content"], "Long report about everything.");
    assert_eq!(msgs[3]["content"], "shorter please");
    let thread = e.store().messages(&t.id);
    assert_eq!(
        thread.iter().filter(|m| m.role == Role::User).count(),
        2,
        "{thread:?}"
    );
}

fn sh(cwd: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
fn instructions_to_a_designer_redraw_the_waiting_review() {
    let base = std::env::temp_dir().join(format!("pipeliner-flow-{}", std::process::id()));
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::env::set_var("PIPELINER_DATA_DIR", base.join("data"));
    sh(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "x").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-q", "-m", "init"]);

    // Requests in order: first turn (write, report), follow-up turn (write, report).
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let url = serve(move |_| {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let write = |color: &str| {
            (
                String::new(),
                Some((
                    "write_file".to_string(),
                    serde_json::json!({
                        "path": "design/home.html",
                        "content": format!("<html><body>{color}</body></html>")
                    })
                    .to_string(),
                )),
            )
        };
        match n {
            0 => write("RED"),
            1 => ("Red mockup.".into(), None),
            2 => write("GREEN"),
            _ => ("Green mockup.".into(), None),
        }
    });
    let e = engine(&url);
    let node = |id: &str, kind: NodeKind| Node {
        id: id.into(),
        name: id.into(),
        kind,
        x: 0.0,
        y: 0.0,
    };
    let edge = |f: &str, t: &str| Edge {
        from: f.into(),
        to: t.into(),
        port: None,
    };
    let graph = PipelineGraph {
        workdirs: vec![Workdir {
            id: "code".into(),
            name: "Code".into(),
            path: repo.display().to_string(),
            worktree: false,
            base_branch: "main".into(),
            branch_prefix: String::new(),
            setup: String::new(),
            shell: String::new(),
            env: Default::default(),
        }],
        nodes: vec![
            node("src", NodeKind::Source(SourceNode::default())),
            node(
                "design",
                NodeKind::Agent(AgentNode {
                    instructions: "DESIGN".into(),
                    workdir: Some("code".into()),
                    ..Default::default()
                }),
            ),
            node(
                "render",
                NodeKind::Render(RenderNode {
                    workdir: Some("code".into()),
                    // Copies the mockup, so the test needs no browser.
                    command: "cp {in} {out}".into(),
                    ..Default::default()
                }),
            ),
            node("pick", NodeKind::Gate(GateNode::default())),
        ],
        edges: vec![
            edge("src", "design"),
            edge("design", "render"),
            edge("render", "pick"),
        ],
    };
    let p = Pipeline {
        id: new_id(),
        name: "t".into(),
        description: String::new(),
        color: 0,
        version: 1,
        draft: None,
        graph,
        archived: false,
        created_at: now(),
        updated_at: now(),
    };
    e.store().put_pipeline(&p).unwrap();
    let r = e.start_run(&p.id, "design the home screen", None).unwrap();
    let r = wait_for(&e, &r.id, Status::Waiting);
    let designer = e
        .store()
        .tasks_for_run(&r.id)
        .into_iter()
        .find(|t| t.node_id.as_deref() == Some("design"))
        .unwrap();
    let gate = e
        .store()
        .tasks_for_run(&r.id)
        .into_iter()
        .find(|t| t.kind == TaskKind::Gate)
        .unwrap();
    let before = e.store().variants(&gate.id);
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].summary, "Red mockup.");
    assert_eq!(before[0].images.len(), 1, "{:?}", before[0]);
    let old_png = std::path::PathBuf::from(&before[0].images[0]);
    assert!(std::fs::read_to_string(&old_png).unwrap().contains("RED"));

    e.steer(&designer.id, "make it green").unwrap();
    wait_task(&e, &designer.id, Status::Done);
    let start = Instant::now();
    let after = loop {
        let vs = e.store().variants(&gate.id);
        if vs[0].summary == "Green mockup." {
            break vs;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "{:?}", vs[0]);
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(after[0].images.len(), 1, "{:?}", after[0]);
    let new_png = std::path::PathBuf::from(&after[0].images[0]);
    assert_ne!(new_png, old_png);
    assert!(std::fs::read_to_string(&new_png).unwrap().contains("GREEN"));
    // The old picture stays on disk for anything still showing it.
    assert!(old_png.exists());

    // The run's own record of the render step moved too.
    let run = e.store().run(&r.id).unwrap();
    let v = &run.outputs["render"].value["variants"][0];
    assert_eq!(v["summary"], "Green mockup.");
    assert_eq!(v["images"][0], after[0].images[0].as_str());
    let _ = std::fs::remove_dir_all(&base);
}
