//! A scripted [`NodeRunner`] for tests of this crate and of the engine.
//!
//! Nothing is run: each job gets the [`Script`] set for its node (or for one variant or one
//! attempt of it), plays the script's notes, delays and gates, and ends with its result. Jobs
//! without a script are done at once, through `else` for a router and `out` for everything
//! else. The runner records the jobs it was given and the outputs it was told to discard.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base::cancel::CancelToken;
use model::graph::{PORT_ELSE, PORT_OUT};
use model::{LogLevel, NodeId, NodeKind, RunId, SessionId, Usage};

use crate::api::{Job, JobNote, JobResult, JobSink, NodeOutput, NodeRunner};

/// How often a job waiting at a gate looks at its cancel token.
const GATE_POLL: Duration = Duration::from_millis(5);

#[derive(Clone, Debug)]
enum Step {
    Note(JobNote),
    Delay(Duration),
    Busy(Duration),
    Gate(String),
}

/// What one job does: steps in order, then a result.
#[derive(Clone, Debug)]
pub struct Script {
    steps: Vec<Step>,
    result: JobResult,
}

impl Script {
    /// Ends with this result.
    pub fn result(result: JobResult) -> Script {
        Script { steps: Vec::new(), result }
    }

    /// Done through `out`.
    pub fn done() -> Script {
        Script::port(PORT_OUT)
    }

    /// Done through `port`: a router's rule id, or `else`.
    pub fn port(port: &str) -> Script {
        Script::result(JobResult::Done { port: port.to_string(), output: NodeOutput::default() })
    }

    /// Stops to ask a person.
    pub fn ask(question: &str) -> Script {
        Script::result(JobResult::Ask { question: question.to_string(), output: NodeOutput::default() })
    }

    /// Fails with `error`.
    pub fn fail(error: &str) -> Script {
        Script::result(JobResult::Failed { error: error.to_string(), output: NodeOutput::default() })
    }

    /// Stops because its budget ran out.
    pub fn over_budget() -> Script {
        Script::result(JobResult::OverBudget { output: NodeOutput::default() })
    }

    /// Replaces the output of the result.
    pub fn output(mut self, output: NodeOutput) -> Script {
        *self.output_mut() = output;
        self
    }

    /// Sets the summary of the result's output.
    pub fn summary(mut self, summary: &str) -> Script {
        self.output_mut().summary = summary.to_string();
        self
    }

    /// Sets the cost in the result's output.
    pub fn cost(mut self, usd: f64) -> Script {
        self.output_mut().cost_usd = usd;
        self
    }

    /// Sends a note before the next step.
    pub fn note(mut self, note: JobNote) -> Script {
        self.steps.push(Step::Note(note));
        self
    }

    /// Reports `usd` spent, as a note.
    pub fn spent(self, usd: f64) -> Script {
        self.note(JobNote::Spent { cost_usd: usd, usage: Usage::default() })
    }

    /// Reports the session the job works in, as a note.
    pub fn session(self, session: &str) -> Script {
        self.note(JobNote::Place { session: Some(SessionId::new(session)), worktree: None, branch: None })
    }

    /// Writes a line to the run's log.
    pub fn log(self, text: &str) -> Script {
        self.note(JobNote::Log { level: LogLevel::Info, text: text.to_string() })
    }

    /// Sleeps; ends as cancelled when the token fires meanwhile.
    pub fn delay(mut self, time: Duration) -> Script {
        self.steps.push(Step::Delay(time));
        self
    }

    /// Works for `time` without looking at the cancel token, like a job stuck in a long call.
    pub fn busy(mut self, time: Duration) -> Script {
        self.steps.push(Step::Busy(time));
        self
    }

    /// Blocks until [`FakeRunner::release`] opens the gate `name`, or the token fires.
    pub fn gate(mut self, name: &str) -> Script {
        self.steps.push(Step::Gate(name.to_string()));
        self
    }

    fn output_mut(&mut self) -> &mut NodeOutput {
        match &mut self.result {
            JobResult::Done { output, .. }
            | JobResult::Ask { output, .. }
            | JobResult::OverBudget { output }
            | JobResult::Failed { output, .. }
            | JobResult::Cancelled { output } => output,
        }
    }
}

struct Rule {
    node: NodeId,
    variant: Option<u32>,
    attempt: Option<u32>,
    /// Played front to back; the last one stays for every later job.
    scripts: VecDeque<Script>,
}

impl Rule {
    fn matches(&self, job: &Job) -> bool {
        self.node == job.key.node
            && self.variant.is_none_or(|variant| variant == job.key.variant)
            && self.attempt.is_none_or(|attempt| attempt == job.key.attempt)
    }

    fn weight(&self) -> u8 {
        u8::from(self.variant.is_some()) + 2 * u8::from(self.attempt.is_some())
    }
}

#[derive(Default)]
struct State {
    rules: Vec<Rule>,
    jobs: Vec<Job>,
    discards: Vec<NodeOutput>,
    open: BTreeSet<String>,
    all_open: bool,
    running: usize,
    max_running: usize,
    at_gate: usize,
}

/// The scripted runner. Share it in an `Arc`; every method takes `&self`.
#[derive(Default)]
pub struct FakeRunner {
    state: Mutex<State>,
    changed: Condvar,
}

impl FakeRunner {
    /// A runner with no scripts: every job is done at once.
    pub fn new() -> FakeRunner {
        FakeRunner::default()
    }

    /// Scripts every job of `node`. Calling it again queues another script: jobs take them in
    /// order and the last one repeats.
    pub fn on(&self, node: &str, script: Script) -> &Self {
        self.add(node, None, None, script)
    }

    /// Scripts the jobs of one variant of `node`; wins over [`Self::on`].
    pub fn on_variant(&self, node: &str, variant: u32, script: Script) -> &Self {
        self.add(node, Some(variant), None, script)
    }

    /// Scripts the jobs of one variant and attempt of `node`; wins over the other two.
    pub fn on_key(&self, node: &str, variant: u32, attempt: u32, script: Script) -> &Self {
        self.add(node, Some(variant), Some(attempt), script)
    }

    fn add(&self, node: &str, variant: Option<u32>, attempt: Option<u32>, script: Script) -> &Self {
        let mut state = self.lock();
        let node = NodeId::new(node);
        let rule = state.rules.iter_mut().find(|rule| rule.node == node && rule.variant == variant && rule.attempt == attempt);
        match rule {
            Some(rule) => rule.scripts.push_back(script),
            None => state.rules.push(Rule { node, variant, attempt, scripts: VecDeque::from([script]) }),
        }
        self
    }

    /// Every job handed to the runner, in the order they began.
    pub fn jobs(&self) -> Vec<Job> {
        self.lock().jobs.clone()
    }

    /// The jobs of one node, in the order they began.
    pub fn jobs_of(&self, node: &str) -> Vec<Job> {
        self.lock().jobs.iter().filter(|job| job.key.node.as_str() == node).cloned().collect()
    }

    /// Every output `discard` was called with.
    pub fn discards(&self) -> Vec<NodeOutput> {
        self.lock().discards.clone()
    }

    /// Opens the gate `name` for the jobs waiting at it and for those that reach it later.
    pub fn release(&self, name: &str) {
        self.lock().open.insert(name.to_string());
        self.changed.notify_all();
    }

    /// Opens every gate, now and later.
    pub fn release_all(&self) {
        self.lock().all_open = true;
        self.changed.notify_all();
    }

    /// Jobs inside `run` right now.
    pub fn running(&self) -> usize {
        self.lock().running
    }

    /// The most jobs that were inside `run` at the same time.
    pub fn max_running(&self) -> usize {
        self.lock().max_running
    }

    /// Waits until `count` jobs were handed to the runner. False when `timeout` passes first.
    pub fn wait_for_jobs(&self, count: usize, timeout: Duration) -> bool {
        self.wait(timeout, |state| state.jobs.len() >= count)
    }

    /// Waits until `count` jobs are blocked at gates. False when `timeout` passes first.
    pub fn wait_at_gates(&self, count: usize, timeout: Duration) -> bool {
        self.wait(timeout, |state| state.at_gate >= count)
    }

    /// Waits until no job is inside `run`. False when `timeout` passes first.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.wait(timeout, |state| state.running == 0)
    }

    fn wait(&self, timeout: Duration, ready: impl Fn(&State) -> bool) -> bool {
        let end = Instant::now() + timeout;
        let mut state = self.lock();
        while !ready(&state) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self.changed.wait_timeout(state, left).map(|(guard, _)| guard).unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
        true
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A test that panicked while holding the lock must not hide its own message.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn script_for(state: &mut State, job: &Job) -> Script {
        let rule = state.rules.iter_mut().filter(|rule| rule.matches(job)).max_by_key(|rule| rule.weight());
        match rule {
            Some(rule) if rule.scripts.len() > 1 => rule.scripts.pop_front().unwrap_or_else(Script::done),
            Some(rule) => rule.scripts.front().cloned().unwrap_or_else(Script::done),
            None => {
                let port = if matches!(job.node.kind, NodeKind::Router(_)) { PORT_ELSE } else { PORT_OUT };
                Script::port(port).summary(&format!("{} done", job.node.name))
            }
        }
    }

    /// Blocks at gate `name`. False when the token fired first.
    fn pass_gate(&self, name: &str, cancel: &CancelToken) -> bool {
        let mut state = self.lock();
        state.at_gate += 1;
        self.changed.notify_all();
        let passed = loop {
            // A cancel wins over a gate that opens at the same moment.
            if cancel.is_cancelled() {
                break false;
            }
            if state.all_open || state.open.contains(name) {
                break true;
            }
            state = self.changed.wait_timeout(state, GATE_POLL).map(|(guard, _)| guard).unwrap_or_else(|poisoned| poisoned.into_inner().0);
        };
        state.at_gate -= 1;
        passed
    }
}

impl NodeRunner for FakeRunner {
    fn run(&self, job: &Job, sink: &dyn JobSink, cancel: &CancelToken) -> JobResult {
        let script = {
            let mut state = self.lock();
            state.jobs.push(job.clone());
            state.running += 1;
            state.max_running = state.max_running.max(state.running);
            Self::script_for(&mut state, job)
        };
        self.changed.notify_all();
        let mut cancelled = cancel.is_cancelled();
        for step in &script.steps {
            if cancelled {
                break;
            }
            match step {
                Step::Note(note) => sink.note(note.clone()),
                Step::Delay(time) => cancelled = !cancel.sleep(*time),
                Step::Busy(time) => std::thread::sleep(*time),
                Step::Gate(name) => cancelled = !self.pass_gate(name, cancel),
            }
        }
        self.lock().running -= 1;
        self.changed.notify_all();
        if cancelled { JobResult::Cancelled { output: NodeOutput::default() } } else { script.result }
    }

    fn discard(&self, _run: &RunId, output: &NodeOutput) {
        self.lock().discards.push(output.clone());
        self.changed.notify_all();
    }
}
