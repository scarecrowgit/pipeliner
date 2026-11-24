//! "Send back": new attempts, where they start from, and how often.

use model::{ReviewState, StartFrom};

use super::*;

fn send_back(bench: &mut Bench, note: &str, start: StartFrom, variants: u32) -> Result<(), ExecError> {
    bench.decide(ReviewDecision::SendBack { note: note.into(), start, variants })
}

fn discarded(bench: &Bench) -> Vec<String> {
    bench.fake.discards().into_iter().map(|out| out.worktree.map(|path| path.display().to_string()).unwrap_or_default()).collect()
}

fn attempts(bench: &Bench, node: &str) -> Vec<(u32, u32)> {
    bench.machine.run().nodes.iter().filter(|instance| instance.node.as_str() == node).map(|instance| (instance.attempt, instance.variant)).collect()
}

#[test]
fn send_back_to_all_gives_every_variant_the_note_and_its_own_branch() {
    let mut bench = at_review(3);
    script_variants(&bench.fake, 2, &["a2", "b2", "c2"]);
    send_back(&mut bench, "Handle the timeout too.", StartFrom::All, 3).expect("send back");

    assert_eq!(bench.machine.snapshot().reviews[0].review.state, ReviewState::SentBack);
    assert!(bench.machine.open_reviews().is_empty());
    assert_eq!(bench.state(), RunState::Running);
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2", "fixer#3"]);
    assert!(discarded(&bench).is_empty(), "every variant builds on its own work");
    for job in &bench.queue {
        assert_eq!(job.key.attempt, 2);
        let tag = ["a", "b", "c"][job.key.variant as usize - 1];
        let retry = job.retry.clone().expect("a retry");
        assert_eq!(retry.notes, ["Handle the timeout too."]);
        assert_eq!(retry.start, StartFrom::All);
        assert_eq!(retry.from_branch, Some(format!("fix/{tag}")));
        assert_eq!(retry.from_worktree, Some(PathBuf::from(format!("/wt/{tag}"))));
        assert_eq!(retry.from_session, Some(SessionId::new(format!("s-{tag}"))));
        assert_eq!(job.upstream.iter().map(|up| up.key.node.as_str()).collect::<Vec<_>>(), ["triage"], "not its own old attempt");
    }

    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
    let review = bench.review();
    assert_eq!(review.id.as_str(), "run-1-review-2");
    assert_eq!(review.feedback.iter().map(|note| note.text.as_str()).collect::<Vec<_>>(), ["Handle the timeout too."]);
    assert_eq!(review.variants[0].branch.as_deref(), Some("fix/a2"));
    assert!(bench.jobs_of("tests").iter().all(|job| job.retry.is_none()), "only the node that was sent back gets the note");

    // History keeps both rounds.
    assert_eq!(attempts(&bench, "fixer"), [(1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3)]);
    assert_eq!(attempts(&bench, "review"), [(1, 0), (2, 0)]);
    assert_eq!(bench.inst("review", 0).attempt, 2);

    bench.ship(1);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn send_back_from_one_variant_builds_the_new_round_on_its_branch() {
    let mut bench = at_review(3);
    script_variants(&bench.fake, 2, &["b1", "b2"]);
    send_back(&mut bench, "Smaller diff.", StartFrom::Variant { index: 2 }, 2).expect("send back");

    assert_eq!(discarded(&bench), ["/wt/a", "/wt/c"], "the variants that were not picked go at once");
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"]);
    for job in &bench.queue {
        let retry = job.retry.clone().expect("a retry");
        assert_eq!(retry.start, StartFrom::Variant { index: 2 });
        assert_eq!((retry.from_branch.as_deref(), retry.from_session), (Some("fix/b"), Some(SessionId::new("s-b"))));
    }
    bench.drain();
    let review = bench.review();
    assert_eq!(review.variants.iter().map(|variant| variant.index).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(review.provenance, ["Issues", "Triage · rule 1", "Fixer ×2", "You"]);

    bench.ship(1);
    assert_eq!(discarded(&bench), ["/wt/a", "/wt/c", "/wt/b", "/wt/b2"], "then the base and the other new variant");
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn starting_over_discards_everything_and_can_shrink_to_one_variant() {
    let mut bench = at_review(3);
    script_variants(&bench.fake, 2, &["z"]);
    send_back(&mut bench, "Wrong direction.", StartFrom::Over, 1).expect("send back");

    assert_eq!(discarded(&bench), ["/wt/a", "/wt/b", "/wt/c"]);
    assert_eq!(bench.queued(), ["fixer#0"], "one variant is no scope");
    let retry = bench.queue[0].retry.clone().expect("a retry");
    assert_eq!((retry.start, retry.from_branch, retry.from_worktree, retry.from_session), (StartFrom::Over, None, None, None));
    assert_eq!(retry.notes, ["Wrong direction."]);

    bench.drain();
    let review = bench.review();
    assert_eq!(review.variants.iter().map(|variant| (variant.index, variant.branch.clone())).collect::<Vec<_>>(), [(1, Some("fix/z".to_string()))]);
    bench.ship(1);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(discarded(&bench).len(), 3, "nothing of the shipped round is thrown away");
}

#[test]
fn a_single_variant_can_be_sent_back_for_three() {
    let mut bench = at_review(1);
    send_back(&mut bench, "Show me options.", StartFrom::All, 3).expect("send back");

    assert_eq!(bench.queued(), ["fixer#1", "fixer#2", "fixer#3"]);
    assert!(bench.queue.iter().all(|job| job.retry.as_ref().is_some_and(|retry| retry.from_branch.as_deref() == Some("fix/a"))));
    assert!(discarded(&bench).is_empty());
    bench.drain();
    assert_eq!(bench.review().variants.len(), 3);
    assert_eq!(bench.review().provenance, ["Issues", "Triage · rule 1", "Fixer ×3", "You"]);
}

#[test]
fn a_zero_count_keeps_the_number_of_variants() {
    let mut bench = at_review(2);
    send_back(&mut bench, "Again.", StartFrom::All, 0).expect("send back");
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"]);
}

#[test]
fn notes_pile_up_and_the_loop_limit_makes_the_run_wait_for_a_person() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    fake.on("fixer", Script::done().output(work("a", "Tried.")));
    let mut def = fix_graph(1);
    def.limits.max_loops = 2;
    let mut bench = Bench::start_with(def, fake);
    bench.drain();

    send_back(&mut bench, "One.", StartFrom::All, 1).expect("first");
    bench.drain();
    send_back(&mut bench, "Two.", StartFrom::All, 1).expect("second");
    assert_eq!(bench.queue[0].retry.clone().expect("a retry").notes, ["One.", "Two."]);
    assert_eq!(bench.queue[0].key.attempt, 3);
    bench.drain();
    assert_eq!(bench.review().feedback.len(), 2);

    let before = bench.machine.snapshot().clone();
    assert_eq!(send_back(&mut bench, "Three.", StartFrom::All, 1), Err(ExecError::LoopLimit(2)));
    assert_eq!(bench.machine.snapshot(), &before);
    assert_eq!(bench.state(), RunState::WaitingReview, "it does not loop; it waits");
    assert!(bench.queue.is_empty());

    bench.ship(1);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn a_send_back_that_names_no_such_variant_changes_nothing() {
    let mut bench = at_review(3);
    let before = bench.machine.snapshot().clone();
    assert_eq!(send_back(&mut bench, "x", StartFrom::Variant { index: 7 }, 2), Err(ExecError::UnknownVariant(7)));
    assert_eq!(bench.machine.snapshot(), &before);
}

/// `src > planner > fixer ×3 > review > pr`; the review sends back to `to`.
pub(super) fn planned(to: &str) -> GraphDef {
    let mut edges = chain(&["src", "planner", "fixer", "polish", "review"]);
    edges.push(edge("review", PORT_APPROVED, "pr"));
    graph(vec![source("src"), agent("planner", 1), agent("fixer", 3), agent("polish", 1), review("review", Some(to)), command("pr", false)], edges)
}

fn planned_at_review(to: &str) -> Bench {
    let fake = Arc::new(FakeRunner::new());
    fake.on("planner", Script::done().output(NodeOutput { session: Some(SessionId::new("s-plan")), ..summary("Plan.") }));
    for (pos, tag) in ["a", "b", "c"].iter().enumerate() {
        fake.on_variant("fixer", pos as u32 + 1, Script::done().output(work(tag, "Fixed.")));
        // The polisher works in the fixer's worktree and reports only that.
        let polished = NodeOutput { worktree: Some(PathBuf::from(format!("/wt/{tag}"))), session: Some(SessionId::new(format!("p-{tag}"))), ..summary("Polished.") };
        fake.on_variant("polish", pos as u32 + 1, Script::done().output(polished));
    }
    let mut bench = Bench::start_with(planned(to), fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
    assert_eq!(bench.review().agent, "Fixer", "the scope's opener is what is compared");
    bench
}

#[test]
fn send_back_to_a_node_before_the_scope_reruns_it_and_reopens_the_scope() {
    let mut bench = planned_at_review("planner");
    send_back(&mut bench, "Plan for retries.", StartFrom::Variant { index: 2 }, 2).expect("send back");

    assert_eq!(bench.queued(), ["planner#0"]);
    let planner = bench.queue[0].clone();
    assert_eq!(planner.key.attempt, 2);
    let retry = planner.retry.expect("a retry");
    assert_eq!((retry.notes.as_slice(), retry.from_session), (["Plan for retries.".to_string()].as_slice(), Some(SessionId::new("s-plan"))));
    assert_eq!(discarded(&bench), ["/wt/a", "/wt/c"]);

    bench.step();
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"], "the new count applies to the opener");
    for job in &bench.queue {
        assert_eq!(job.key.attempt, 2);
        assert_eq!(job.retry.as_ref().and_then(|retry| retry.from_branch.as_deref()), Some("fix/b"));
        assert_eq!(job.upstream.iter().map(|up| (up.key.node.as_str(), up.key.attempt)).collect::<Vec<_>>(), [("planner", 2)]);
    }
    bench.drain();
    assert_eq!(bench.review().variants.len(), 2);
}

#[test]
fn send_back_to_a_node_inside_the_scope_reruns_it_in_every_variant() {
    let mut bench = planned_at_review("polish");
    send_back(&mut bench, "Keep the comments.", StartFrom::All, 5).expect("send back");

    assert_eq!(bench.queued(), ["polish#1", "polish#2", "polish#3"], "the scope stays; the count is not used");
    assert!(discarded(&bench).is_empty());
    for job in &bench.queue {
        assert_eq!(job.key.attempt, 2);
        let tag = ["a", "b", "c"][job.key.variant as usize - 1];
        assert_eq!(job.retry.as_ref().and_then(|retry| retry.from_session.clone()), Some(SessionId::new(format!("p-{tag}"))));
        assert_eq!(job.upstream.iter().map(|up| (up.key.node.as_str(), up.key.attempt)).collect::<Vec<_>>(), [("planner", 1), ("fixer", 1)]);
    }
    assert_eq!(bench.jobs_of("fixer").len(), 3, "the opener is not run again");
    bench.drain();
    assert_eq!(bench.review().variants.len(), 3);
}

#[test]
fn send_back_inside_the_scope_from_one_variant_drops_the_others() {
    let mut bench = planned_at_review("polish");
    send_back(&mut bench, "Only this one.", StartFrom::Variant { index: 2 }, 3).expect("send back");

    assert_eq!(bench.queued(), ["polish#2"]);
    assert_eq!(discarded(&bench), ["/wt/a", "/wt/c"]);
    bench.drain();
    let review = bench.review();
    assert_eq!(review.variants.iter().map(|variant| variant.index).collect::<Vec<_>>(), [2], "it keeps its letter");
    bench.ship(2);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(discarded(&bench), ["/wt/a", "/wt/c"]);
    let upstream: Vec<String> = bench.jobs_of("pr")[0].upstream.iter().map(|up| format!("{}#{}@{}", up.key.node, up.key.variant, up.key.attempt)).collect();
    assert_eq!(upstream, ["planner#0@1", "fixer#2@1", "polish#2@2", "review#0@2"]);
}

#[test]
fn starting_over_from_inside_the_scope_restarts_its_opener() {
    let mut bench = planned_at_review("polish");
    send_back(&mut bench, "From scratch.", StartFrom::Over, 2).expect("send back");
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"]);
    assert!(bench.queue.iter().all(|job| job.key.attempt == 2 && job.retry.as_ref().is_some_and(|retry| retry.from_branch.is_none())));
    assert_eq!(discarded(&bench), ["/wt/a", "/wt/b", "/wt/c"]);
    assert_eq!(bench.jobs_of("planner").len(), 1);
}

/// `src` fans out to `coder > review > join` and `docs > join`; `coder` also feeds `lint`.
pub(super) fn side_branches() -> GraphDef {
    graph(
        vec![source("src"), agent("coder", 1), review("review", Some("coder")), agent("docs", 1), command("lint", false), command("join", false)],
        vec![
            edge("src", PORT_OUT, "coder"),
            edge("coder", PORT_OUT, "review"),
            edge("coder", PORT_OUT, "lint"),
            edge("review", PORT_APPROVED, "join"),
            edge("src", PORT_OUT, "docs"),
            edge("docs", PORT_OUT, "join"),
        ],
    )
}

#[test]
fn a_join_keeps_the_branch_that_was_not_sent_back() {
    let mut bench = Bench::start(side_branches());
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
    send_back(&mut bench, "Again.", StartFrom::All, 1).expect("send back");
    bench.drain();
    bench.ship(1);
    bench.drain();

    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.jobs_of("docs").len(), 1, "docs is not behind the node that was sent back");
    assert_eq!(bench.jobs_of("lint").len(), 2, "lint is");
    let join = &bench.jobs_of("join")[0];
    let mut upstream: Vec<String> = join.upstream.iter().map(|up| format!("{}@{}", up.key.node, up.key.attempt)).collect();
    upstream.sort();
    assert_eq!(upstream, ["coder@2", "docs@1", "review@2"]);
    assert_eq!(join.key.attempt, 1, "the join itself never ran before");
}

#[test]
fn a_job_behind_the_target_is_cancelled_and_its_late_result_changes_nothing() {
    let mut bench = Bench::start(side_branches());
    bench.step_node("coder", 0);
    bench.step_node("docs", 0);
    assert_eq!(bench.queued(), ["lint#0"]);
    let old = bench.queue[0].key.clone();

    send_back(&mut bench, "Again.", StartFrom::All, 1).expect("send back");
    assert_eq!(bench.cancelled, std::slice::from_ref(&old), "the running lint is told to stop");
    assert_eq!(bench.queued(), ["lint#0", "coder#0"]);
    bench.step_node("coder", 0);
    assert_eq!(bench.queued(), ["lint#0", "lint#0"], "the new attempt has its own key");
    assert_eq!(bench.queue[1].key.attempt, 2);

    // The old job ignores the cancel and reports success: history only.
    bench.cancelled.clear();
    let snapshot_before = bench.machine.run().nodes.iter().filter(|instance| instance.node.as_str() == "lint").count();
    let fx = bench.machine.job_finished(&old, JobResult::Done { port: PORT_OUT.into(), output: summary("late") }, 1).expect("in flight");
    assert!(!fx.iter().any(|effect| matches!(effect, Effect::Start(_))));
    assert_eq!(bench.machine.run().nodes.iter().filter(|instance| instance.node.as_str() == "lint").count(), snapshot_before);
    assert_eq!(bench.inst("lint", 0).status, NodeStatus::Running, "the new attempt is untouched");
    assert_eq!(bench.machine.job_finished(&old, JobResult::Done { port: PORT_OUT.into(), output: summary("twice") }, 2), Err(ExecError::NoJob(old)));

    bench.queue.remove(0);
    bench.drain();
    bench.ship(1);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}
