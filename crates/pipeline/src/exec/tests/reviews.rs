//! Variant scopes and reviews: what a person is shown, Ship, Skip, and variants that fail.

use model::{ReviewState, SpanKind, StartFrom};

use super::*;

#[test]
fn three_variants_run_through_tests_and_render_into_one_review() {
    let bench = at_review(3);

    for node in ["fixer", "tests", "render"] {
        for variant in 1..=3 {
            assert_eq!(bench.status(node, variant), NodeStatus::Done, "{node} #{variant}");
        }
        assert!(!bench.has(node, 0), "{node} runs once per variant, not outside the scope");
    }
    let instance = bench.inst("review", 0);
    assert_eq!((instance.status, instance.kind), (NodeStatus::Waiting, SpanKind::Waiting));
    assert!(!bench.has("merge", 0));

    let review = bench.review();
    assert_eq!(review.id.as_str(), "run-1-review-1");
    assert_eq!((review.run.as_str(), review.node.as_str(), review.state), ("run-1", "review", ReviewState::Open));
    assert_eq!(review.issue, Some("HAR-7".into()));
    assert_eq!(review.title, "Double charge on retry");
    assert_eq!(review.agent, "Fixer");
    assert_eq!(review.provenance, ["Issues", "Triage · rule 1", "Fixer ×3", "You"]);
    assert!(review.feedback.is_empty());
    assert_eq!(review.picked, None);

    assert_eq!(review.variants.iter().map(|variant| variant.index).collect::<Vec<_>>(), [1, 2, 3]);
    let b = &review.variants[1];
    assert_eq!(b.name, "Approach b");
    assert_eq!(b.approach, "Approach b. With care.");
    assert_eq!(b.branch.as_deref(), Some("fix/b"));
    assert_eq!(b.worktree, Some(PathBuf::from("/wt/b")));
    assert_eq!(b.session, Some(SessionId::new("s-b")));
    assert_eq!((b.added, b.removed, b.files.len()), (12, 3, 1));
    assert_eq!(b.tests, Some(TestResult { passed: 10, total: 10 }));
    assert_eq!(b.tests_ok, Some(true));
    let took = |node: &str| {
        let instance = bench.inst(node, 2);
        (instance.finished.expect("finished") - instance.started.expect("started")) as u64 / 1000
    };
    assert!(took("fixer") > 0 && took("tests") > 0);
    assert_eq!(b.secs, took("fixer") + took("tests") + took("render"), "the agent and what ran on its work");
    let renders: Vec<&str> = review.renders.iter().map(|render| render.path.as_str()).collect();
    assert_eq!(renders, ["a.html", "b.html", "c.html"]);

    let opened = bench.effects.iter().filter(|effect| matches!(effect, Effect::ReviewOpened(_))).count();
    assert_eq!(opened, 1);
}

#[test]
fn inside_a_scope_a_job_sees_only_its_own_variant_and_what_came_before() {
    let bench = at_review(3);
    let render = bench.jobs_of("render").into_iter().find(|job| job.key.variant == 2).expect("render of b");
    let upstream: Vec<String> = render.upstream.iter().map(|up| format!("{}#{}", up.key.node, up.key.variant)).collect();
    assert_eq!(upstream, ["triage#0", "fixer#2", "tests#2"]);
}

#[test]
fn shipping_a_variant_discards_the_others_and_hands_its_outputs_on() {
    let mut bench = at_review(3);
    bench.ship(2);

    let review = &bench.machine.snapshot().reviews[0].review;
    assert_eq!((review.state, review.picked), (ReviewState::Shipped, Some(2)));
    assert!(bench.effects.iter().any(|effect| matches!(effect, Effect::ReviewClosed(review) if review.state == ReviewState::Shipped)));
    let instance = bench.inst("review", 0);
    assert_eq!((instance.status, instance.port.as_deref()), (NodeStatus::Done, Some("approved")));
    assert_eq!(bench.state(), RunState::Running);

    let discarded: Vec<Option<PathBuf>> = bench.fake.discards().into_iter().map(|out| out.worktree).collect();
    assert_eq!(discarded, [Some(PathBuf::from("/wt/a")), Some(PathBuf::from("/wt/c"))], "once per variant, never the picked one");
    assert_eq!(bench.fake.discards()[0].branch.as_deref(), Some("fix/a"));

    assert_eq!(bench.queued(), ["merge#0"], "after the review the item is outside the scope");
    let merge = &bench.queue[0];
    let upstream: Vec<String> = merge.upstream.iter().map(|up| format!("{}#{}", up.key.node, up.key.variant)).collect();
    assert_eq!(upstream, ["triage#0", "fixer#2", "tests#2", "render#2", "review#0"]);
    let shipped = &merge.upstream[4].output;
    assert_eq!((shipped.branch.as_deref(), shipped.worktree.clone()), (Some("fix/b"), Some(PathBuf::from("/wt/b"))));

    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.status("pr", 0), NodeStatus::Done);
    assert_eq!(bench.fake.discards().len(), 2);
}

#[test]
fn skip_ends_the_run_without_anything_after_the_review() {
    let mut bench = at_review(3);
    bench.decide(ReviewDecision::Skip).expect("skip");

    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.machine.snapshot().reviews[0].review.state, ReviewState::Skipped);
    assert_eq!(bench.inst("review", 0).port.as_deref(), Some(crate::exec::PORT_SKIPPED));
    assert_eq!(bench.status("merge", 0), NodeStatus::Skipped);
    assert_eq!(bench.status("pr", 0), NodeStatus::Skipped);
    assert!(bench.queue.is_empty());
    assert!(bench.fake.discards().is_empty(), "the variants stay where they are");
    assert_eq!(bench.finished_with(), [RunState::Done]);
}

#[test]
fn after_a_skip_a_join_behind_the_review_is_skipped_but_other_branches_finish() {
    // `src` fans out to `coder > review > join` and to `docs > join`; `docs` also feeds `site`.
    let mut bench = Bench::start(graph(
        vec![source("src"), agent("coder", 1), review("review", Some("coder")), agent("docs", 1), command("join", false), command("site", false)],
        vec![
            edge("src", PORT_OUT, "coder"),
            edge("coder", PORT_OUT, "review"),
            edge("review", PORT_APPROVED, "join"),
            edge("src", PORT_OUT, "docs"),
            edge("docs", PORT_OUT, "join"),
            edge("docs", PORT_OUT, "site"),
        ],
    ));
    bench.step_node("coder", 0);
    assert_eq!(bench.state(), RunState::WaitingReview, "a person is needed even while docs is written");
    bench.decide(ReviewDecision::Skip).expect("skip");
    assert_eq!(bench.state(), RunState::Running);

    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.status("join", 0), NodeStatus::Skipped);
    assert_eq!(bench.status("site", 0), NodeStatus::Done);
}

#[test]
fn a_single_variant_is_reviewed_as_variant_one() {
    let mut bench = at_review(1);
    assert_eq!(bench.status("fixer", 0), NodeStatus::Done);
    let review = bench.review();
    assert_eq!(review.variants.len(), 1);
    assert_eq!((review.variants[0].index, review.variants[0].branch.as_deref()), (1, Some("fix/a")));
    assert_eq!(review.provenance, ["Issues", "Triage · rule 1", "Fixer", "You"]);

    assert_eq!(bench.decide(ReviewDecision::Ship { variant: 2 }), Err(ExecError::UnknownVariant(2)));
    assert_eq!(bench.state(), RunState::WaitingReview, "a refused decision changes nothing");
    bench.ship(1);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert!(bench.fake.discards().is_empty());
}

#[test]
fn a_review_with_nothing_to_look_at_passes_through_when_told_to() {
    let mut def = fix_graph(1);
    for node in &mut def.nodes {
        if let NodeKind::Review(review) = &mut node.kind {
            review.skip_if_empty = true;
        }
    }
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    let mut bench = Bench::start_with(def.clone(), fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done, "no diff, no render: nobody is asked");
    assert_eq!(bench.inst("review", 0).port.as_deref(), Some("approved"));
    assert!(bench.machine.snapshot().reviews.is_empty());
    assert_eq!(bench.status("pr", 0), NodeStatus::Done);

    // With a diff the same review waits.
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    fake.on("fixer", Script::done().output(work("a", "Changed a file.")));
    let mut bench = Bench::start_with(def, fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
}

#[test]
fn a_failed_variant_drops_out_and_the_review_shows_the_rest() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    fake.on_key("tests", 2, 1, Script::fail("cargo test exited with 101").output(tested("b", 3, 10)));
    script_variants(&fake, 1, &["a", "b", "c"]);
    let mut bench = Bench::start_with(fix_graph(3), fake);
    bench.drain();

    assert_eq!(bench.state(), RunState::WaitingReview);
    assert_eq!(bench.status("tests", 2), NodeStatus::Failed);
    assert_eq!(bench.inst("tests", 2).summary, "cargo test exited with 101");
    assert_eq!(bench.status("render", 2), NodeStatus::Skipped);
    assert_eq!(bench.machine.run().error, None);
    let review = bench.review();
    assert_eq!(review.variants.iter().map(|variant| variant.index).collect::<Vec<_>>(), [1, 3]);

    assert_eq!(bench.decide(ReviewDecision::Ship { variant: 2 }), Err(ExecError::UnknownVariant(2)));
    bench.ship(3);
    let discarded: Vec<Option<PathBuf>> = bench.fake.discards().into_iter().map(|out| out.worktree).collect();
    assert_eq!(discarded, [Some(PathBuf::from("/wt/a")), Some(PathBuf::from("/wt/b"))], "the failed variant's worktree goes too");
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn the_run_fails_when_every_variant_failed() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    fake.on("fixer", Script::fail("model refused"));
    let mut def = fix_graph(3);
    def.limits.max_parallel = 3;
    let mut bench = Bench::start_with(def, fake);
    bench.step();
    bench.step_node("fixer", 1);
    bench.step_node("fixer", 3);
    assert_eq!(bench.state(), RunState::Running, "one variant is still alive");
    assert_eq!(bench.status("tests", 1), NodeStatus::Skipped);

    bench.step_node("fixer", 2);
    assert_eq!(bench.state(), RunState::Failed);
    let error = bench.machine.run().error.clone().unwrap_or_default();
    assert!(error.starts_with("Every variant of Fixer failed."), "{error}");
    assert!(error.contains("model refused"), "{error}");
    assert!(bench.machine.open_reviews().is_empty());
    assert_eq!(bench.finished_with(), [RunState::Failed]);
}

#[test]
fn a_failure_outside_a_scope_fails_the_run_after_cancelling_the_other_jobs() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("a", Script::fail("disk full"));
    let mut bench = Bench::start_with(
        graph(
            vec![source("src"), agent("a", 1), agent("b", 1), command("join", false)],
            vec![edge("src", PORT_OUT, "a"), edge("src", PORT_OUT, "b"), edge("a", PORT_OUT, "join"), edge("b", PORT_OUT, "join")],
        ),
        fake,
    );
    bench.effects.clear();
    bench.step_node("a", 0);

    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.machine.run().error.as_deref(), Some("A failed: disk full"));
    assert_eq!(bench.status("b", 0), NodeStatus::Cancelled);
    let order: Vec<&str> = bench
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::CancelJob(_) => Some("cancel"),
            Effect::Run(_) => Some("run"),
            Effect::Finished(_) => Some("finished"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["cancel", "run", "finished"]);

    // The cancelled job reports; the run stays failed and keeps what the job said.
    bench.step_node("b", 0);
    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.status("b", 0), NodeStatus::Cancelled);
    assert_eq!(bench.machine.jobs_in_flight(), 0);
}

#[test]
fn a_decision_for_a_review_that_is_not_open_is_refused() {
    let mut bench = at_review(3);
    let id = bench.review().id;
    assert_eq!(bench.machine.decide(&"nope".into(), ReviewDecision::Skip, 1), Err(ExecError::ReviewNotOpen("nope".into())));

    bench.ship(1);
    let before = bench.machine.snapshot().clone();
    for decision in [
        ReviewDecision::Ship { variant: 2 },
        ReviewDecision::Skip,
        ReviewDecision::SendBack { note: "again".into(), start: StartFrom::All, variants: 3 },
    ] {
        assert_eq!(bench.machine.decide(&id, decision, 2), Err(ExecError::ReviewNotOpen(id.clone())));
    }
    assert_eq!(bench.machine.snapshot(), &before, "refused inputs leave no trace");
}
