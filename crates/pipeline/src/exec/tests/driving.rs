//! The driver: the same machine on real threads, with the fake runner.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use base::time::SystemClock;
use model::{ReviewState, RunLogLine};

use super::*;
use crate::exec::{Driver, DriverHandle, Observer, Snapshot};

/// Long enough for a loaded machine, short enough to fail a hung test.
const WAIT: Duration = Duration::from_secs(20);

#[derive(Debug)]
enum Seen {
    State(RunState),
    Review(Review),
    Asked(InstanceKey, String),
    Finished(Run),
}

/// Forwards what the driver reports to the test and keeps the last snapshot as JSON, the way
/// the engine stores it.
struct Watch {
    seen: Sender<Seen>,
    stored: Arc<Mutex<Option<String>>>,
    logs: Arc<Mutex<Vec<RunLogLine>>>,
}

impl Observer for Watch {
    fn snapshot(&mut self, snapshot: &Snapshot) {
        *self.stored.lock().expect("stored") = Some(serde_json::to_string(snapshot).expect("json"));
    }

    fn run_changed(&mut self, run: &Run) {
        let _ = self.seen.send(Seen::State(run.state));
    }

    fn review_changed(&mut self, review: &Review) {
        let _ = self.seen.send(Seen::Review(review.clone()));
    }

    fn log(&mut self, _run: &RunId, line: &RunLogLine) {
        self.logs.lock().expect("logs").push(line.clone());
    }

    fn asked(&mut self, _run: &RunId, key: &InstanceKey, question: &str) {
        let _ = self.seen.send(Seen::Asked(key.clone(), question.to_string()));
    }

    fn finished(&mut self, run: &Run) {
        let _ = self.seen.send(Seen::Finished(run.clone()));
    }
}

struct Rig {
    driver: Option<Driver>,
    fake: Arc<FakeRunner>,
    seen: Receiver<Seen>,
    stored: Arc<Mutex<Option<String>>>,
    logs: Arc<Mutex<Vec<RunLogLine>>>,
}

impl Rig {
    fn new(graph: GraphDef, fake: Arc<FakeRunner>) -> Rig {
        Rig::drive(Machine::new(graph, run_shell(), item()), fake)
    }

    fn drive(machine: Machine, fake: Arc<FakeRunner>) -> Rig {
        let (tx, seen) = channel();
        let stored = Arc::new(Mutex::new(None));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let watch = Watch { seen: tx, stored: stored.clone(), logs: logs.clone() };
        let driver = Driver::spawn(machine, fake.clone(), Box::new(watch), Arc::new(SystemClock)).expect("driver");
        Rig { driver: Some(driver), fake, seen, stored, logs }
    }

    fn handle(&self) -> DriverHandle {
        self.driver.as_ref().expect("a driver").handle()
    }

    fn stop(&mut self) -> Machine {
        self.driver.take().expect("a driver").stop().expect("stop")
    }

    /// Waits for the first report `pick` accepts.
    fn wait<T>(&self, what: &str, pick: impl Fn(&Seen) -> Option<T>) -> T {
        let end = Instant::now() + WAIT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let seen = self.seen.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            if let Some(found) = pick(&seen) {
                return found;
            }
        }
    }

    fn wait_state(&self, state: RunState) {
        self.wait(&format!("{state:?}"), |seen| matches!(seen, Seen::State(now) if *now == state).then_some(()));
    }

    fn wait_finished(&self) -> Run {
        self.wait("the end of the run", |seen| if let Seen::Finished(run) = seen { Some(run.clone()) } else { None })
    }

    fn wait_review(&self) -> Review {
        self.wait("a review", |seen| match seen {
            Seen::Review(review) if review.state == ReviewState::Open => Some(review.clone()),
            _ => None,
        })
    }

    fn stored(&self) -> Snapshot {
        let text = self.stored.lock().expect("stored").clone().expect("a snapshot was stored");
        serde_json::from_str(&text).expect("snapshot json")
    }
}

fn gated_variants() -> (GraphDef, Arc<FakeRunner>) {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug"));
    for (pos, tag) in ["a", "b", "c"].iter().enumerate() {
        fake.on_variant("fixer", pos as u32 + 1, Script::done().session(&format!("s-{tag}")).gate("fix").output(work(tag, "Fixed.")));
    }
    let mut def = fix_graph(3);
    def.limits.max_parallel = 2;
    (def, fake)
}

#[test]
fn a_run_is_driven_to_its_end_with_notes_and_log_lines() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::done().log("reading the code").spent(0.3).output(work("a", "Fixed.")));
    let mut rig = Rig::new(graph(vec![source("src"), agent("coder", 1), command("pr", false)], chain(&["src", "coder", "pr"])), fake);

    let run = rig.wait_finished();
    assert_eq!(run.state, RunState::Done);
    assert!((run.cost_usd - 0.3).abs() < 1e-9);
    assert_eq!(run.nodes.iter().map(|instance| instance.status).collect::<Vec<_>>(), [NodeStatus::Done; 3]);
    assert!(rig.logs.lock().expect("logs").iter().any(|line| line.text == "reading the code" && line.node == Some("coder".into())));

    let stored = rig.stored();
    assert_eq!(stored.run, run, "the last stored snapshot is the finished run");
    let machine = rig.stop();
    assert_eq!(machine.run().state, RunState::Done, "stopping a finished run changes nothing");
    assert_eq!(rig.fake.running(), 0);
}

#[test]
fn the_driver_keeps_to_max_parallel_and_takes_a_decision_from_another_thread() {
    let (def, fake) = gated_variants();
    let mut rig = Rig::new(def, fake);
    assert!(rig.fake.wait_at_gates(2, WAIT), "two fixers start");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(rig.fake.jobs_of("fixer").len(), 2, "the third waits for a slot");

    rig.fake.release("fix");
    let review = rig.wait_review();
    assert_eq!(review.variants.len(), 3);

    let handle = rig.handle();
    let decided = std::thread::spawn(move || {
        let outcome = handle.decide(review.id.clone(), ReviewDecision::Ship { variant: 3 });
        (outcome, handle.decide(review.id, ReviewDecision::Skip))
    });
    let (first, second) = decided.join().expect("the deciding thread");
    assert_eq!(first, Ok(()));
    assert!(matches!(second, Err(ExecError::ReviewNotOpen(_))), "the machine's refusal comes back: {second:?}");

    let run = rig.wait_finished();
    assert_eq!(run.state, RunState::Done);
    let machine = rig.stop();
    let discarded: Vec<Option<String>> = rig.fake.discards().into_iter().map(|out| out.branch).collect();
    assert_eq!(discarded, [Some("fix/a".to_string()), Some("fix/b".to_string())], "discards ran before the driver let go");
    assert_eq!(machine.jobs_in_flight(), 0);
}

#[test]
fn agents_never_work_more_than_max_parallel_at_once_on_threads() {
    let fake = Arc::new(FakeRunner::new());
    let mut nodes = vec![source("src")];
    let mut edges = Vec::new();
    for id in ["a", "b", "c", "d", "e", "f"] {
        fake.on(id, Script::done().delay(Duration::from_millis(30)));
        nodes.push(agent(id, 1));
        edges.push(edge("src", PORT_OUT, id));
    }
    let mut def = graph(nodes, edges);
    def.limits.max_parallel = 2;
    let mut rig = Rig::new(def, fake);
    assert_eq!(rig.wait_finished().state, RunState::Done);
    assert_eq!(rig.fake.max_running(), 2);
    assert_eq!(rig.fake.jobs().len(), 6);
    rig.stop();
}

#[test]
fn cancel_returns_promptly_and_stop_joins_every_thread() {
    let (def, fake) = gated_variants();
    let mut rig = Rig::new(def, fake);
    assert!(rig.fake.wait_at_gates(2, WAIT));

    let begun = Instant::now();
    rig.handle().cancel().expect("cancel");
    assert!(begun.elapsed() < Duration::from_secs(5), "cancel does not wait for the jobs: {:?}", begun.elapsed());
    let run = rig.wait_finished();
    assert_eq!(run.state, RunState::Cancelled);
    assert_eq!(rig.handle().cancel(), Err(ExecError::RunClosed(RunState::Cancelled)));

    let begun = Instant::now();
    let machine = rig.stop();
    assert!(begun.elapsed() < Duration::from_secs(5), "{:?}", begun.elapsed());
    assert_eq!(rig.fake.running(), 0, "no job thread outlives the driver");
    assert_eq!(machine.jobs_in_flight(), 0, "every cancelled job reported before the loop ended");
    assert_eq!(machine.run().state, RunState::Cancelled);
    assert_eq!(rig.fake.jobs_of("fixer").len(), 2, "the third variant never started");
    assert_eq!(rig.stored().run, *machine.run(), "the last word of each job was stored");
}

#[test]
fn a_cancelled_run_is_resumed_through_the_handle() {
    let (def, fake) = gated_variants();
    let rig = Rig::new(def, fake);
    assert!(rig.fake.wait_at_gates(2, WAIT));
    let handle = rig.handle();
    handle.cancel().expect("cancel");
    rig.wait_finished();
    assert!(rig.fake.wait_idle(WAIT), "the cancelled jobs return");

    rig.fake.release("fix");
    handle.resume().expect("resume");
    let review = rig.wait_review();
    assert_eq!(review.variants.len(), 3);
    let resumed: Vec<Job> = rig.fake.jobs_of("fixer").into_iter().filter(|job| job.resume_session.is_some()).collect();
    assert_eq!(resumed.len(), 2, "the two that were cancelled continue their sessions");
    handle.decide(review.id, ReviewDecision::Skip).expect("skip");
    assert_eq!(rig.wait_finished().state, RunState::Done);
}

#[test]
fn stopping_the_driver_interrupts_the_run_and_a_new_driver_resumes_it_from_the_stored_snapshot() {
    let (def, fake) = gated_variants();
    let mut rig = Rig::new(def.clone(), fake.clone());
    assert!(fake.wait_at_gates(2, WAIT));
    let machine = rig.stop();
    assert_eq!(machine.run().state, RunState::Interrupted);
    assert_eq!(fake.running(), 0);
    assert_eq!(machine.jobs_in_flight(), 0);
    let stored = rig.stored();
    assert_eq!(&stored, machine.snapshot());

    fake.release("fix");
    let rig = Rig::drive(Machine::restore(def, stored), fake.clone());
    assert_eq!(rig.handle().resume(), Ok(()));
    let review = rig.wait_review();
    assert_eq!(review.variants.iter().map(|variant| variant.branch.clone()).collect::<Vec<_>>().len(), 3);
    let again: Vec<u32> = fake.jobs_of("fixer").iter().filter(|job| job.resume_session.is_some()).map(|job| job.key.variant).collect();
    assert_eq!(again.len(), 2, "the interrupted fixers continue their sessions");
    drop(rig);
    assert_eq!(fake.running(), 0, "dropping the driver joins its threads too");
}

#[test]
fn questions_and_budgets_are_answered_through_the_handle() {
    let fake = Arc::new(FakeRunner::new());
    fake.on("coder", Script::ask("Which database?"));
    fake.on("coder", Script::done().spent(1.0).delay(Duration::from_secs(3)));
    fake.on("coder", Script::done());
    let mut def = graph(vec![source("src"), agent("coder", 1), command("pr", false)], chain(&["src", "coder", "pr"]));
    def.limits.budget_usd = Some(1.0);
    let rig = Rig::new(def, fake);
    let handle = rig.handle();

    let (key, question) = rig.wait("a question", |seen| if let Seen::Asked(key, question) = seen { Some((key.clone(), question.clone())) } else { None });
    assert_eq!(question, "Which database?");
    assert_eq!(handle.answer(key.clone(), String::new() + "Postgres."), Ok(()));
    assert_eq!(handle.answer(key, "Again.".into()), Err(ExecError::NotWaiting(InstanceKey { node: "coder".into(), variant: 0, attempt: 1 })));

    // The second job spends the whole budget and is cancelled in its sleep.
    rig.wait_state(RunState::Running);
    rig.wait_state(RunState::WaitingAnswer);
    assert_eq!(handle.raise_budget(-1.0), Err(ExecError::BadAmount));
    assert_eq!(handle.raise_budget(5.0), Ok(()));
    assert_eq!(rig.wait_finished().state, RunState::Done);
    assert_eq!(rig.fake.jobs_of("coder").len(), 3);
    assert_eq!(rig.fake.jobs_of("coder")[1].answer.as_deref(), Some("Postgres."));
}

#[test]
fn a_runner_that_panics_fails_the_run_instead_of_hanging_it() {
    struct Panics;
    impl NodeRunner for Panics {
        fn run(&self, _job: &Job, _sink: &dyn JobSink, _cancel: &CancelToken) -> JobResult {
            panic!("boom");
        }
        fn discard(&self, _run: &RunId, _output: &NodeOutput) {}
    }
    let (tx, seen) = channel();
    let watch = Watch { seen: tx, stored: Arc::default(), logs: Arc::default() };
    let machine = Machine::new(graph(vec![source("src"), agent("a", 1)], chain(&["src", "a"])), run_shell(), item());
    let driver = Driver::spawn(machine, Arc::new(Panics), Box::new(watch), Arc::new(SystemClock)).expect("driver");
    let end = Instant::now() + WAIT;
    let run = loop {
        match seen.recv_timeout(end.saturating_duration_since(Instant::now())).expect("the run ends") {
            Seen::Finished(run) => break run,
            _ => continue,
        }
    };
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.error.as_deref(), Some("A failed: The node runner panicked."));
    driver.stop().expect("stop");
}
