use model::{IssueKey, LogLevel, NodeId, NodeStatus, PipelineId, RunId, RunState, RunnableRef, SpanKind, Usage};
use serde_json::{Value, json};

use super::{Fixture, old_issue, old_run, old_stage, png};

fn run_id(id: &str) -> RunId {
    RunId::new(id)
}

fn setup(fx: &Fixture, run: &Value) {
    fx.write_json(
        "issues.json",
        &json!({"next": 1, "items": [old_issue("HAR-1", "review", run["id"].as_str())]}),
    );
    fx.write_json(&format!("runs/{}.json", run["id"].as_str().unwrap()), run);
}

/// A finished run with notes, a log, per-agent usage and two renders.
fn full_run(fx: &Fixture) -> Value {
    let renders = fx.data().join("runs/run1/renders");
    let mut run = old_run("run1", "HAR-1", "done");
    run["stages"] = json!([
        old_stage("triage", "Triage", "decision", "done"),
        {"id": "mockup", "name": "Mockup", "kind": "agent", "status": "done", "started": 1_700_000_300, "finished": 1_700_000_900, "summary": "short"},
        old_stage("approval", "Approval", "approval", "skipped"),
        {"id": "pull-request", "name": "Pull request", "kind": "pull_request", "status": "done", "started": 0, "finished": 0, "summary": "https://github.com/o/r/pull/1"},
    ]);
    run["notes"] = json!({"mockup": "  I changed two mockups.\n\nDetails here.  "});
    run["log"] = json!([
        {"at": 1_700_000_060, "who": "pipeline", "kind": "step", "text": "worktree ready"},
        {"at": 1_700_000_070, "who": "Triage", "kind": "tool", "text": "triage_decision "},
        {"at": 1_700_000_080, "who": "Mockup", "kind": "text", "text": "Looking at the board."},
        {"at": 1_700_000_090, "who": "Coder 2", "kind": "error", "text": "cargo failed"},
    ]);
    run["usage"] = json!([
        {"who": "Triage", "model": "nr/cc/claude-haiku-5-5", "tokens": 5000, "cost_usd": 0.001},
        {"who": "Mockup", "model": "nr/cc/claude-opus-5-5", "tokens": 100_000, "cost_usd": 2.5},
        {"who": "Coder 2", "model": "nr/cc/claude-sonnet-5-5", "tokens": 7000, "cost_usd": 0.25},
    ]);
    run["tokens"] = json!(112_000);
    run["cost_usd"] = json!(2.751);
    run["pr_url"] = json!("https://github.com/o/r/pull/1");
    run["renders"] = json!([
        {"step": "render", "screen": "issue-board", "before": renders.join("issue-board.before.png"), "after": renders.join("issue-board.r1.png")},
        {"step": "render", "screen": "home", "before": null, "after": renders.join("home.r1.png")},
    ]);
    run
}

fn write_pictures(fx: &Fixture) {
    fx.write("runs/run1/renders/issue-board.before.png", png(640, 480, 1));
    fx.write("runs/run1/renders/issue-board.r1.png", png(641, 481, 2));
    fx.write("runs/run1/renders/home.r1.png", png(300, 200, 3));
}

#[test]
fn a_finished_run_becomes_history_with_nodes_logs_usage_and_renders() {
    let fx = Fixture::new();
    let run = full_run(&fx);
    setup(&fx, &run);
    write_pictures(&fx);
    let report = fx.import();
    assert_eq!((report.runs.imported, report.images.imported), (1, 3), "{report}");
    assert!(report.problems.is_empty(), "{report}");

    let run = fx.store.run(&run_id("run1")).unwrap().unwrap();
    assert_eq!(run.pipeline, PipelineId::new("mockup-gpui-build"));
    assert_eq!((run.version, run.number, run.test), (0, 1, false));
    assert_eq!(run.issue, Some(IssueKey::new("HAR-1")));
    assert_eq!(
        (run.title.as_str(), run.input.as_str()),
        ("Title of HAR-1", "Body text")
    );
    assert_eq!(run.state, RunState::Done);
    assert_eq!(run.started, 1_700_000_050_000);
    assert_eq!(run.finished, Some(1_700_000_900_000));
    assert_eq!(run.cost_usd, 2.751);
    assert_eq!(
        run.usage,
        Usage {
            input: 112_000,
            ..Usage::default()
        }
    );
    assert_eq!(run.pr_url.as_deref(), Some("https://github.com/o/r/pull/1"));
    assert_eq!(run.error, None);

    assert_eq!(run.nodes.len(), 4);
    let mockup = &run.nodes[1];
    assert_eq!((mockup.node.as_str(), mockup.name.as_str()), ("mockup", "Mockup"));
    assert_eq!(
        (mockup.status, mockup.kind, mockup.attempt, mockup.variant),
        (NodeStatus::Done, SpanKind::Agent, 1, 0)
    );
    assert_eq!(
        (mockup.started, mockup.finished),
        (Some(1_700_000_300_000), Some(1_700_000_900_000))
    );
    // The note is the agent's own summary, the stage's one-liner is the fallback.
    assert_eq!(mockup.summary, "I changed two mockups.\n\nDetails here.");
    assert_eq!(run.nodes[0].summary, "one line");
    assert_eq!(
        (run.nodes[2].status, run.nodes[2].kind),
        (NodeStatus::Skipped, SpanKind::Waiting)
    );
    assert_eq!((run.nodes[3].started, run.nodes[3].kind), (None, SpanKind::Step));
    assert_eq!(mockup.cost_usd, 2.5);

    let log = fx.store.run_log_after(&run_id("run1"), 0, 100).unwrap();
    assert_eq!(log.len(), 4);
    assert_eq!(
        (log[0].node.clone(), log[0].level, log[0].at),
        (None, LogLevel::Info, 1_700_000_060_000)
    );
    assert_eq!(
        (log[1].node.clone(), log[1].level),
        (Some(NodeId::new("triage")), LogLevel::Tool)
    );
    assert_eq!(
        (log[2].node.clone(), log[2].level),
        (Some(NodeId::new("mockup")), LogLevel::Info)
    );
    // A sub-agent that is not a stage keeps its name.
    assert_eq!(
        (log[3].node.clone(), log[3].level, log[3].text.as_str()),
        (None, LogLevel::Error, "Coder 2: cargo failed")
    );
    assert_eq!(log.iter().map(|l| l.seq).collect::<Vec<_>>(), [1, 2, 3, 4]);
}

#[test]
fn per_agent_usage_goes_to_the_ledger_marked_as_imported() {
    let fx = Fixture::new();
    let run = full_run(&fx);
    setup(&fx, &run);
    write_pictures(&fx);
    fx.import();

    let spent = fx.store.spend_for_run(&run_id("run1")).unwrap();
    assert_eq!(spent.calls, 3);
    assert!((spent.cost_usd - 2.751).abs() < 1e-9);
    assert_eq!(spent.usage.input, 112_000);
    let by_node = fx.store.spend_by_node(&run_id("run1")).unwrap();
    assert_eq!(
        by_node.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["mockup", "triage"]
    );
    // `Coder 2` is the agent `coder`; its number only tells instances apart.
    let by_agent = fx.store.spend_by_runnable(0, i64::MAX, 10).unwrap();
    let coder = by_agent
        .iter()
        .find(|row| row.runnable == RunnableRef::agent("coder"))
        .unwrap();
    assert_eq!((coder.name.as_str(), coder.cost_usd), ("Coder 2", 0.25));
    // Dated at the run's end.
    assert_eq!(
        fx.store
            .spend_between(1_700_000_900_000, 1_700_000_900_001)
            .unwrap()
            .calls,
        3
    );
}

#[test]
fn renders_are_copied_into_the_blob_store() {
    let fx = Fixture::new();
    let run = full_run(&fx);
    setup(&fx, &run);
    write_pictures(&fx);
    fx.import();

    let renders = fx.store.renders(&run_id("run1")).unwrap();
    assert_eq!(renders.len(), 2);
    assert_eq!(renders[0].path, "screens/issue-board.html");
    assert_eq!((renders[0].width, renders[0].height), (641, 481));
    assert_eq!(
        fx.store.read_blob(renders[0].before.as_ref().unwrap()).unwrap(),
        png(640, 480, 1)
    );
    assert_eq!(fx.store.read_blob(&renders[0].after).unwrap(), png(641, 481, 2));
    // A new file has no before.
    assert_eq!(
        (renders[1].path.as_str(), renders[1].before.clone()),
        ("screens/home.html", None)
    );
    assert_eq!((renders[1].width, renders[1].height), (300, 200));
    assert_eq!(fx.store.read_blob(&renders[1].after).unwrap(), png(300, 200, 3));
}

#[test]
fn a_missing_picture_is_reported_and_the_rest_of_the_run_imports() {
    let fx = Fixture::new();
    let run = full_run(&fx);
    setup(&fx, &run);
    fx.write("runs/run1/renders/issue-board.r1.png", png(641, 481, 2));
    // The "before" of the first render and the "after" of the second are gone.
    let report = fx.import();
    assert_eq!(
        (report.runs.imported, report.images.imported, report.images.failed),
        (1, 1, 2),
        "{report}"
    );
    assert_eq!(report.problems.len(), 2);
    assert!(
        report.problems[0].item.ends_with("issue-board (before)"),
        "{}",
        report.problems[0].item
    );
    assert!(
        report.problems[1].item.ends_with("home (after)"),
        "{}",
        report.problems[1].item
    );

    let renders = fx.store.renders(&run_id("run1")).unwrap();
    assert_eq!(renders.len(), 1);
    assert_eq!(renders[0].before, None);
}

#[test]
fn a_picture_is_found_in_the_runs_folder_when_the_recorded_path_is_elsewhere() {
    let fx = Fixture::new();
    let mut run = full_run(&fx);
    run["renders"] = json!([{"step": "render", "screen": "home", "before": "/elsewhere/old/home.before.png", "after": "/elsewhere/old/home.r1.png"}]);
    setup(&fx, &run);
    fx.write("runs/run1/renders/home.before.png", png(5, 6, 1));
    fx.write("runs/run1/renders/home.r1.png", png(7, 8, 2));
    let report = fx.import();
    assert_eq!(report.images.imported, 2, "{report}");
    let renders = fx.store.renders(&run_id("run1")).unwrap();
    assert_eq!((renders[0].width, renders[0].height), (7, 8));
}

#[test]
fn a_path_outside_the_data_directory_is_never_read() {
    let fx = Fixture::new();
    let outside = fx.paths.home.join("secret.png");
    std::fs::create_dir_all(&fx.paths.home).unwrap();
    std::fs::write(&outside, png(1, 1, 9)).unwrap();
    let mut run = old_run("run1", "HAR-1", "done");
    run["renders"] = json!([{"step": "render", "screen": "home", "before": null, "after": outside}]);
    setup(&fx, &run);
    let report = fx.import();
    assert_eq!(report.images.failed, 1);
    assert!(fx.store.renders(&run_id("run1")).unwrap().is_empty());
}

#[test]
fn a_run_that_was_running_or_waiting_becomes_interrupted() {
    let fx = Fixture::new();
    let mut running = old_run("run1", "HAR-1", "running");
    running["stages"] = json!([
        old_stage("triage", "Triage", "decision", "done"),
        old_stage("build", "Build", "agent", "running"),
        old_stage("pull-request", "Pull request", "pull_request", "pending"),
    ]);
    let mut waiting = old_run("run2", "HAR-1", "waiting_approval");
    waiting["created"] = json!(1_700_001_000);
    waiting["stages"] = json!([old_stage("approval", "Approval", "approval", "waiting")]);
    let mut failed = old_run("run3", "HAR-1", "failed");
    failed["created"] = json!(1_700_002_000);
    failed["error"] = json!("the build broke");
    let mut cancelled = old_run("run4", "HAR-1", "cancelled");
    cancelled["created"] = json!(1_700_003_000);
    fx.write_json(
        "issues.json",
        &json!({"next": 1, "items": [old_issue("HAR-1", "build", Some("run4"))]}),
    );
    for run in [&running, &waiting, &failed, &cancelled] {
        fx.write_json(&format!("runs/{}.json", run["id"].as_str().unwrap()), run);
    }
    fx.import();

    let get = |id: &str| fx.store.run(&run_id(id)).unwrap().unwrap();
    let running = get("run1");
    assert_eq!((running.state, running.finished), (RunState::Interrupted, None));
    assert_eq!(
        running.nodes.iter().map(|n| n.status).collect::<Vec<_>>(),
        [NodeStatus::Done, NodeStatus::Interrupted, NodeStatus::Pending]
    );
    assert_eq!(get("run2").state, RunState::Interrupted);
    assert_eq!(get("run2").nodes[0].status, NodeStatus::Interrupted);
    let failed = get("run3");
    assert_eq!(
        (failed.state, failed.error.as_deref()),
        (RunState::Failed, Some("the build broke"))
    );
    assert!(failed.finished.is_some());
    assert_eq!(get("run4").state, RunState::Cancelled);
    // The engine finds the interrupted ones among the open runs.
    let open: Vec<String> = fx.store.open_runs().unwrap().iter().map(|r| r.id.to_string()).collect();
    assert_eq!(open, ["run1", "run2"]);
}

#[test]
fn a_run_of_the_first_version_has_no_ids_names_or_pipeline() {
    let fx = Fixture::new();
    let run = json!({
        "id": "run-first", "issue": "HAR-1", "created": 1_700_000_050, "status": "done",
        "stages": [
            {"kind": "triage", "status": "done", "started": 1_700_000_051, "finished": 1_700_000_060, "summary": "needs mockup changes"},
            {"kind": "pull_request", "status": "done"}
        ],
        "tokens": 396_701, "needs_design": true, "screens": ["home"], "tasks": [],
        "log": [{"at": 1_700_000_060, "who": "Triage", "kind": "step", "text": "needs mockup changes"}]
    });
    setup(&fx, &run);
    let report = fx.import();
    assert!(report.problems.is_empty(), "{report}");

    let run = fx.store.run(&run_id("run-first")).unwrap().unwrap();
    assert_eq!(run.pipeline, PipelineId::new("mockup-gpui-build"));
    let nodes: Vec<(&str, &str)> = run.nodes.iter().map(|n| (n.node.as_str(), n.name.as_str())).collect();
    assert_eq!(nodes, [("triage", "Triage"), ("pull_request", "Pull request")]);
    assert_eq!(run.nodes[0].summary, "needs mockup changes");
    // Only a token total was recorded: no cost, nothing for the ledger.
    assert_eq!((run.cost_usd, run.usage.input), (0.0, 396_701));
    assert_eq!(fx.store.spend_for_run(&run_id("run-first")).unwrap().calls, 0);
    assert_eq!(
        fx.store.run_log_after(&run_id("run-first"), 0, 10).unwrap()[0].node,
        Some(NodeId::new("triage"))
    );
}

#[test]
fn a_stage_id_used_twice_becomes_attempts() {
    let fx = Fixture::new();
    let mut run = old_run("run1", "HAR-1", "done");
    run["stages"] = json!([
        old_stage("build", "Build", "agent", "failed"),
        old_stage("build", "Build", "agent", "done")
    ]);
    setup(&fx, &run);
    fx.import();
    let run = fx.store.run(&run_id("run1")).unwrap().unwrap();
    assert_eq!(run.nodes.iter().map(|n| n.attempt).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(run.instance(&NodeId::new("build"), 0).unwrap().status, NodeStatus::Done);
}

#[test]
fn a_corrupt_run_file_is_reported_and_the_others_import() {
    let fx = Fixture::new();
    fx.write_json("issues.json", &json!({"next": 2, "items": [old_issue("HAR-1", "review", Some("run1")), old_issue("HAR-2", "done", Some("run2"))]}));
    fx.write_json("runs/run1.json", &old_run("run1", "HAR-1", "done"));
    fx.write("runs/run2.json", "{\"id\": \"run2\", \"stages\": [");
    fx.write_json(
        "runs/run3.json",
        &json!({"id": "run3", "issue": "HAR-2", "created": 1, "status": "exploded", "stages": []}),
    );
    let report = fx.import();
    assert_eq!((report.runs.imported, report.runs.failed), (1, 2), "{report}");
    let items: Vec<&str> = report.problems.iter().map(|p| p.item.as_str()).collect();
    assert_eq!(items, ["runs/run2.json", "runs/run3.json"]);
    // The issue whose run is unreadable is still imported, without a run.
    let issue = fx.store.issue(&IssueKey::new("HAR-2")).unwrap().unwrap();
    assert_eq!((issue.run, issue.run_count), (None, 0));
    assert_eq!(report.issues.imported, 2);
}

#[test]
fn a_run_left_half_written_is_completed_by_the_next_import() {
    let fx = Fixture::new();
    let run = full_run(&fx);
    setup(&fx, &run);
    write_pictures(&fx);
    // An earlier run wrote the run row and two log lines, then stopped before the marker.
    let first = fx.import();
    assert_eq!(first.runs.imported, 1);
    assert!(fx.store.remove_marker("v1_run:run1").unwrap());

    let again = fx.import();
    assert_eq!(again.runs.imported, 1);
    // Nothing was written twice.
    assert_eq!(fx.store.run_log_after(&run_id("run1"), 0, 100).unwrap().len(), 4);
    assert_eq!(fx.store.renders(&run_id("run1")).unwrap().len(), 2);
    assert_eq!(fx.store.spend_for_run(&run_id("run1")).unwrap().calls, 3);
    let third = fx.import();
    assert_eq!((third.runs.imported, third.runs.skipped), (0, 1));
}
