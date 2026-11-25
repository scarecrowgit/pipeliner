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
    Engine::new(store, config)
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
