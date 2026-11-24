//! Cancel, failure and resume, and inputs that arrive too late.

use model::ReviewState;

use super::*;
use crate::exec::Hold;

fn line() -> GraphDef {
    graph(vec![source("src"), agent("coder", 1), command("pr", false)], chain(&["src", "coder", "pr"]))
}

fn key(node: &str) -> InstanceKey {
    InstanceKey { node: node.into(), variant: 0, attempt: 1 }
}

#[test]
fn cancel_before_start_closes_the_run() {
    let mut machine = Machine::new(line(), run_shell(), item());
    let fx = machine.cancel(1).expect("cancel");
    assert_eq!(machine.run().state, RunState::Cancelled);
    assert!(fx.contains(&Effect::Finished(RunState::Cancelled)));
    assert_eq!(machine.start(2), Err(ExecError::RunClosed(RunState::Cancelled)));
    assert_eq!(machine.cancel(3), Err(ExecError::RunClosed(RunState::Cancelled)));
}

#[test]
fn cancel_while_jobs_run_stops_them_and_resume_reissues_them() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::done().session("s-1").summary("Done."));
    let mut bench = Bench::start_with(line(), fake);
    bench.machine.note(&key("coder"), JobNote::Place { session: Some(SessionId::new("s-1")), worktree: None, branch: None }, 1).expect("place");
    bench.cancel();

    assert_eq!(bench.state(), RunState::Cancelled);
    assert_eq!(bench.cancelled, [key("coder")]);
    assert_eq!(bench.status("coder", 0), NodeStatus::Cancelled);
    assert!(bench.machine.run().finished.is_some());
    assert_eq!(bench.finished_with(), [RunState::Cancelled]);
    assert_eq!(bench.machine.jobs_in_flight(), 1, "its result is still expected");

    // The cancelled job reports: nothing moves.
    bench.step();
    assert_eq!(bench.state(), RunState::Cancelled);
    assert_eq!(bench.machine.jobs_in_flight(), 0);
    assert!(bench.queue.is_empty());
    assert!(!bench.has("pr", 0));

    bench.resume();
    assert_eq!(bench.state(), RunState::Running);
    assert_eq!(bench.machine.run().finished, None);
    let job = bench.queue[0].clone();
    assert_eq!((job.key.clone(), job.resume_session.clone()), (key("coder"), Some(SessionId::new("s-1"))));
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.finished_with(), [RunState::Cancelled, RunState::Done]);
    assert_eq!(bench.machine.resume(1), Err(ExecError::NotStopped));
}

#[test]
fn a_job_that_finished_while_being_cancelled_is_not_run_again() {
    let mut bench = Bench::start(line());
    bench.cancel();
    bench.cancelled.clear();
    bench.step();
    assert_eq!(bench.state(), RunState::Cancelled);
    assert_eq!(bench.status("coder", 0), NodeStatus::Done, "the work is there; keep it");
    assert!(bench.queue.is_empty(), "but a cancelled run does not go on");

    bench.resume();
    assert_eq!(bench.queued(), ["pr#0"]);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.jobs_of("coder").len(), 1);
}

#[test]
fn resume_before_the_cancelled_job_returned_waits_for_it() {
    let mut bench = Bench::start(line());
    bench.cancel();
    bench.resume();
    assert!(bench.effects.iter().filter(|effect| matches!(effect, Effect::Start(_))).count() == 1, "no second job for the key yet");
    assert_eq!(bench.status("coder", 0), NodeStatus::Pending);
    assert_eq!(bench.state(), RunState::Running);

    bench.step();
    assert_eq!(bench.queued(), ["coder#0"]);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn cancel_while_a_review_is_open_closes_it_and_resume_opens_it_again() {
    let mut bench = at_review(3);
    let id = bench.review().id;
    bench.cancel();

    assert_eq!(bench.state(), RunState::Cancelled);
    assert!(bench.machine.open_reviews().is_empty());
    assert!(bench.effects.iter().any(|effect| matches!(effect, Effect::ReviewClosed(review) if review.id == id && review.state == ReviewState::Skipped)));
    assert_eq!(bench.status("review", 0), NodeStatus::Cancelled);
    assert_eq!(bench.machine.decide(&id, ReviewDecision::Ship { variant: 1 }, 1), Err(ExecError::ReviewNotOpen(id.clone())));

    bench.resume();
    assert_eq!(bench.state(), RunState::WaitingReview);
    let review = bench.review();
    assert_eq!((review.id.clone(), review.variants.len()), (id, 3));
    assert_eq!(bench.machine.snapshot().reviews.len(), 1, "the same review, not a second one");
    assert_eq!(bench.fake.jobs_of("fixer").len(), 3, "nothing is run again");
    bench.ship(2);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn cancel_while_waiting_for_an_answer_keeps_the_question_for_resume() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::ask("Which?"));
    fake.on("coder", Script::done());
    let mut bench = Bench::start_with(line(), fake);
    bench.step();
    bench.cancel();

    assert_eq!(bench.state(), RunState::Cancelled);
    assert_eq!(bench.status("coder", 0), NodeStatus::Cancelled);
    assert!(bench.cancelled.is_empty(), "no job was running");
    assert_eq!(bench.machine.answer(&key("coder"), "x".into(), 1), Err(ExecError::NotWaiting(key("coder"))));

    bench.effects.clear();
    bench.resume();
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.machine.holds(), [(key("coder"), Hold::Question { text: "Which?".into() })]);
    assert!(bench.effects.contains(&Effect::Asked { key: key("coder"), question: "Which?".into() }), "asked again");
    assert!(bench.queue.is_empty(), "the agent is not run again without an answer");

    let fx = bench.machine.answer(&key("coder"), "This.".into(), bench.now + 1).expect("answer");
    bench.apply(fx);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn cancel_while_waiting_for_budget_and_resume_still_waits_for_the_raise() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::done().cost(1.0));
    let mut def = line();
    def.limits.budget_usd = Some(1.0);
    let mut bench = Bench::start_with(def, fake);
    bench.step();
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.status("pr", 0), NodeStatus::Pending);

    bench.cancel();
    assert_eq!(bench.state(), RunState::Cancelled);
    bench.resume();
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert!(bench.queue.is_empty());
    let fx = bench.machine.raise_budget(1.0, bench.now + 1).expect("raise");
    bench.apply(fx);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn cancel_while_variants_run_stops_all_of_them() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    let mut def = fix_graph(3);
    def.limits.max_parallel = 2;
    let mut bench = Bench::start_with(def, fake);
    bench.step();
    bench.step_node("fixer", 1);
    bench.cancel();

    assert_eq!(bench.cancelled.len(), 3, "fixer 2, fixer 3 and the tests of 1");
    assert_eq!(bench.status("fixer", 1), NodeStatus::Done);
    assert_eq!(bench.status("fixer", 2), NodeStatus::Cancelled);
    assert_eq!(bench.status("tests", 1), NodeStatus::Cancelled);
    bench.drain();
    assert_eq!(bench.machine.jobs_in_flight(), 0);

    bench.resume();
    assert_eq!(bench.queued(), ["fixer#2", "fixer#3", "tests#1"]);
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
    assert_eq!(bench.review().variants.len(), 3);
    assert_eq!(bench.fake.jobs_of("fixer").iter().filter(|job| job.key.variant == 1).count(), 1);
}

#[test]
fn a_failed_run_resumes_by_trying_the_failed_node_again() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::fail("network down").session("s-1"));
    fake.on("coder", Script::done().summary("Second time lucky."));
    let mut bench = Bench::start_with(line(), fake);
    bench.step();
    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.machine.run().error.as_deref(), Some("Coder failed: network down"));
    assert_eq!(bench.machine.cancel(1), Err(ExecError::RunClosed(RunState::Failed)));

    bench.resume();
    assert_eq!(bench.state(), RunState::Running);
    assert_eq!(bench.machine.run().error, None);
    assert_eq!(bench.queue[0].key, key("coder"));
    assert_eq!(bench.queue[0].resume_session, Some(SessionId::new("s-1")));
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.inst("coder", 0).summary, "Second time lucky.");
}

#[test]
fn resuming_after_every_variant_failed_tries_them_all_again() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    for variant in 1..=2 {
        fake.on_variant("fixer", variant, Script::fail("model refused"));
        fake.on_variant("fixer", variant, Script::done().output(work(["a", "b"][variant as usize - 1], "Fixed.")));
    }
    let mut bench = Bench::start_with(fix_graph(2), fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::Failed);
    assert_eq!(bench.status("tests", 1), NodeStatus::Skipped, "the first failure was tolerated and its path marked dead");

    bench.resume();
    assert_eq!(bench.queued(), ["fixer#1", "fixer#2"]);
    assert!(!bench.has("tests", 1), "the dead path is judged afresh");
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
    assert_eq!(bench.review().variants.len(), 2);
    assert_eq!(bench.inst("tests", 1).attempt, 1);
}

#[test]
fn results_and_notes_without_a_job_are_refused() {
    let mut bench = Bench::start(line());
    let done = || JobResult::Done { port: PORT_OUT.into(), output: NodeOutput::default() };
    let ghost = InstanceKey { node: "ghost".into(), variant: 0, attempt: 1 };
    assert_eq!(bench.machine.job_finished(&ghost, done(), 1), Err(ExecError::UnknownInstance(ghost)));
    let later = InstanceKey { node: "coder".into(), variant: 0, attempt: 2 };
    assert_eq!(bench.machine.job_finished(&later, done(), 1), Err(ExecError::UnknownInstance(later)));
    assert_eq!(bench.machine.job_finished(&key("src"), done(), 1), Err(ExecError::NoJob(key("src"))), "the source is no job");

    bench.step();
    let before = bench.machine.snapshot().clone();
    assert_eq!(bench.machine.job_finished(&key("coder"), done(), 2), Err(ExecError::NoJob(key("coder"))), "a result twice");
    let note = JobNote::Spent { cost_usd: 5.0, usage: Default::default() };
    assert_eq!(bench.machine.note(&key("coder"), note, 3), Err(ExecError::NoJob(key("coder"))), "a note after the end");
    assert_eq!(bench.machine.snapshot(), &before);
}

#[test]
fn a_finished_run_takes_no_more_commands() {
    let mut bench = Bench::start(line());
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.machine.cancel(1), Err(ExecError::RunClosed(RunState::Done)));
    assert_eq!(bench.machine.resume(1), Err(ExecError::NotStopped));
    assert_eq!(bench.machine.raise_budget(1.0, 1), Err(ExecError::RunClosed(RunState::Done)));
    assert_eq!(bench.machine.start(1), Err(ExecError::AlreadyStarted));
    assert!(bench.machine.interrupt(1).is_empty());
}

#[test]
fn a_question_that_arrives_from_a_cancelled_job_is_asked_on_resume() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::ask("Really?"));
    fake.on("coder", Script::done());
    let mut bench = Bench::start_with(line(), fake);
    bench.cancel();
    bench.cancelled.clear();
    bench.step();
    assert_eq!(bench.state(), RunState::Cancelled);
    assert_eq!(bench.status("coder", 0), NodeStatus::Cancelled);

    bench.resume();
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.status("coder", 0), NodeStatus::Waiting);
}

#[test]
fn a_job_stopped_from_outside_interrupts_the_run() {
    let mut bench = Bench::start(graph(
        vec![source("src"), agent("a", 1), agent("b", 1)],
        vec![edge("src", PORT_OUT, "a"), edge("src", PORT_OUT, "b")],
    ));
    let a = InstanceKey { node: "a".into(), variant: 0, attempt: 1 };
    let fx = bench.machine.job_finished(&a, JobResult::Cancelled { output: NodeOutput::default() }, 1).expect("in flight");
    assert_eq!(bench.state(), RunState::Interrupted);
    assert!(fx.iter().any(|effect| matches!(effect, Effect::CancelJob(key) if key.node.as_str() == "b")), "the rest stops too");
    assert_eq!(bench.status("a", 0), NodeStatus::Interrupted);
    assert_eq!(bench.status("b", 0), NodeStatus::Interrupted);
    assert!(!fx.iter().any(|effect| matches!(effect, Effect::Finished(_))), "an interrupted run is not closed");
}
