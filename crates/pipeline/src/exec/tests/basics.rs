//! Lines, routers, joins, parallel branches and the limits on what may start.

use model::{FileMode, FileNode, PipelineNode, SpanKind, SubagentNode};

use super::*;
use crate::api::SpawnedState;

#[test]
fn a_line_runs_node_by_node_to_done() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::done().output(work("a", "Fixed it.")).cost(0.4));
    fake.on("pr", Script::done().output(NodeOutput { pr_url: Some("https://example.test/pr/1".into()), ..summary("Opened.") }));
    let mut bench = Bench::start_with(graph(vec![source("src"), agent("coder", 1), command("pr", false)], chain(&["src", "coder", "pr"])), fake);

    assert_eq!(bench.state(), RunState::Running);
    assert_eq!(bench.status("src", 0), NodeStatus::Done);
    assert_eq!(bench.queued(), ["coder#0"]);
    assert!(!bench.has("pr", 0), "a node exists once the item reaches it");

    bench.step();
    assert_eq!(bench.queued(), ["pr#0"]);
    let coder = bench.inst("coder", 0).clone();
    assert_eq!((coder.status, coder.port.as_deref(), coder.kind), (NodeStatus::Done, Some("out"), SpanKind::Agent));
    assert_eq!(coder.summary, "Fixed it.");
    assert_eq!(coder.session, Some(SessionId::new("s-a")));
    assert_eq!(coder.worktree, Some(PathBuf::from("/wt/a")));
    assert_eq!(coder.branch.as_deref(), Some("fix/a"));
    assert!(coder.started.is_some() && coder.finished > coder.started);

    bench.step();
    let run = bench.machine.run();
    assert_eq!(run.state, RunState::Done);
    assert_eq!(run.finished, Some(bench.now));
    assert_eq!(run.pr_url.as_deref(), Some("https://example.test/pr/1"));
    assert!((run.cost_usd - 0.4).abs() < 1e-9);
    assert_eq!(run.error, None);
    assert_eq!(bench.inst("pr", 0).kind, SpanKind::Step);
    assert_eq!(bench.finished_with(), [RunState::Done]);
    assert_eq!(bench.machine.jobs_in_flight(), 0);
}

#[test]
fn a_job_sees_the_item_and_what_was_done_before_it() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("planner", Script::done().summary("Plan: two steps."));
    fake.on("coder", Script::done().output(work("a", "Done.")));
    let mut bench = Bench::start_with(
        graph(vec![source("src"), agent("planner", 1), agent("coder", 1), command("check", false)], chain(&["src", "planner", "coder", "check"])),
        fake,
    );
    bench.drain();

    let check = &bench.jobs_of("check")[0];
    assert_eq!(check.run, RunId::new("run-1"));
    assert_eq!(check.item, item());
    assert_eq!(check.key, InstanceKey { node: "check".into(), variant: 0, attempt: 1 });
    let names: Vec<&str> = check.upstream.iter().map(|up| up.name.as_str()).collect();
    assert_eq!(names, ["Planner", "Coder"], "in the order they finished, without the source");
    assert_eq!(check.upstream[0].output.summary, "Plan: two steps.");
    assert_eq!(check.upstream[1].output.worktree, Some(PathBuf::from("/wt/a")));
    assert_eq!((check.retry.clone(), check.answer.clone(), check.resume_session.clone()), (None, None, None));
    assert_eq!(check.budget_left_usd, None);
    assert!(!check.test);
}

#[test]
fn a_router_takes_the_rule_it_answers_and_the_other_way_is_skipped() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug").summary("Crash on retry."));
    let mut bench = Bench::start_with(fix_graph(1), fake);
    bench.step();

    assert_eq!(bench.inst("triage", 0).port.as_deref(), Some("bug"));
    assert_eq!(bench.inst("triage", 0).summary, "Crash on retry.");
    assert_eq!(bench.status("report", 0), NodeStatus::Skipped);
    assert_eq!(bench.queued(), ["fixer#0"]);
    assert_eq!(bench.inst("report", 0).started, None);
}

#[test]
fn a_router_without_a_match_goes_through_else_and_skips_the_whole_other_path() {
    let mut bench = Bench::start(fix_graph(3));
    bench.drain();

    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.status("report", 0), NodeStatus::Done);
    for dead in ["fixer", "tests", "render", "review", "merge", "pr"] {
        assert_eq!(bench.status(dead, 0), NodeStatus::Skipped, "{dead}");
        assert!(!bench.has(dead, 1), "{dead}: a scope the item never enters is not opened");
    }
    assert!(bench.fake.jobs_of("fixer").is_empty());
    assert!((bench.machine.run().progress() - 1.0).abs() < 1e-6, "skipped instances do not count");
}

#[test]
fn a_router_answer_that_is_no_rule_fails_the_run() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("feature"));
    let mut bench = Bench::start_with(fix_graph(1), fake);
    bench.drain();

    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.status("triage", 0), NodeStatus::Failed);
    assert!(bench.machine.run().error.as_deref().is_some_and(|error| error.contains("feature")), "{:?}", bench.machine.run().error);
}

/// `src > triage`, `bug` to `fixer`, `else` to `writer`, both into `pr`.
fn exclusive_join() -> GraphDef {
    graph(
        vec![source("src"), router("triage", &["bug"]), agent("fixer", 1), agent("writer", 1), command("pr", false)],
        vec![
            edge("src", PORT_OUT, "triage"),
            edge("triage", "bug", "fixer"),
            edge("triage", PORT_ELSE, "writer"),
            edge("fixer", PORT_OUT, "pr"),
            edge("writer", PORT_OUT, "pr"),
        ],
    )
}

#[test]
fn a_join_after_exclusive_branches_starts_when_the_taken_one_delivers() {
    for (port, taken, dead) in [("bug", "fixer", "writer"), (PORT_ELSE, "writer", "fixer")] {
        let fake = Arc::new(FakeRunner::new());
        fake.on("triage", Script::port(port));
        let mut bench = Bench::start_with(exclusive_join(), fake);
        bench.step();
        assert_eq!(bench.status(dead, 0), NodeStatus::Skipped);
        assert_eq!(bench.queued(), [format!("{taken}#0")]);
        assert!(!bench.has("pr", 0), "the join waits for the live branch");

        bench.drain();
        assert_eq!(bench.state(), RunState::Done);
        let upstream: Vec<String> = bench.jobs_of("pr")[0].upstream.iter().map(|up| up.key.node.to_string()).collect();
        assert_eq!(upstream, ["triage", taken], "only the path taken");
    }
}

/// `src` fans out to `a` and `b`, which join in `join`.
fn diamond() -> GraphDef {
    graph(
        vec![source("src"), agent("a", 1), agent("b", 1), command("join", false)],
        vec![edge("src", PORT_OUT, "a"), edge("src", PORT_OUT, "b"), edge("a", PORT_OUT, "join"), edge("b", PORT_OUT, "join")],
    )
}

#[test]
fn parallel_branches_run_together_and_the_join_waits_for_both() {
    let mut bench = Bench::start(diamond());
    assert_eq!(bench.queued(), ["a#0", "b#0"]);

    bench.step_node("b", 0);
    assert!(!bench.has("join", 0), "one branch is still running");
    assert_eq!(bench.state(), RunState::Running);

    bench.step_node("a", 0);
    assert_eq!(bench.queued(), ["join#0"]);
    let upstream: Vec<String> = bench.queue[0].upstream.iter().map(|up| up.key.node.to_string()).collect();
    assert_eq!(upstream, ["b", "a"], "in the order they finished");

    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn a_join_is_skipped_when_every_way_into_it_is_dead() {
    // Both branches hang off router rules that are not taken; `else` leads nowhere.
    let mut bench = Bench::start(graph(
        vec![source("src"), router("triage", &["x", "y"]), agent("a", 1), agent("b", 1), command("join", false)],
        vec![
            edge("src", PORT_OUT, "triage"),
            edge("triage", "x", "a"),
            edge("triage", "y", "b"),
            edge("a", PORT_OUT, "join"),
            edge("b", PORT_OUT, "join"),
        ],
    ));
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.statuses(), ["src#0:Done", "triage#0:Done", "a#0:Skipped", "b#0:Skipped", "join#0:Skipped"]);
}

#[test]
fn no_more_agents_work_at_once_than_max_parallel_and_steps_do_not_count() {
    // Four agents and a command hang off the source; two agents may work at once.
    let mut nodes = vec![source("src"), command("lint", false)];
    let mut edges = vec![edge("src", PORT_OUT, "lint")];
    for id in ["a", "b", "c", "d"] {
        nodes.push(agent(id, 1));
        edges.push(edge("src", PORT_OUT, id));
    }
    let mut def = graph(nodes, edges);
    def.limits.max_parallel = 2;
    let mut bench = Bench::start_with(def, Arc::new(FakeRunner::new()));

    assert_eq!(bench.queued(), ["lint#0", "a#0", "b#0"], "the command starts beside two agents");
    assert_eq!(bench.status("c", 0), NodeStatus::Pending);
    assert_eq!(bench.state(), RunState::Running);

    bench.step_node("b", 0);
    assert_eq!(bench.queued(), ["lint#0", "a#0", "c#0"], "a free slot goes to the oldest pending agent");
    bench.step_node("lint", 0);
    assert_eq!(bench.queued(), ["a#0", "c#0"], "a finished step frees nothing");
    bench.step_node("a", 0);
    assert_eq!(bench.queued(), ["c#0", "d#0"]);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn variants_share_the_parallel_limit() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    let mut def = fix_graph(3);
    def.limits.max_parallel = 2;
    let mut bench = Bench::start_with(def, fake);
    bench.step();
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"]);
    assert_eq!(bench.status("fixer", 3), NodeStatus::Pending);

    bench.step_node("fixer", 1);
    assert_eq!(bench.queued(), ["fixer#2", "fixer#3", "tests#1"], "the third variant takes the slot; tests are a step and need none");
}

#[test]
fn a_nested_pipeline_fails_the_run_with_a_clear_message_when_the_item_reaches_it() {
    let nested = node("nested", "Pay triage", NodeKind::Pipeline(PipelineNode { pipeline: PipelineId::new("pay") }));
    let mut bench = Bench::start(graph(vec![source("src"), agent("a", 1), nested], chain(&["src", "a", "nested"])));
    assert_eq!(bench.state(), RunState::Running, "not before the item gets there");
    bench.drain();

    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.status("nested", 0), NodeStatus::Failed);
    assert_eq!(bench.machine.run().error.as_deref(), Some("Pay triage: nested pipelines are not available yet."));
    assert!(bench.fake.jobs_of("nested").is_empty());
    assert_eq!(bench.finished_with(), [RunState::Failed]);
}

#[test]
fn a_source_that_is_not_available_fails_the_run() {
    for (kind, word) in [(SourceKind::Jira, "Jira"), (SourceKind::Sentry, "Sentry")] {
        let src = node("src", "Tracker", NodeKind::Source(SourceNode { source: kind }));
        let mut machine = Machine::new(graph(vec![src, agent("a", 1)], chain(&["src", "a"])), run_shell(), Item { source: kind, ..item() });
        let fx = machine.start(5).expect("start");
        assert_eq!(machine.run().state, RunState::Failed);
        assert_eq!(machine.run().error, Some(format!("Tracker: the {word} source is not available yet.")));
        assert!(!fx.iter().any(|effect| matches!(effect, Effect::Start(_))));
        assert!(fx.contains(&Effect::Finished(RunState::Failed)));
    }
}

#[test]
fn the_item_enters_at_the_source_of_its_kind_and_other_sources_are_dead() {
    let manual = node("manual", "Manual", NodeKind::Source(SourceNode { source: SourceKind::Manual }));
    let def = graph(
        vec![source("issues"), manual, agent("a", 1), agent("b", 1)],
        vec![edge("issues", PORT_OUT, "a"), edge("manual", PORT_OUT, "b")],
    );
    let mut bench = Bench::idle(Machine::new(def, run_shell(), Item { source: SourceKind::Manual, ..item() }), Arc::new(FakeRunner::new()));
    let fx = bench.machine.start(1).expect("start");
    bench.apply(fx);
    bench.drain();
    assert_eq!(bench.statuses(), ["issues#0:Skipped", "manual#0:Done", "a#0:Skipped", "b#0:Done"]);
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn a_graph_that_cannot_run_fails_at_start() {
    let mut looped = Machine::new(
        graph(vec![source("src"), agent("a", 1), agent("b", 1)], vec![edge("src", PORT_OUT, "a"), edge("a", PORT_OUT, "b"), edge("b", PORT_OUT, "a")]),
        run_shell(),
        item(),
    );
    looped.start(1).expect("start");
    assert_eq!(looped.run().state, RunState::Failed);
    assert!(looped.run().error.as_deref().is_some_and(|error| error.contains("loop")), "{:?}", looped.run().error);

    let mut empty = Machine::new(graph(vec![agent("a", 1)], Vec::new()), run_shell(), item());
    empty.start(1).expect("start");
    assert_eq!(empty.run().error.as_deref(), Some("The pipeline has no source node."));
    assert_eq!(empty.start(2), Err(ExecError::AlreadyStarted));
}

#[test]
fn files_and_sub_agent_templates_ride_with_the_job_of_their_agent() {
    let file = |id: &str| node(id, id, NodeKind::File(FileNode { workdir: "code".into(), path: "docs/**".into(), mode: FileMode::Read }));
    let mut def = graph(
        vec![source("src"), agent("lead", 1), file("docs"), file("specs"), node("coder", "Coder", NodeKind::Subagent(SubagentNode::default())), agent("other", 1)],
        chain(&["src", "lead", "other"]),
    );
    let attach = |from: &str, to: &str, kind| EdgeDef { from: from.into(), port: PORT_OUT.into(), to: to.into(), kind };
    def.edges.push(attach("docs", "lead", EdgeKind::Context));
    def.edges.push(attach("lead", "coder", EdgeKind::Spawn));
    def.edges.push(attach("specs", "coder", EdgeKind::Context));
    let mut bench = Bench::start(def);
    bench.drain();

    let attached = |node: &str| bench.jobs_of(node)[0].attached.iter().map(|def| def.id.to_string()).collect::<Vec<_>>();
    assert_eq!(attached("lead"), ["docs", "coder", "specs"]);
    assert!(attached("other").is_empty());
    assert_eq!(bench.state(), RunState::Done);
    assert!(!bench.machine.run().nodes.iter().any(|instance| ["docs", "specs", "coder"].contains(&instance.node.as_str())), "never scheduled");
}

#[test]
fn spawned_sub_agents_appear_as_instances_of_their_parent() {
    let spawned = |state, summary: &str| JobNote::Spawned {
        id: "sub-1".into(),
        name: "Coder".into(),
        session: Some(SessionId::new("s-sub")),
        state,
        summary: summary.into(),
    };
    let fake = Arc::new(FakeRunner::new());
    fake.on(
        "lead",
        Script::done()
            .note(spawned(SpawnedState::Running, ""))
            .note(spawned(SpawnedState::Done, "Wrote the parser."))
            .note(JobNote::Spawned { id: "sub-2".into(), name: "Tester".into(), session: None, state: SpawnedState::Running, summary: String::new() }),
    );
    let mut bench = Bench::start_with(graph(vec![source("src"), agent("lead", 1), command("pr", false)], chain(&["src", "lead", "pr"])), fake);
    bench.drain();

    let run = bench.machine.run();
    let spawned: Vec<&NodeInstance> = run.nodes.iter().filter(|instance| instance.kind == SpanKind::Spawned).collect();
    assert_eq!(spawned.len(), 2, "one instance per sub-agent, updated in place");
    assert_eq!(spawned[0].name, "Coder");
    assert_eq!(spawned[0].spawned_by, Some("lead".into()));
    assert_eq!((spawned[0].status, spawned[0].summary.as_str()), (NodeStatus::Done, "Wrote the parser."));
    assert_eq!(spawned[0].session, Some(SessionId::new("s-sub")));
    assert!(spawned[0].started.is_some() && spawned[0].finished.is_some());
    assert_eq!(spawned[1].status, NodeStatus::Done, "what the parent left running ends with it");
    assert_eq!(run.state, RunState::Done);
    assert_eq!(bench.jobs_of("pr")[0].upstream.len(), 1, "sub-agents are not on the item's path");
}

#[test]
fn notes_fill_in_the_place_the_cost_and_the_log_while_the_job_runs() {
    let mut bench = Bench::start(graph(vec![source("src"), agent("a", 1)], chain(&["src", "a"])));
    let key = bench.queue[0].key.clone();
    let place = JobNote::Place { session: Some(SessionId::new("s-1")), worktree: Some("/wt/x".into()), branch: None };
    bench.machine.note(&key, place, 10).expect("place");
    let usage = model::Usage { input: 100, output: 20, ..Default::default() };
    bench.machine.note(&key, JobNote::Spent { cost_usd: 0.25, usage }, 11).expect("spent");
    let fx = bench.machine.note(&key, JobNote::Log { level: model::LogLevel::Tool, text: "read src/lib.rs".into() }, 12).expect("log");

    let a = bench.inst("a", 0);
    assert_eq!((a.status, a.session.clone(), a.worktree.clone()), (NodeStatus::Running, Some(SessionId::new("s-1")), Some(PathBuf::from("/wt/x"))));
    assert!((a.cost_usd - 0.25).abs() < 1e-9);
    assert_eq!(bench.machine.run().usage, usage);
    let [Effect::Log(line)] = fx.as_slice() else { panic!("one log line expected: {fx:?}") };
    assert_eq!((line.node.clone(), line.level, line.text.as_str(), line.at), (Some("a".into()), model::LogLevel::Tool, "read src/lib.rs", 12));

    // The output repeats the totals the notes already reported: nothing is counted twice.
    let output = NodeOutput { cost_usd: 0.4, usage: model::Usage { input: 150, output: 20, ..Default::default() }, ..summary("ok") };
    bench.machine.job_finished(&key, JobResult::Done { port: PORT_OUT.into(), output }, 13).expect("done");
    let run = bench.machine.run();
    assert!((run.cost_usd - 0.4).abs() < 1e-9, "{}", run.cost_usd);
    assert_eq!(run.usage, model::Usage { input: 150, output: 20, ..Default::default() });
    assert_eq!(bench.inst("a", 0).session, Some(SessionId::new("s-1")), "kept when the output does not name one");
}
