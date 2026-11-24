//! Runs a [`Machine`] on real threads.
//!
//! One loop thread owns the machine. Every started job gets a worker thread that calls the
//! [`NodeRunner`] and reports back through a channel; commands from other threads arrive on
//! the same channel, so the machine is only ever touched by the loop. Stopping the driver
//! cancels the jobs, waits for every worker and returns the machine.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};
use base::cancel::CancelToken;
use base::time::Clock;
use model::{Review, ReviewDecision, ReviewId, Run, RunId, RunLogLine};

use super::state::Snapshot;
use super::{Effect, ExecError, Machine};
use crate::api::{InstanceKey, JobNote, JobResult, JobSink, NodeOutput, NodeRunner};

/// What the engine hears from a run. Called on the run's loop thread, one call at a time; the
/// loop waits for each call, so `snapshot` may write to the store before the run goes on.
/// A call must not wait on the run's own [`DriverHandle`]: the loop cannot answer while it is
/// inside the observer.
pub trait Observer: Send {
    /// The machine's state after a step that changed it, before any other call about that
    /// step. Store it to continue after a restart.
    fn snapshot(&mut self, _snapshot: &Snapshot) {}
    /// The run's state or instances changed.
    fn run_changed(&mut self, _run: &Run) {}
    /// A review opened or closed; `review.state` says which.
    fn review_changed(&mut self, _review: &Review) {}
    /// A line for the run's log, from the machine or from a job.
    fn log(&mut self, _run: &RunId, _line: &RunLogLine) {}
    /// An agent waits for a person's answer.
    fn asked(&mut self, _run: &RunId, _key: &InstanceKey, _question: &str) {}
    /// The run reached `Done`, `Failed` or `Cancelled`.
    fn finished(&mut self, _run: &Run) {}
}

enum Command {
    Decide(ReviewId, ReviewDecision),
    Answer(InstanceKey, String),
    RaiseBudget(f64),
    Cancel,
    Resume,
}

enum Msg {
    Command(Command, Sender<Result<(), ExecError>>),
    Note(InstanceKey, JobNote),
    Finished(InstanceKey, JobResult),
    Stop,
}

/// Sends a person's commands to a running driver from any thread. Each call returns once the
/// machine has taken the command, with the machine's refusal when it did not.
#[derive(Clone)]
pub struct DriverHandle {
    tx: Sender<Msg>,
}

impl DriverHandle {
    /// Decides an open review.
    pub fn decide(&self, review: ReviewId, decision: ReviewDecision) -> Result<(), ExecError> {
        self.call(Command::Decide(review, decision))
    }

    /// Answers the question of a waiting instance.
    pub fn answer(&self, key: InstanceKey, text: String) -> Result<(), ExecError> {
        self.call(Command::Answer(key, text))
    }

    /// Allows `usd` more to be spent from now.
    pub fn raise_budget(&self, usd: f64) -> Result<(), ExecError> {
        self.call(Command::RaiseBudget(usd))
    }

    /// Cancels the run. Returns as soon as the jobs were told to stop, not when they have.
    pub fn cancel(&self) -> Result<(), ExecError> {
        self.call(Command::Cancel)
    }

    /// Continues a cancelled, failed or interrupted run.
    pub fn resume(&self) -> Result<(), ExecError> {
        self.call(Command::Resume)
    }

    fn call(&self, command: Command) -> Result<(), ExecError> {
        let (reply, answer) = channel();
        self.tx.send(Msg::Command(command, reply)).map_err(|_| ExecError::DriverStopped)?;
        answer.recv().map_err(|_| ExecError::DriverStopped)?
    }
}

/// A machine being driven. Dropping it stops the run's jobs and waits for its threads.
pub struct Driver {
    handle: DriverHandle,
    thread: Option<JoinHandle<Machine>>,
}

impl Driver {
    /// Starts driving `machine`: a fresh one is started, a restored one waits for
    /// [`DriverHandle::resume`] or a decision.
    pub fn spawn(
        machine: Machine,
        runner: Arc<dyn NodeRunner>,
        observer: Box<dyn Observer>,
        clock: Arc<dyn Clock>,
    ) -> Result<Driver> {
        let (tx, rx) = channel();
        let name = format!("run-{}", machine.run().id);
        let driving = Loop {
            run: machine.run().id.clone(),
            machine,
            runner,
            observer,
            clock,
            tx: tx.clone(),
            rx,
            workers: HashMap::new(),
            discards: None,
            stopping: false,
            stored_log_seq: 0,
        };
        let thread = thread::Builder::new().name(name).spawn(move || driving.run()).context("starting the run's thread")?;
        Ok(Driver { handle: DriverHandle { tx }, thread: Some(thread) })
    }

    /// A handle for sending commands; clone it freely.
    pub fn handle(&self) -> DriverHandle {
        self.handle.clone()
    }

    /// Stops driving: running jobs are cancelled and the run becomes `Interrupted` (a run that
    /// only waits for a person stays as it is). Returns the machine once every thread is gone.
    pub fn stop(mut self) -> Result<Machine> {
        let thread = self.thread.take().ok_or_else(|| anyhow!("the run's thread is already gone"))?;
        // The loop holds a sender itself, so this only fails when it is gone.
        let _ = self.handle.tx.send(Msg::Stop);
        thread.join().map_err(|_| anyhow!("the run's thread panicked"))
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.handle.tx.send(Msg::Stop);
            let _ = thread.join();
        }
    }
}

struct Worker {
    cancel: CancelToken,
    thread: JoinHandle<()>,
}

struct Sink {
    key: InstanceKey,
    tx: Sender<Msg>,
}

impl JobSink for Sink {
    fn note(&self, note: JobNote) {
        let _ = self.tx.send(Msg::Note(self.key.clone(), note));
    }
}

struct Loop {
    run: RunId,
    machine: Machine,
    runner: Arc<dyn NodeRunner>,
    observer: Box<dyn Observer>,
    clock: Arc<dyn Clock>,
    /// Cloned into every worker; also keeps `rx` open while the loop lives.
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    workers: HashMap<InstanceKey, Worker>,
    /// The thread doing `discard` calls, one after the other, started on first use.
    discards: Option<(Sender<NodeOutput>, JoinHandle<()>)>,
    stopping: bool,
    /// `log_seq` of the last snapshot handed to the observer.
    stored_log_seq: u64,
}

impl Loop {
    fn run(mut self) -> Machine {
        if !self.machine.snapshot().started {
            let now = self.clock.now_ms();
            if let Ok(fx) = self.machine.start(now) {
                self.apply(fx, true);
            }
        }
        while !(self.stopping && self.workers.is_empty()) {
            let Ok(msg) = self.rx.recv() else { break };
            let now = self.clock.now_ms();
            match msg {
                Msg::Stop => {
                    if !self.stopping {
                        self.stopping = true;
                        let fx = self.machine.interrupt(now);
                        self.apply(fx, false);
                        for worker in self.workers.values() {
                            worker.cancel.cancel();
                        }
                    }
                }
                Msg::Command(command, reply) => {
                    let outcome = if self.stopping { Err(ExecError::DriverStopped) } else { self.command(command, now) };
                    // A command can change what no effect shows (a budget raised on a run that
                    // was not waiting for it), so its outcome is always stored.
                    let _ = reply.send(outcome.map(|fx| self.apply(fx, true)));
                }
                Msg::Note(key, note) => {
                    // A note of a job the machine no longer waits for is dropped.
                    if let Ok(fx) = self.machine.note(&key, note, now) {
                        self.apply(fx, false);
                    }
                }
                Msg::Finished(key, result) => {
                    if let Some(worker) = self.workers.remove(&key) {
                        // The message is the last thing a worker does.
                        let _ = worker.thread.join();
                    }
                    if let Ok(fx) = self.machine.job_finished(&key, result, now) {
                        // Even a result that moves nothing leaves the job's output behind.
                        self.apply(fx, true);
                    }
                }
            }
        }
        if let Some((queue, thread)) = self.discards.take() {
            // Closing the queue lets the thread finish what is in it and end.
            drop(queue);
            let _ = thread.join();
        }
        self.machine
    }

    fn command(&mut self, command: Command, now: i64) -> Result<Vec<Effect>, ExecError> {
        match command {
            Command::Decide(review, decision) => self.machine.decide(&review, decision, now),
            Command::Answer(key, text) => self.machine.answer(&key, text, now),
            Command::RaiseBudget(usd) => self.machine.raise_budget(usd, now),
            Command::Cancel => self.machine.cancel(now),
            Command::Resume => self.machine.resume(now),
        }
    }

    /// Stores the machine's state and remembers the log numbers that are now safe to use.
    fn store(&mut self) {
        self.stored_log_seq = self.machine.snapshot().log_seq;
        self.observer.snapshot(self.machine.snapshot());
    }

    /// Carries out the effects of one step and whatever they lead to. The state is stored
    /// before anyone hears about it, so what the engine publishes is never ahead of the store.
    /// It is stored when the step changed it: `changed` says so for steps whose effects may
    /// not show it; a step that only logged changes it when it reserved new log numbers.
    fn apply(&mut self, fx: Vec<Effect>, changed: bool) {
        let logged_only = fx.iter().all(|effect| matches!(effect, Effect::Log(_)));
        if changed || !logged_only || self.machine.snapshot().log_seq != self.stored_log_seq {
            self.store();
        }
        let mut queue: VecDeque<Effect> = fx.into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Start(job) => {
                    let key = job.key.clone();
                    // Once stopping nothing new starts; the machine hears that the job was
                    // stopped, like any job the stop cancels.
                    let result = if self.stopping {
                        Some(JobResult::Cancelled { output: NodeOutput::default() })
                    } else {
                        self.start_worker(*job).err().map(|error| JobResult::Failed { error: format!("{error:#}"), output: NodeOutput::default() })
                    };
                    if let Some(result) = result
                        && let Ok(more) = self.machine.job_finished(&key, result, self.clock.now_ms())
                    {
                        self.store();
                        queue.extend(more);
                    }
                }
                Effect::CancelJob(key) => {
                    if let Some(worker) = self.workers.get(&key) {
                        worker.cancel.cancel();
                    }
                }
                Effect::ReviewOpened(review) | Effect::ReviewClosed(review) => self.observer.review_changed(&review),
                Effect::Run(run) => self.observer.run_changed(&run),
                Effect::Log(line) => self.observer.log(&self.run, &line),
                Effect::Asked { key, question } => self.observer.asked(&self.run, &key, &question),
                Effect::Discard(output) => self.discard(*output),
                Effect::Finished(_) => self.observer.finished(self.machine.run()),
            }
        }
    }

    fn start_worker(&mut self, job: crate::api::Job) -> Result<()> {
        let key = job.key.clone();
        if let Some(old) = self.workers.remove(&key) {
            // The machine starts a key again only after its last job reported, so this thread
            // is at its end.
            let _ = old.thread.join();
        }
        let cancel = CancelToken::new();
        let (token, runner, tx) = (cancel.clone(), self.runner.clone(), self.tx.clone());
        let name = format!("job-{}-{}", key.node, key.variant);
        let thread = thread::Builder::new()
            .name(name)
            .spawn(move || {
                let sink = Sink { key: job.key.clone(), tx: tx.clone() };
                // A runner must not panic; if it does, the run must still hear that the job ended.
                let result = catch_unwind(AssertUnwindSafe(|| runner.run(&job, &sink, &token))).unwrap_or_else(|_| {
                    JobResult::Failed { error: "The node runner panicked.".to_string(), output: NodeOutput::default() }
                });
                let _ = tx.send(Msg::Finished(job.key.clone(), result));
            })
            .context("starting a job thread")?;
        self.workers.insert(key, Worker { cancel, thread });
        Ok(())
    }

    /// Discards off the loop, so that a slow `git worktree remove` does not hold up commands,
    /// and one at a time, so that two of them never work on the same repository at once.
    fn discard(&mut self, output: NodeOutput) {
        if self.discards.is_none() {
            let (queue, outputs) = channel::<NodeOutput>();
            let (runner, run) = (self.runner.clone(), self.run.clone());
            let spawned = thread::Builder::new().name("discard".to_string()).spawn(move || {
                for output in outputs {
                    // Best effort: a discard that panics must not take the later ones with it.
                    let _ = catch_unwind(AssertUnwindSafe(|| runner.discard(&run, &output)));
                }
            });
            match spawned {
                Ok(thread) => self.discards = Some((queue, thread)),
                // Best effort, as the contract says: without a thread it is done here.
                Err(_) => {
                    let _ = catch_unwind(AssertUnwindSafe(|| self.runner.discard(&self.run, &output)));
                    return;
                }
            }
        }
        if let Some((queue, _)) = &self.discards {
            let _ = queue.send(output);
        }
    }
}
