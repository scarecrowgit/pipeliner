//! Snapshots across a restart, and the app's own self-edit pipeline end to end.

use std::path::Path;

use catalog::{SelfEditModels, self_edit_pipeline};
use model::{ReviewState, SpanKind, StartFrom};

use super::*;
use crate::api::SpawnedState;
use crate::exec::Snapshot;

fn through_json(snapshot: &Snapshot) -> Snapshot {
    let text = serde_json::to_string(snapshot).expect("to json");
    let back: Snapshot = serde_json::from_str(&text).expect("from json");
    assert_eq!(&back, snapshot, "a snapshot survives json unchanged");
    back
}

fn scripted() -> Arc<FakeRunner> {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug").summary("A bug."));
    for attempt in 1..=2 {
        for (pos, tag) in ["a", "b", "c"].iter().enumerate() {
            let variant = pos as u32 + 1;
            let fixer = Script::done().session(&format!("s-{tag}")).output(NodeOutput { cost_usd: 0.2, ..work(tag, "Fixed.") });
            fake.on_key("fixer", variant, attempt, fixer);
            fake.on_key("tests", variant, attempt, Script::done().output(tested(tag, 9, 10)));
            fake.on_key("render", variant, attempt, Script::done().output(rendered(tag)));
        }
    }
    fake.on("pr", Script::done().output(NodeOutput { pr_url: Some("https://example.test/pr/9".into()), ..summary("Opened.") }));
    fake
}

/// The fix graph with three variants, two agents at a time, on a frozen clock.
fn frozen() -> Bench {
    let mut def = fix_graph(3);
    def.limits.max_parallel = 2;
    def.limits.budget_usd = Some(50.0);
    let mut bench = Bench::idle(Machine::new(def, run_shell(), item()), scripted());
    bench.tick = 0;
    let fx = bench.machine.start(bench.now).expect("start");
    bench.apply(fx);
    bench
}

/// Sends the review back once, ships variant 2 of the second round and runs to the end.
fn finish(bench: &mut Bench) {
    bench.drain();
    bench.decide(ReviewDecision::SendBack { note: "Tighter.".into(), start: StartFrom::All, variants: 3 }).expect("send back");
    bench.drain();
    bench.ship(2);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done, "{:?}", bench.machine.run().error);
}

#[test]
fn a_run_restored_mid_flight_ends_like_one_that_was_never_interrupted() {
    let mut straight = frozen();
    finish(&mut straight);

    // Stop at several points: with jobs running, with one waiting for a slot, at the review.
    for steps in [0, 1, 2, 4, 7, 10] {
        let mut bench = frozen();
        for _ in 0..steps {
            bench.step();
        }
        let running: Vec<InstanceKey> = bench.queue.iter().map(|job| job.key.clone()).collect();
        // Jobs that were running leave their session behind before the process dies.
        for job in &bench.queue {
            if job.key.node.as_str() == "fixer" {
                let tag = ["a", "b", "c"][job.key.variant as usize - 1];
                let place = JobNote::Place { session: Some(SessionId::new(format!("s-{tag}"))), worktree: None, branch: None };
                bench.machine.note(&job.key, place, bench.now).expect("place");
            }
        }
        let stored = through_json(bench.machine.snapshot());
        let graph = bench.machine.graph().clone();
        drop(bench);

        let mut bench = Bench::idle(Machine::restore(graph, stored), scripted());
        bench.tick = 0;
        if running.is_empty() {
            assert_eq!(bench.state(), RunState::WaitingReview, "after {steps} steps nothing was running");
        } else {
            assert_eq!(bench.state(), RunState::Interrupted, "after {steps} steps");
            for key in &running {
                assert_eq!(bench.status(key.node.as_str(), key.variant), NodeStatus::Interrupted);
            }
            assert_eq!(bench.machine.jobs_in_flight(), 0, "no job survived the restart");
            bench.resume();
            let reissued: Vec<InstanceKey> = bench.queue.iter().map(|job| job.key.clone()).collect();
            assert_eq!(reissued, running, "the same keys, in the same order");
            for job in bench.queue.iter().filter(|job| job.key.node.as_str() == "fixer") {
                let tag = ["a", "b", "c"][job.key.variant as usize - 1];
                assert_eq!(job.resume_session, Some(SessionId::new(format!("s-{tag}"))));
            }
        }
        finish(&mut bench);

        assert_eq!(bench.machine.run(), straight.machine.run(), "after {steps} steps");
        assert_eq!(bench.machine.snapshot().reviews, straight.machine.snapshot().reviews, "after {steps} steps");
    }
}

#[test]
fn a_restored_run_that_waited_for_a_person_is_left_as_it_was() {
    let bench = at_review(3);
    let id = bench.review().id;
    let mut machine = Machine::restore(bench.machine.graph().clone(), through_json(bench.machine.snapshot()));
    assert_eq!(machine.run().state, RunState::WaitingReview);
    assert_eq!(machine.resume(1), Err(ExecError::NotStopped));
    let fx = machine.decide(&id, ReviewDecision::Ship { variant: 1 }, 2).expect("the review is still open");
    assert!(fx.iter().any(|effect| matches!(effect, Effect::Start(job) if job.key.node.as_str() == "merge")));
}

#[test]
fn a_decision_is_taken_while_the_run_is_interrupted_and_acted_on_after_resume() {
    // `coder > review > pr` beside a long-running `docs`.
    let mut bench = Bench::start(graph(
        vec![source("src"), agent("coder", 1), review("review", Some("coder")), command("pr", false), agent("docs", 1)],
        vec![edge("src", PORT_OUT, "coder"), edge("coder", PORT_OUT, "review"), edge("review", PORT_APPROVED, "pr"), edge("src", PORT_OUT, "docs")],
    ));
    bench.step_node("coder", 0);
    let id = bench.review().id;
    let mut machine = Machine::restore(bench.machine.graph().clone(), through_json(bench.machine.snapshot()));
    assert_eq!(machine.run().state, RunState::Interrupted, "docs was running");
    assert_eq!(machine.open_reviews().len(), 1, "the review is still there");

    let fx = machine.decide(&id, ReviewDecision::Ship { variant: 1 }, 5).expect("ship");
    assert!(!fx.iter().any(|effect| matches!(effect, Effect::Start(_))), "nothing starts in a stopped run");
    let fx = machine.resume(6).expect("resume");
    let started: Vec<&str> = fx.iter().filter_map(|effect| if let Effect::Start(job) = effect { Some(job.key.node.as_str()) } else { None }).collect();
    assert_eq!(started, ["docs", "pr"]);
}

// ---------------------------------------------------------------- the self-edit pipeline

pub(super) fn self_edit() -> GraphDef {
    self_edit_pipeline(Path::new("/repo/pipeliner"), Path::new("/repo/mockups"), &SelfEditModels::default())
}

#[test]
fn the_self_edit_pipeline_goes_through_the_mockup_when_triage_says_so() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("mockup").summary("The inbox gets a new filter."));
    fake.on("designer", Script::done().output(NodeOutput { branch: Some("design/har-7".into()), ..work("design", "Added the filter row.") }));
    fake.on("render", Script::done().output(rendered("screens-v2/inbox")));
    let coder = |state, text: &str| JobNote::Spawned { id: "c1".into(), name: "Coder".into(), session: None, state, summary: text.into() };
    fake.on(
        "build",
        Script::done()
            .note(coder(SpawnedState::Running, ""))
            .note(coder(SpawnedState::Done, "Built the filter."))
            .output(work("code", "Implemented and verified.")),
    );
    fake.on("pull-request", Script::done().output(NodeOutput { pr_url: Some("https://example.test/pr/12".into()), ..summary("Opened a draft.") }));
    let mut bench = Bench::start_with(self_edit(), fake);
    bench.drain();

    assert_eq!(bench.state(), RunState::WaitingReview, "{:?}", bench.statuses());
    assert!(!bench.has("build", 0), "the build waits for the approval");
    let review = bench.review();
    assert_eq!(review.agent, "Mockup designer");
    assert_eq!(review.provenance, ["Issues", "Triage · rule 1", "Mockup designer", "You"]);
    assert_eq!(review.variants.len(), 1);
    assert_eq!(review.variants[0].branch.as_deref(), Some("design/har-7"));
    assert_eq!(review.renders.len(), 1);

    bench.ship(1);
    bench.drain();
    let run = bench.machine.run();
    assert_eq!(run.state, RunState::Done, "{:?}", run.error);
    assert_eq!(run.pr_url.as_deref(), Some("https://example.test/pr/12"));
    assert_eq!(
        bench.statuses(),
        [
            "source#0:Done",
            "triage#0:Done",
            "designer#0:Done",
            "render#0:Done",
            "approval#0:Done",
            "merge-mockup#0:Done",
            "build#0:Done",
            "c1#0:Done",
            "pull-request#0:Done"
        ]
    );
    let sub = run.instance(&"c1".into(), 0).expect("the coder");
    assert_eq!((sub.kind, sub.spawned_by.clone(), sub.summary.as_str()), (SpanKind::Spawned, Some("build".into()), "Built the filter."));

    let merge = &bench.jobs_of("merge-mockup")[0];
    assert_eq!(merge.upstream.last().map(|up| up.output.branch.as_deref()), Some(Some("design/har-7")), "the merge knows what was approved");
    let build = &bench.jobs_of("build")[0];
    let attached: Vec<&str> = build.attached.iter().map(|def| def.id.as_str()).collect();
    assert_eq!(attached, ["mockups-file", "coder"]);
    let upstream: Vec<&str> = build.upstream.iter().map(|up| up.key.node.as_str()).collect();
    assert_eq!(upstream, ["triage", "designer", "render", "approval", "merge-mockup"]);
    assert!(bench.fake.discards().is_empty());
}

#[test]
fn the_self_edit_pipeline_goes_straight_to_the_build_otherwise() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port(PORT_ELSE).summary("Pure refactoring."));
    let mut bench = Bench::start_with(self_edit(), fake);
    bench.step();
    assert_eq!(bench.queued(), ["build#0"]);
    bench.drain();

    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(
        bench.statuses(),
        [
            "source#0:Done",
            "triage#0:Done",
            "designer#0:Skipped",
            "render#0:Skipped",
            "approval#0:Skipped",
            "merge-mockup#0:Skipped",
            "build#0:Done",
            "pull-request#0:Done"
        ]
    );
    assert!(bench.machine.snapshot().reviews.is_empty());
    let build = &bench.jobs_of("build")[0];
    let upstream: Vec<&str> = build.upstream.iter().map(|up| up.key.node.as_str()).collect();
    assert_eq!(upstream, ["triage"]);
}

#[test]
fn the_self_edit_review_passes_through_when_the_designer_changed_nothing() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("mockup"));
    let mut bench = Bench::start_with(self_edit(), fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done, "its review skips itself when empty");
    assert_eq!(bench.inst("approval", 0).summary, "Nothing to review.");
    assert_eq!(bench.status("pull-request", 0), NodeStatus::Done);
}

#[test]
fn the_self_edit_review_can_send_the_mockup_back() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("mockup"));
    fake.on("designer", Script::done().output(work("design", "Added the filter row.")));
    let mut bench = Bench::start_with(self_edit(), fake);
    bench.drain();
    bench.decide(ReviewDecision::SendBack { note: "Use the chip style.".into(), start: StartFrom::Variant { index: 1 }, variants: 1 }).expect("send back");

    let job = bench.queue[0].clone();
    assert_eq!((job.key.node.as_str(), job.key.attempt), ("designer", 2));
    let retry = job.retry.expect("a retry");
    assert_eq!((retry.notes.as_slice(), retry.from_branch.as_deref()), (["Use the chip style.".to_string()].as_slice(), Some("fix/design")));
    bench.drain();
    assert_eq!(bench.review().feedback.len(), 1);
    assert_eq!(bench.machine.snapshot().reviews[0].review.state, ReviewState::SentBack);
    bench.ship(1);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.jobs_of("build").len(), 1);
}
