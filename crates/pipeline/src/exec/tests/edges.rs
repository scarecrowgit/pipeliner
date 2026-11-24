//! Corners of the state space: odd graph shapes and inputs that cross each other.

use model::StartFrom;

use super::*;
use crate::exec::Snapshot;

#[test]
fn every_step_reports_the_run_once_after_what_changed_it() {
    let mut bench = Bench::start(graph(vec![source("src"), agent("a", 1)], chain(&["src", "a"])));
    let kinds = |effects: &[Effect]| {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Start(_) => Some("start"),
                Effect::Run(_) => Some("run"),
                Effect::Finished(_) => Some("finished"),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&bench.effects), ["start", "run"]);
    let Some(Effect::Run(run)) = bench.effects.iter().find(|effect| matches!(effect, Effect::Run(_))) else { panic!("a run") };
    assert_eq!(run.state, RunState::Running);
    assert_eq!(run.started, bench.now);

    bench.effects.clear();
    bench.step();
    assert_eq!(kinds(&bench.effects), ["run", "finished"]);
    let Some(Effect::Run(run)) = bench.effects.first() else { panic!("a run") };
    assert_eq!(&**run, bench.machine.run());
}

#[test]
fn a_cancelled_job_behind_a_restart_is_discarded_when_it_returns() {
    // `coder > review` and `coder > lint`; lint is running in its own worktree when the
    // review starts over.
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::done().output(work("a", "Did it.")));
    let mut bench = Bench::start_with(
        graph(
            vec![source("src"), agent("coder", 1), review("review", Some("coder")), command("lint", false)],
            vec![edge("src", PORT_OUT, "coder"), edge("coder", PORT_OUT, "review"), edge("coder", PORT_OUT, "lint")],
        ),
        fake,
    );
    bench.step();
    let lint = bench.queue[0].key.clone();
    bench.machine.note(&lint, JobNote::Place { session: None, worktree: Some("/wt/lint".into()), branch: None }, 1).expect("place");

    bench.decide(ReviewDecision::SendBack { note: "No.".into(), start: StartFrom::Over, variants: 1 }).expect("send back");
    let gone = |bench: &Bench| bench.fake.discards().into_iter().map(|out| out.worktree).collect::<Vec<_>>();
    assert_eq!(gone(&bench), [Some(PathBuf::from("/wt/a"))], "the running job's worktree is not pulled from under it");

    let fx = bench.machine.job_finished(&lint, JobResult::Cancelled { output: NodeOutput::default() }, 2).expect("in flight");
    bench.apply(fx);
    assert_eq!(gone(&bench), [Some(PathBuf::from("/wt/a")), Some(PathBuf::from("/wt/lint"))]);
}

#[test]
fn variants_asked_for_inside_a_scope_fail_the_run_at_start() {
    let mut edges = chain(&["src", "outer", "lint", "inner", "review"]);
    edges.push(edge("review", PORT_APPROVED, "pr"));
    let nodes = vec![source("src"), agent("outer", 2), command("lint", false), agent("inner", 3), review("review", None), command("pr", false)];
    let bench = Bench::start(graph(nodes.clone(), edges));
    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(
        bench.machine.run().error.as_deref(),
        Some("Inner asks for variants inside the variant scope of Outer; put a review between them.")
    );
    assert!(bench.queue.is_empty());

    // With a review between them each scope is closed before the next opens.
    let mut edges = chain(&["src", "outer", "first", "inner", "review"]);
    edges.remove(2);
    edges.push(edge("first", PORT_APPROVED, "inner"));
    let mut nodes = nodes;
    nodes[2] = review("first", None);
    let mut bench = Bench::start(graph(nodes, edges));
    bench.drain();
    assert_eq!(bench.review().variants.len(), 2);
    bench.ship(1);
    bench.drain();
    assert_eq!(bench.review().variants.len(), 3);
}

#[test]
fn a_node_that_asks_for_too_many_variants_fails_the_run_at_start() {
    let bench = Bench::start(graph(vec![source("src"), agent("fixer", 9)], chain(&["src", "fixer"])));
    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.machine.run().error.as_deref(), Some("Fixer asks for 9 variants; the most is 8."));
    assert!(bench.queue.is_empty());

    let mut bench = Bench::start(graph(vec![source("src"), agent("fixer", 8)], chain(&["src", "fixer"])));
    assert_eq!(bench.queue.len(), 8);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn a_send_back_cannot_ask_for_more_variants_than_a_scope_may_have() {
    let mut bench = at_review(3);
    let before = bench.machine.snapshot().clone();
    for variants in [9, u32::MAX] {
        let decision = ReviewDecision::SendBack { note: "More.".into(), start: StartFrom::Over, variants };
        assert_eq!(bench.decide(decision), Err(ExecError::TooManyVariants(8)));
    }
    assert_eq!(bench.machine.snapshot(), &before);
    bench.decide(ReviewDecision::SendBack { note: "More.".into(), start: StartFrom::Over, variants: 8 }).expect("eight are fine");
    assert_eq!(bench.queue.len(), 8);
}

#[test]
fn a_send_back_cannot_open_a_scope_in_front_of_another() {
    // `writer > fixer ×2 > first > polish > second`; the second review sends back to the writer.
    let mut edges = chain(&["src", "writer", "fixer", "first"]);
    edges.push(edge("first", PORT_APPROVED, "polish"));
    edges.push(edge("polish", PORT_OUT, "second"));
    let nodes = vec![source("src"), agent("writer", 1), agent("fixer", 2), review("first", None), agent("polish", 1), review("second", Some("writer"))];
    let mut bench = Bench::start(graph(nodes, edges));
    bench.drain();
    bench.ship(1);
    bench.drain();
    assert_eq!(bench.review().node.as_str(), "second");

    let before = bench.machine.snapshot().clone();
    let decision = ReviewDecision::SendBack { note: "Two takes.".into(), start: StartFrom::All, variants: 2 };
    assert_eq!(bench.decide(decision), Err(ExecError::NestedVariants));
    assert_eq!(bench.machine.snapshot(), &before);
    bench.decide(ReviewDecision::SendBack { note: "Once more.".into(), start: StartFrom::All, variants: 1 }).expect("one take is fine");
    assert_eq!(bench.queued(), ["writer#0"]);
}

/// `src > triage`; rule `big` to `team ×3`, `else` to `solo`; both lead into `to`.
pub(super) fn exclusive_scope(to: NodeDef) -> GraphDef {
    let id = to.id.to_string();
    graph(
        vec![source("src"), router("triage", &["big"]), agent("team", 3), agent("solo", 1), to],
        vec![
            edge("src", PORT_OUT, "triage"),
            edge("triage", "big", "team"),
            edge("triage", PORT_ELSE, "solo"),
            edge("team", PORT_OUT, &id),
            edge("solo", PORT_OUT, &id),
        ],
    )
}

#[test]
fn a_scope_on_a_branch_not_taken_does_not_reach_the_review_behind_it() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("solo", Script::done().output(work("solo", "Did it alone.")));
    let mut bench = Bench::start_with(exclusive_scope(review("review", None)), fake);
    bench.drain();

    assert_eq!(bench.state(), RunState::WaitingReview, "the work of the branch taken is reviewed: {:?}", bench.statuses());
    assert_eq!(bench.statuses(), ["src#0:Done", "triage#0:Done", "team#0:Skipped", "solo#0:Done", "review#0:Waiting"]);
    let open = bench.review();
    assert_eq!(open.variants.iter().map(|variant| (variant.index, variant.branch.clone())).collect::<Vec<_>>(), [(1, Some("fix/solo".to_string()))]);
    assert_eq!(open.agent, "Solo");
    assert_eq!(open.provenance, ["Issues", "Triage · else", "Solo", "You"]);

    // The other way round the scope is there as before.
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("big"));
    let mut bench = Bench::start_with(exclusive_scope(review("review", None)), fake);
    bench.drain();
    assert_eq!(bench.review().variants.len(), 3);
    assert_eq!(bench.status("solo", 0), NodeStatus::Skipped);
}

#[test]
fn a_join_behind_a_scope_not_taken_runs_once() {
    let mut bench = Bench::start(exclusive_scope(command("join", false)));
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.jobs_of("join").len(), 1, "one item came through, on the single lane");
    assert_eq!(bench.jobs_of("join")[0].key.variant, 0);
    assert_eq!(bench.statuses(), ["src#0:Done", "triage#0:Done", "team#0:Skipped", "solo#0:Done", "join#0:Done"]);
}

#[test]
fn a_shared_branch_does_not_start_a_variant_whose_own_work_is_dead() {
    // `team ×2 > gate (router) > rule go > join` and `docs > join`: a variant whose router
    // says no does not reach the join on the back of the shared docs.
    let fake = Arc::new(FakeRunner::new());
    fake.on_variant("gate", 1, Script::port("go"));
    let mut bench = Bench::start_with(
        graph(
            vec![source("src"), agent("team", 2), router("gate", &["go"]), agent("docs", 1), command("join", false)],
            vec![
                edge("src", PORT_OUT, "team"),
                edge("team", PORT_OUT, "gate"),
                edge("gate", "go", "join"),
                edge("src", PORT_OUT, "docs"),
                edge("docs", PORT_OUT, "join"),
            ],
        ),
        fake,
    );
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.status("join", 1), NodeStatus::Done);
    assert_eq!(bench.status("join", 2), NodeStatus::Skipped);
    assert_eq!(bench.jobs_of("join").len(), 1);
}

#[test]
fn a_branch_all_variants_share_does_not_carry_a_failed_variant_into_the_review() {
    // `fixer ×2 > review` and `docs > review`.
    let fake = Arc::new(FakeRunner::new());
    fake.on_variant("fixer", 2, Script::fail("gave up"));
    let mut bench = Bench::start_with(
        graph(
            vec![source("src"), agent("fixer", 2), agent("docs", 1), review("review", Some("fixer"))],
            vec![edge("src", PORT_OUT, "fixer"), edge("src", PORT_OUT, "docs"), edge("fixer", PORT_OUT, "review"), edge("docs", PORT_OUT, "review")],
        ),
        fake,
    );
    bench.step_node("fixer", 1);
    bench.step_node("fixer", 2);
    assert!(bench.machine.open_reviews().is_empty(), "docs is still being written");
    bench.step_node("docs", 0);
    let review = bench.review();
    assert_eq!(review.variants.iter().map(|variant| variant.index).collect::<Vec<_>>(), [1]);
    assert_eq!(review.provenance, ["Issues", "Fixer ×2", "Docs", "You"]);
}

#[test]
fn sending_back_to_all_gives_a_failed_variant_another_go() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    fake.on_key("tests", 2, 1, Script::fail("exit 101").output(tested("b", 1, 10)));
    script_variants(&fake, 1, &["a", "b", "c"]);
    let mut bench = Bench::start_with(fix_graph(3), fake);
    bench.drain();
    assert_eq!(bench.review().variants.len(), 2);

    bench.decide(ReviewDecision::SendBack { note: "Fix the tests of B too.".into(), start: StartFrom::All, variants: 3 }).expect("send back");
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2", "fixer#3"]);
    assert_eq!(bench.queue[1].retry.as_ref().and_then(|retry| retry.from_branch.as_deref()), Some("fix/b"));
    assert!(bench.fake.discards().is_empty());
    bench.drain();
    assert_eq!(bench.review().variants.len(), 3);
}

#[test]
fn interrupting_keeps_reviews_open_and_resume_continues_the_jobs() {
    let mut bench = Bench::start(graph(
        vec![source("src"), agent("coder", 1), review("review", Some("coder")), agent("docs", 1)],
        vec![edge("src", PORT_OUT, "coder"), edge("coder", PORT_OUT, "review"), edge("src", PORT_OUT, "docs")],
    ));
    bench.step_node("coder", 0);
    let fx = bench.machine.interrupt(bench.now);
    bench.apply(fx);

    assert_eq!(bench.state(), RunState::Interrupted);
    assert_eq!(bench.status("docs", 0), NodeStatus::Interrupted);
    assert_eq!(bench.status("review", 0), NodeStatus::Waiting);
    assert_eq!(bench.machine.open_reviews().len(), 1);
    assert!(bench.finished_with().is_empty());
    assert!(bench.machine.interrupt(bench.now).is_empty(), "once is enough");

    bench.step();
    bench.resume();
    assert_eq!(bench.state(), RunState::WaitingReview);
    assert_eq!(bench.queued(), ["docs#0"]);

    // A run that only waits for a person is not interrupted at all.
    bench.drain();
    assert!(bench.machine.interrupt(bench.now).is_empty());
    assert_eq!(bench.state(), RunState::WaitingReview);
}

#[test]
fn planned_retries_survive_a_restart() {
    // `planner > fixer ×2 > review`, sent back to the planner: the fixers' retries wait in the
    // snapshot until the planner is done again.
    let mut edges = chain(&["src", "planner", "fixer", "review"]);
    edges.push(edge("review", PORT_APPROVED, "pr"));
    let def = graph(vec![source("src"), agent("planner", 1), agent("fixer", 2), review("review", Some("planner")), command("pr", false)], edges);
    let fake = Arc::new(FakeRunner::new());
    for (pos, tag) in ["a", "b"].iter().enumerate() {
        fake.on_variant("fixer", pos as u32 + 1, Script::done().output(work(tag, "Fixed.")));
    }
    let mut bench = Bench::start_with(def.clone(), fake.clone());
    bench.drain();
    bench.decide(ReviewDecision::SendBack { note: "Other plan.".into(), start: StartFrom::Variant { index: 1 }, variants: 2 }).expect("send back");
    assert_eq!(bench.machine.snapshot().retries.len(), 2);

    let text = serde_json::to_string(bench.machine.snapshot()).expect("json");
    let stored: Snapshot = serde_json::from_str(&text).expect("snapshot");
    assert_eq!(&stored, bench.machine.snapshot());
    let mut bench = Bench::idle(Machine::restore(def, stored), fake);
    assert_eq!(bench.state(), RunState::Interrupted, "the planner was running");
    bench.resume();
    bench.step();
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"]);
    for job in &bench.queue {
        let retry = job.retry.clone().expect("the planned retry");
        assert_eq!((retry.notes.as_slice(), retry.from_branch.as_deref()), (["Other plan.".to_string()].as_slice(), Some("fix/a")));
    }
    assert!(bench.machine.snapshot().retries.is_empty());
}

#[test]
fn spending_reported_by_a_superseded_job_still_counts_against_the_budget() {
    let mut def = graph(
        vec![source("src"), agent("coder", 1), review("review", Some("coder")), agent("slow", 1)],
        vec![edge("src", PORT_OUT, "coder"), edge("coder", PORT_OUT, "review"), edge("coder", PORT_OUT, "slow")],
    );
    def.limits.budget_usd = Some(1.0);
    let mut bench = Bench::start(def);
    bench.step();
    let slow = bench.queue[0].key.clone();
    bench.decide(ReviewDecision::SendBack { note: "Again.".into(), start: StartFrom::All, variants: 1 }).expect("send back");
    assert_eq!(bench.queued(), ["slow#0", "coder#0"]);

    let output = NodeOutput { cost_usd: 2.0, ..Default::default() };
    let fx = bench.machine.job_finished(&slow, JobResult::Cancelled { output }, bench.now).expect("in flight");
    assert!(fx.iter().any(|effect| matches!(effect, Effect::CancelJob(key) if key.node.as_str() == "coder")), "the cap is reached");
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert!(bench.machine.budget_reached());
    assert!((bench.machine.run().cost_usd - 2.0).abs() < 1e-9);
}
