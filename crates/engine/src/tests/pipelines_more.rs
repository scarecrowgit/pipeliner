//! Pipelines: variants with send-back, questions, cancel, restart and resume, deploy checks
//! and the daily limit.

use std::time::Duration;

use llm::Reply;
use model::graph::{PORT_APPROVED, PORT_OUT};
use model::{
    ActionNode, AgentNode, EdgeDef, EdgeKind, GraphDef, InboxKind, Limits, NodeDef, NodeKind, PipelineId, ReviewDecision,
    ReviewNode, RunState, SourceKind, SourceNode, StartFrom, WorkdirDef,
};
use serde_json::json;

use super::pipelines::{deploy, pipeline_fx, scripted, wait_run};
use super::git;
use crate::api::{CatalogApi, InboxApi, OpsApi, RunApi, RunInput};

pub(super) fn node(id: &str, kind: NodeKind) -> NodeDef {
    NodeDef { id: id.into(), name: id.to_uppercase(), pos: Default::default(), kind }
}

pub(super) fn route(from: &str, port: &str, to: &str) -> EdgeDef {
    EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind: EdgeKind::Route }
}

/// Manual source → coder (×`variants`) → review → merge, on `repo`.
pub(super) fn graph(repo: &std::path::Path, variants: u32) -> GraphDef {
    GraphDef {
        schema: 1,
        id: PipelineId::new("variants"),
        name: "Variants".into(),
        description: String::new(),
        limits: Limits { max_parallel: 1, ..Default::default() },
        workdirs: vec![WorkdirDef {
            id: "code".into(),
            name: "Code".into(),
            path: repo.to_path_buf(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: "try/".into(),
            ..Default::default()
        }],
        nodes: vec![
            node("source", NodeKind::Source(SourceNode { source: SourceKind::Manual })),
            node(
                "coder",
                NodeKind::Agent(AgentNode {
                    role: "You code.".into(),
                    model: Some(scripted()),
                    workdir: "code".into(),
                    variants,
                    ..Default::default()
                }),
            ),
            node("review", NodeKind::Review(ReviewNode { send_back_to: Some("coder".into()), skip_if_empty: false })),
            node("merge", NodeKind::Action(ActionNode::Merge { workdir: "code".into() })),
        ],
        edges: vec![
            route("source", PORT_OUT, "coder"),
            route("coder", PORT_OUT, "review"),
            route("review", PORT_APPROVED, "merge"),
            EdgeDef { from: "review".into(), port: PORT_OUT.into(), to: "coder".into(), kind: EdgeKind::Back },
        ],
    }
}

pub(super) fn input() -> RunInput {
    RunInput { title: "Shorter greeting".into(), text: "Make the greeting shorter.".into() }
}

fn write_and_finish(llm: &llm::ScriptedLlm, n: u32, text: &str) {
    llm.push(Reply::tool(format!("w{n}"), "write_file", json!({"path": "hello.txt", "content": text})));
    llm.push(Reply::tool(format!("f{n}"), "step_done", json!({"summary": format!("Wrote {}", text.trim())})));
}

#[test]
fn two_variants_go_through_review_with_a_send_back_and_ship() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello world\n")]);
    deploy(&fx.engine, &graph(&repo, 2));
    write_and_finish(&fx.llm, 1, "hi world\n");
    write_and_finish(&fx.llm, 2, "hey\n");
    let run = fx.engine.start_run(&PipelineId::new("variants"), input()).expect("start");
    let waiting = wait_run(&fx.engine, &run.id, "in review", |r| r.state == RunState::WaitingReview || !r.state.is_open());
    assert_eq!(waiting.state, RunState::WaitingReview, "{waiting:#?}");
    let review = fx.engine.open_reviews().unwrap().into_iter().find(|r| r.run == run.id).expect("review");
    assert_eq!(review.variants.len(), 2);
    let branches: Vec<_> = review.variants.iter().map(|v| v.branch.clone().unwrap_or_default()).collect();
    assert_eq!(branches, ["try/".to_string() + &base::id::slug(run.id.as_str(), 40) + "-a", "try/".to_string() + &base::id::slug(run.id.as_str(), 40) + "-b"]);
    assert!(review.variants.iter().all(|v| v.files.iter().any(|f| f.path == "hello.txt")));
    let diff = fx.engine.variant_diff(&review.id, 2, "hello.txt").expect("diff");
    assert!(format!("{diff:?}").contains("hey"));
    let estimate = fx.engine.send_back_estimate(&review.id, StartFrom::Variant { index: 2 }, 1).expect("estimate");
    assert!(estimate.cost_usd >= 0.0);

    write_and_finish(&fx.llm, 3, "hey there\n");
    let decision = ReviewDecision::SendBack { note: "Add a word".into(), start: StartFrom::Variant { index: 2 }, variants: 1 };
    fx.engine.decide_review(&review.id, decision).expect("send back");
    let again = wait_run(&fx.engine, &run.id, "in review again", |r| {
        r.nodes.iter().any(|n| n.node.as_str() == "review" && n.attempt == 2 && n.status == model::NodeStatus::Waiting)
            || !r.state.is_open()
    });
    assert_eq!(again.state, RunState::WaitingReview, "{again:#?}");
    let second = fx.engine.open_reviews().unwrap().into_iter().find(|r| r.run == run.id).expect("second review");
    assert_eq!(second.variants.len(), 1);
    assert_eq!(second.feedback.len(), 1);
    let requests = fx.llm.requests();
    assert!(requests.iter().any(|r| format!("{r:?}").contains("Add a word")), "the note reached the agent");

    fx.engine.decide_review(&second.id, ReviewDecision::Ship { variant: second.variants[0].index }).expect("ship");
    let done = wait_run(&fx.engine, &run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
    assert_eq!(git(&repo, &["show", "main:hello.txt"]), "hey there\n");
    let inbox = fx.engine.inbox(false).unwrap();
    assert!(!inbox.iter().any(|i| matches!(i.kind, InboxKind::Review { .. })), "{inbox:#?}");
}

#[test]
fn a_question_waits_for_the_answer_in_the_inbox() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.nodes.truncate(2);
    g.edges.truncate(1);
    deploy(&fx.engine, &g);
    fx.llm.push(Reply::tool("a1", "ask_user", json!({"question": "Which greeting?"})));
    let run = fx.engine.start_run(&g.id, input()).expect("start");
    let waiting = wait_run(&fx.engine, &run.id, "asking", |r| r.state == RunState::WaitingAnswer || !r.state.is_open());
    assert_eq!(waiting.state, RunState::WaitingAnswer, "{waiting:#?}");
    let coder = waiting.nodes.iter().find(|n| n.node.as_str() == "coder").unwrap();
    assert_eq!(coder.question.as_deref(), Some("Which greeting?"));
    let inbox = fx.engine.inbox(false).unwrap();
    assert!(inbox.iter().any(|i| matches!(i.kind, InboxKind::Question { .. }) && i.title == "Which greeting?"), "{inbox:#?}");
    assert!(fx.engine.needs_you().unwrap().iter().any(|n| n.kind == crate::api::NeedKind::Question));

    fx.llm.push(Reply::tool("a2", "step_done", json!({"summary": "Used hi."})));
    fx.engine.answer_run(&run.id, None, "Use hi").expect("answer");
    let done = wait_run(&fx.engine, &run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
    assert!(!fx.engine.inbox(false).unwrap().iter().any(|i| matches!(i.kind, InboxKind::Question { .. })));
    assert!(fx.llm.requests().iter().any(|r| format!("{r:?}").contains("Use hi")));
}

#[test]
fn a_run_can_be_cancelled() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    deploy(&fx.engine, &graph(&repo, 1));
    fx.llm.push(Reply::text("thinking").with_delay(Duration::from_secs(20)));
    let run = fx.engine.start_run(&PipelineId::new("variants"), input()).expect("start");
    wait_run(&fx.engine, &run.id, "running", |r| r.nodes.iter().any(|n| n.node.as_str() == "coder" && n.session.is_some()));
    fx.engine.cancel_run(&run.id).expect("cancel");
    let cancelled = wait_run(&fx.engine, &run.id, "cancelled", |r| r.state == RunState::Cancelled);
    assert!(cancelled.finished.is_some());
    assert!(fx.engine.cancel_run(&run.id).is_err(), "a closed run cannot be cancelled again");
}

#[test]
fn a_run_interrupted_by_a_restart_resumes_where_it_stopped() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.nodes.truncate(2);
    g.edges.truncate(1);
    deploy(&fx.engine, &g);
    fx.llm.push(Reply::text("thinking").with_delay(Duration::from_secs(20)));
    let run = fx.engine.start_run(&g.id, input()).expect("start");
    wait_run(&fx.engine, &run.id, "running", |r| r.nodes.iter().any(|n| n.node.as_str() == "coder" && n.session.is_some()));
    fx.engine.shutdown();
    assert_eq!(fx.engine.run(&run.id).unwrap().unwrap().state, RunState::Interrupted);

    let (engine, llm) = fx.reopen();
    let restored = engine.run(&run.id).unwrap().unwrap();
    assert_eq!(restored.state, RunState::Interrupted);
    llm.push(Reply::tool("f1", "step_done", json!({"summary": "Finished after the restart."})));
    engine.resume_run(&run.id).expect("resume");
    let done = wait_run(&engine, &run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
    let coder = done.nodes.iter().rev().find(|n| n.node.as_str() == "coder").unwrap();
    assert_eq!(coder.summary, "Finished after the restart.");
    engine.shutdown();
}

#[test]
fn deploy_refuses_a_graph_with_errors_and_reports_changes() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut bad = graph(&repo, 1);
    bad.edges.push(route("merge", PORT_OUT, "nowhere"));
    fx.engine.save_pipeline(&catalog::Place::Library, bad.clone()).expect("save");
    let problems = fx.engine.pipeline_problems(&bad);
    assert!(problems.iter().any(|p| p.severity == model::Severity::Error), "{problems:#?}");
    let error = fx.engine.deploy_pipeline(&bad.id, "bad").expect_err("refused");
    assert!(format!("{error:#}").contains("cannot be deployed"), "{error:#}");
    assert!(fx.engine.start_run(&bad.id, input()).is_err(), "nothing is live");

    let good = graph(&repo, 1);
    deploy(&fx.engine, &good);
    let mut changed = good.clone();
    changed.nodes[1].name = "Writer".into();
    fx.engine.save_pipeline(&catalog::Place::Library, changed).expect("save");
    let changes = fx.engine.pipeline_changes(&good.id).expect("changes");
    assert!(!changes.is_empty(), "{changes:#?}");
    let unknown = {
        let mut g = good.clone();
        if let NodeKind::Agent(a) = &mut g.nodes[1].kind {
            a.model = Some(model::ModelRef::parse("nowhere/model"));
        }
        fx.engine.pipeline_problems(&g)
    };
    assert!(unknown.iter().any(|p| p.message.contains("nowhere/model")), "{unknown:#?}");
}

#[test]
fn the_daily_limit_blocks_a_start() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    deploy(&fx.engine, &graph(&repo, 1));
    let mut settings = fx.engine.settings().unwrap();
    settings.daily_limit_usd = Some(0.0);
    fx.engine.set_settings(settings).unwrap();
    let error = fx.engine.start_run(&PipelineId::new("variants"), input()).expect_err("blocked");
    assert!(format!("{error:#}").contains("spend limit"), "{error:#}");
}
