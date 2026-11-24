//! Questions and budgets: the run waits for a person and goes on when it hears from one.

use super::*;
use crate::exec::Hold;

fn line() -> GraphDef {
    graph(vec![source("src"), agent("coder", 1), command("pr", false)], chain(&["src", "coder", "pr"]))
}

fn key(node: &str, variant: u32, attempt: u32) -> InstanceKey {
    InstanceKey { node: node.into(), variant, attempt }
}

#[test]
fn a_question_makes_the_run_wait_and_the_answer_continues_the_same_instance() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::ask("Which database?").session("s-1").cost(0.2));
    fake.on("coder", Script::done().summary("Used Postgres.").cost(0.3));
    let mut bench = Bench::start_with(line(), fake);
    bench.step();

    assert_eq!(bench.state(), RunState::WaitingAnswer);
    let coder = bench.inst("coder", 0);
    assert_eq!((coder.status, coder.summary.as_str(), coder.finished), (NodeStatus::Waiting, "Which database?", None));
    assert_eq!(bench.machine.holds(), [(key("coder", 0, 1), Hold::Question { text: "Which database?".into() })]);
    assert!(bench.effects.contains(&Effect::Asked { key: key("coder", 0, 1), question: "Which database?".into() }));
    assert!(bench.queue.is_empty());

    let fx = bench.machine.answer(&key("coder", 0, 1), "Postgres.".into(), bench.now + 1).expect("answer");
    bench.apply(fx);
    assert_eq!(bench.state(), RunState::Running);
    let job = bench.queue[0].clone();
    assert_eq!(job.key, key("coder", 0, 1), "the same key");
    assert_eq!(job.answer.as_deref(), Some("Postgres."));
    assert_eq!(job.resume_session, Some(SessionId::new("s-1")), "so the agent keeps its conversation");

    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    let coder = bench.inst("coder", 0);
    assert_eq!((coder.status, coder.summary.as_str(), coder.attempt), (NodeStatus::Done, "Used Postgres.", 1));
    assert!((coder.cost_usd - 0.5).abs() < 1e-9, "both jobs count: {}", coder.cost_usd);
    assert!(bench.machine.holds().is_empty());
    assert_eq!(bench.machine.run().nodes.iter().filter(|instance| instance.node.as_str() == "coder").count(), 1);
}

#[test]
fn an_answer_nobody_asked_for_is_refused() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::ask("Which?"));
    let mut bench = Bench::start_with(line(), fake);

    assert_eq!(bench.machine.answer(&key("coder", 0, 1), "x".into(), 1), Err(ExecError::NotWaiting(key("coder", 0, 1))), "it is still running");
    assert_eq!(bench.machine.answer(&key("ghost", 0, 1), "x".into(), 1), Err(ExecError::UnknownInstance(key("ghost", 0, 1))));
    bench.step();
    bench.machine.answer(&key("coder", 0, 1), "x".into(), 2).expect("answer");
    assert_eq!(bench.machine.answer(&key("coder", 0, 1), "y".into(), 3), Err(ExecError::NotWaiting(key("coder", 0, 1))), "answered already");
}

#[test]
fn one_variant_asking_does_not_stop_the_others() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    fake.on_variant("fixer", 2, Script::ask("Old API or new?"));
    fake.on_variant("fixer", 2, Script::done().output(work("b", "New API.")));
    let mut bench = Bench::start_with(fix_graph(3), fake);
    bench.step();
    bench.step_node("fixer", 2);
    assert_eq!(bench.state(), RunState::WaitingAnswer, "a person is needed, even while two variants work");

    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.status("render", 1), NodeStatus::Done);
    assert!(bench.machine.open_reviews().is_empty(), "the review waits for every variant");

    let fx = bench.machine.answer(&key("fixer", 2, 1), "New.".into(), bench.now + 1).expect("answer");
    bench.apply(fx);
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview);
    assert_eq!(bench.review().variants.len(), 3);
}

/// `src` fans out to `a` and `b`, which join in `pr`; the run may spend one dollar.
fn budgeted() -> GraphDef {
    let mut def = graph(
        vec![source("src"), agent("a", 1), agent("b", 1), command("pr", false)],
        vec![edge("src", PORT_OUT, "a"), edge("src", PORT_OUT, "b"), edge("a", PORT_OUT, "pr"), edge("b", PORT_OUT, "pr")],
    );
    def.limits.budget_usd = Some(1.0);
    def
}

#[test]
fn reaching_the_run_budget_stops_the_jobs_until_it_is_raised() {
    let mut bench = Bench::start(budgeted());
    assert_eq!(bench.queue[0].budget_left_usd, Some(1.0));
    let (a, b) = (key("a", 0, 1), key("b", 0, 1));
    let spent = |usd| JobNote::Spent { cost_usd: usd, usage: Default::default() };
    bench.machine.note(&a, JobNote::Place { session: Some(SessionId::new("s-a")), worktree: None, branch: None }, 1).expect("place");
    let fx = bench.machine.note(&a, spent(0.6), 2).expect("spent");
    assert!(!fx.iter().any(|effect| matches!(effect, Effect::CancelJob(_))), "under the cap");

    let fx = bench.machine.note(&b, spent(0.4), 3).expect("spent");
    let cancels: Vec<&Effect> = fx.iter().filter(|effect| matches!(effect, Effect::CancelJob(_))).collect();
    assert_eq!(cancels, [&Effect::CancelJob(a.clone()), &Effect::CancelJob(b.clone())], "the cap is reached: both are told to stop");
    bench.apply(fx);
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.status("a", 0), NodeStatus::Waiting);
    assert_eq!(bench.machine.holds(), [(a.clone(), Hold::Budget), (b.clone(), Hold::Budget)]);

    // One job stops as told, the other had just finished its work: that work counts.
    bench.step_node("a", 0);
    bench.cancelled.clear();
    bench.step_node("b", 0);
    assert_eq!(bench.status("a", 0), NodeStatus::Waiting);
    assert_eq!(bench.status("b", 0), NodeStatus::Done);
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert!(bench.queue.is_empty(), "nothing new starts over budget");

    assert_eq!(bench.machine.raise_budget(0.0, 5), Err(ExecError::BadAmount));
    let fx = bench.machine.raise_budget(2.0, 6).expect("raise");
    bench.apply(fx);
    assert_eq!(bench.state(), RunState::Running);
    assert_eq!(bench.machine.snapshot().budget_usd, Some(3.0), "two more dollars from what was spent");
    let job = bench.queue[0].clone();
    assert_eq!((job.key.clone(), job.resume_session.clone()), (a, Some(SessionId::new("s-a"))));
    assert_eq!(job.budget_left_usd, Some(2.0));

    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.jobs_of("pr").len(), 1);
}

#[test]
fn a_raise_before_the_cancelled_job_returned_waits_for_it() {
    let mut bench = Bench::start(budgeted());
    let a = key("a", 0, 1);
    let fx = bench.machine.note(&a, JobNote::Spent { cost_usd: 1.5, usage: Default::default() }, 1).expect("spent");
    bench.apply(fx);
    assert_eq!(bench.queue.len(), 2, "both jobs are still winding down");

    let fx = bench.machine.raise_budget(1.0, 2).expect("raise");
    assert!(!fx.iter().any(|effect| matches!(effect, Effect::Start(_))), "a key never has two jobs at once");
    bench.apply(fx);
    assert_eq!(bench.status("a", 0), NodeStatus::Pending);

    bench.step_node("a", 0);
    assert_eq!(bench.queued(), ["b#0", "a#0"], "the old job is back: now the new one starts");
    assert_eq!(bench.status("a", 0), NodeStatus::Running);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
    assert_eq!(bench.jobs_of("a").len(), 2);
}

#[test]
fn a_node_budget_caps_what_its_job_may_spend() {
    let mut def = budgeted();
    def.limits.budget_usd = Some(10.0);
    for node in &mut def.nodes {
        if let (NodeKind::Agent(agent), "a") = (&mut node.kind, node.id.as_str()) {
            agent.budget_usd = Some(0.5);
        }
    }
    let fake = Arc::new(FakeRunner::new());
    fake.on("a", Script::over_budget().spent(0.5).session("s-a"));
    fake.on("a", Script::done().cost(0.1));
    let mut bench = Bench::start_with(def, fake);
    assert_eq!(bench.queue[0].budget_left_usd, Some(0.5), "the smaller of the node's and the run's");
    assert_eq!(bench.queue[1].budget_left_usd, Some(10.0));

    bench.step_node("a", 0);
    assert_eq!(bench.status("a", 0), NodeStatus::Waiting);
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.queued(), ["b#0"], "the other job is not touched by a node's cap");

    let fx = bench.machine.raise_budget(1.0, bench.now + 1).expect("raise");
    bench.apply(fx);
    let job = bench.queue.back().expect("a again").clone();
    assert_eq!(job.key, key("a", 0, 1));
    assert_eq!(job.budget_left_usd, Some(1.0), "the node's cap moved with the raise");
    assert_eq!(job.resume_session, Some(SessionId::new("s-a")));
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}

#[test]
fn without_any_budget_a_job_gets_no_limit() {
    let bench = Bench::start(line());
    assert_eq!(bench.queue[0].budget_left_usd, None);
}

#[test]
fn spending_that_ends_exactly_with_the_last_job_still_finishes_the_run() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("a", Script::done().cost(0.5));
    fake.on("b", Script::done().cost(0.5));
    let mut def = budgeted();
    def.nodes.retain(|node| node.id.as_str() != "pr");
    def.edges.retain(|edge| edge.to.as_str() != "pr");
    let mut bench = Bench::start_with(def, fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done, "nothing is left that would need money");
}

#[test]
fn over_budget_with_work_left_waits_even_when_no_job_is_running() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("a", Script::done().cost(0.5));
    fake.on("b", Script::done().cost(0.5));
    let mut bench = Bench::start_with(budgeted(), fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingAnswer);
    assert_eq!(bench.status("pr", 0), NodeStatus::Pending);
    let fx = bench.machine.raise_budget(0.5, bench.now + 1).expect("raise");
    bench.apply(fx);
    bench.drain();
    assert_eq!(bench.state(), RunState::Done);
}
