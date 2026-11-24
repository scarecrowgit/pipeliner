//! Tests of the executor. Most drive a [`Machine`] by hand on one thread: started jobs wait in
//! a queue and the test decides which one ends when, with the scripted [`FakeRunner`] saying
//! how. `driving` runs the same machine on real threads.

mod basics;
mod driving;
mod edges;
mod restore;
mod reviews;
mod rounds;
mod stops;
mod storm;
mod waits;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base::cancel::CancelToken;
use model::graph::{PORT_APPROVED, PORT_ELSE, PORT_OUT};
use model::{
    ActionNode, AgentNode, BlobId, EdgeDef, EdgeKind, FileChange, GraphDef, Limits, Millis, NodeDef, NodeId, NodeInstance,
    NodeKind, NodeStatus, PipelineId, Pos, Render, Review, ReviewDecision, ReviewNode, RouterNode, RouterRule, Run, RunId,
    RunState, SessionId, SourceKind, SourceNode, TestResult, GRAPH_SCHEMA,
};

use super::{Effect, ExecError, Machine};
use crate::api::{InstanceKey, Item, Job, JobNote, JobResult, JobSink, NodeOutput, NodeRunner};
use crate::fake::{FakeRunner, Script};

// ---------------------------------------------------------------- graphs

fn node(id: &str, name: &str, kind: NodeKind) -> NodeDef {
    NodeDef { id: id.into(), name: name.into(), pos: Pos::default(), kind }
}

fn source(id: &str) -> NodeDef {
    node(id, "Issues", NodeKind::Source(SourceNode::default()))
}

fn router(id: &str, rules: &[&str]) -> NodeDef {
    let rules = rules.iter().map(|rule| RouterRule { id: rule.to_string(), label: rule.to_string(), ..Default::default() }).collect();
    node(id, "Triage", NodeKind::Router(RouterNode { rules }))
}

fn agent(id: &str, variants: u32) -> NodeDef {
    node(id, &title(id), NodeKind::Agent(AgentNode { variants, workdir: "code".into(), ..Default::default() }))
}

fn command(id: &str, tests: bool) -> NodeDef {
    let action = ActionNode::Command { workdir: "code".into(), command: "make".into(), tests, parse: String::new(), timeout_secs: 0 };
    node(id, &title(id), NodeKind::Action(action))
}

fn render(id: &str) -> NodeDef {
    node(id, "Render", NodeKind::Action(ActionNode::Render { workdir: "code".into(), files: "*.html".into() }))
}

fn review(id: &str, send_back_to: Option<&str>) -> NodeDef {
    node(id, "Review", NodeKind::Review(ReviewNode { send_back_to: send_back_to.map(NodeId::from), skip_if_empty: false }))
}

fn title(id: &str) -> String {
    let mut chars = id.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn edge(from: &str, port: &str, to: &str) -> EdgeDef {
    EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind: EdgeKind::Route }
}

/// `a > b > c`: route edges through `out`.
fn chain(ids: &[&str]) -> Vec<EdgeDef> {
    ids.windows(2).map(|pair| edge(pair[0], PORT_OUT, pair[1])).collect()
}

fn graph(nodes: Vec<NodeDef>, edges: Vec<EdgeDef>) -> GraphDef {
    GraphDef {
        schema: GRAPH_SCHEMA,
        id: PipelineId::new("test"),
        name: "Test".into(),
        description: String::new(),
        limits: Limits { max_parallel: 8, budget_usd: None, max_loops: 5 },
        workdirs: Vec::new(),
        nodes,
        edges,
    }
}

/// `src > triage`, rule `bug` to `fixer ×variants > tests > render > review > merge > pr`,
/// `else` to `report`. The review sends back to the fixer.
fn fix_graph(variants: u32) -> GraphDef {
    let mut edges = vec![edge("src", PORT_OUT, "triage"), edge("triage", "bug", "fixer"), edge("triage", PORT_ELSE, "report")];
    edges.extend(chain(&["fixer", "tests", "render", "review"]));
    edges.push(edge("review", PORT_APPROVED, "merge"));
    edges.extend(chain(&["merge", "pr"]));
    graph(
        vec![
            source("src"),
            router("triage", &["bug"]),
            agent("fixer", variants),
            command("tests", true),
            render("render"),
            review("review", Some("fixer")),
            node("merge", "Merge", NodeKind::Action(ActionNode::Merge { workdir: "code".into() })),
            node("pr", "Pull request", NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true })),
            node("report", "Report", NodeKind::Action(ActionNode::Report)),
        ],
        edges,
    )
}

fn run_shell() -> Run {
    Run {
        id: RunId::new("run-1"),
        pipeline: PipelineId::new("test"),
        version: 3,
        issue: Some("HAR-7".into()),
        number: 1,
        title: "Double charge on retry".into(),
        input: "Charges twice.".into(),
        state: RunState::Queued,
        test: false,
        started: 0,
        finished: None,
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: None,
        nodes: Vec::new(),
    }
}

fn item() -> Item {
    Item { title: "Double charge on retry".into(), text: "Charges twice.".into(), issue: None, source: SourceKind::Issue }
}

// ---------------------------------------------------------------- outputs

fn summary(text: &str) -> NodeOutput {
    NodeOutput { summary: text.into(), ..Default::default() }
}

/// What an agent leaves in its own worktree: `/wt/<tag>` on branch `fix/<tag>`.
fn work(tag: &str, text: &str) -> NodeOutput {
    NodeOutput {
        summary: text.into(),
        session: Some(SessionId::new(format!("s-{tag}"))),
        worktree: Some(PathBuf::from(format!("/wt/{tag}"))),
        branch: Some(format!("fix/{tag}")),
        files: vec![FileChange { path: "src/retry.rs".into(), added: 12, removed: 3 }],
        ..Default::default()
    }
}

fn tested(tag: &str, passed: u32, total: u32) -> NodeOutput {
    NodeOutput {
        summary: format!("{passed} of {total} passed"),
        worktree: Some(PathBuf::from(format!("/wt/{tag}"))),
        tests: Some(TestResult { passed, total }),
        ..Default::default()
    }
}

fn rendered(tag: &str) -> NodeOutput {
    let render = Render { path: format!("{tag}.html"), before: None, after: BlobId::new(format!("blob-{tag}")), width: 800, height: 600 };
    NodeOutput { summary: "1 render".into(), renders: vec![render], ..Default::default() }
}

/// Scripts the fixer, its tests and its render for variants `a`, `b`, `c`… of one round.
fn script_variants(fake: &FakeRunner, attempt: u32, tags: &[&str]) {
    for (pos, tag) in tags.iter().enumerate() {
        let variant = if tags.len() == 1 { 0 } else { pos as u32 + 1 };
        fake.on_key("fixer", variant, attempt, Script::done().output(work(tag, &format!("Approach {tag}. With care."))));
        fake.on_key("tests", variant, attempt, Script::done().output(tested(tag, 10, 10)));
        fake.on_key("render", variant, attempt, Script::done().output(rendered(tag)));
    }
}

// ---------------------------------------------------------------- the bench

#[derive(Default)]
struct Notes(Mutex<Vec<JobNote>>);

impl JobSink for Notes {
    fn note(&self, note: JobNote) {
        self.0.lock().expect("notes").push(note);
    }
}

/// A machine with its started jobs held in a queue, and everything it ever asked for.
struct Bench {
    machine: Machine,
    fake: Arc<FakeRunner>,
    /// Jobs the machine started that have not ended.
    queue: VecDeque<Job>,
    /// Keys whose jobs the machine asked to cancel; they end as cancelled.
    cancelled: Vec<InstanceKey>,
    effects: Vec<Effect>,
    now: Millis,
    /// Time between inputs; 0 freezes the clock.
    tick: Millis,
}

impl Bench {
    /// Starts a run of `graph` with an unscripted runner.
    fn start(graph: GraphDef) -> Bench {
        Bench::start_with(graph, Arc::new(FakeRunner::new()))
    }

    fn start_with(graph: GraphDef, fake: Arc<FakeRunner>) -> Bench {
        let mut bench = Bench::idle(Machine::new(graph, run_shell(), item()), fake);
        let now = bench.tick();
        let fx = bench.machine.start(now).expect("start");
        bench.apply(fx);
        bench
    }

    fn idle(machine: Machine, fake: Arc<FakeRunner>) -> Bench {
        Bench { machine, fake, queue: VecDeque::new(), cancelled: Vec::new(), effects: Vec::new(), now: 1_000_000, tick: 1000 }
    }

    fn tick(&mut self) -> Millis {
        self.now += self.tick;
        self.now
    }

    fn apply(&mut self, fx: Vec<Effect>) {
        for effect in &fx {
            match effect {
                Effect::Start(job) => self.queue.push_back((**job).clone()),
                // Asking twice is allowed: a token only fires once.
                Effect::CancelJob(key) if !self.cancelled.contains(key) => self.cancelled.push(key.clone()),
                Effect::Discard(output) => self.fake.discard(&RunId::new("run-1"), output),
                _ => {}
            }
        }
        self.effects.extend(fx);
    }

    /// Ends the queued job at `pos` the way the fake scripts it, or as cancelled when the
    /// machine asked for that.
    fn end_at(&mut self, pos: usize) {
        let job = self.queue.remove(pos).expect("a queued job");
        let notes = Notes::default();
        let mut result = self.fake.run(&job, &notes, &CancelToken::new());
        for note in notes.0.into_inner().expect("notes") {
            let now = self.tick();
            if let Ok(fx) = self.machine.note(&job.key, note, now) {
                self.apply(fx);
            }
        }
        if let Some(pos) = self.cancelled.iter().position(|key| key == &job.key) {
            self.cancelled.remove(pos);
            result = JobResult::Cancelled { output: NodeOutput::default() };
        }
        let now = self.tick();
        let fx = self.machine.job_finished(&job.key, result, now).expect("the job was in flight");
        self.apply(fx);
    }

    /// Ends the oldest queued job. False when nothing is queued.
    fn step(&mut self) -> bool {
        if self.queue.is_empty() {
            return false;
        }
        self.end_at(0);
        true
    }

    /// Ends the queued job of `node` in `variant`.
    fn step_node(&mut self, node: &str, variant: u32) {
        let pos = self.queue.iter().position(|job| job.key.node.as_str() == node && job.key.variant == variant);
        self.end_at(pos.unwrap_or_else(|| panic!("no queued job for {node} #{variant}: {:?}", self.queued())));
    }

    /// Ends jobs, oldest first, until none is queued.
    fn drain(&mut self) {
        while self.step() {}
    }

    /// Queued jobs as `node#variant`.
    fn queued(&self) -> Vec<String> {
        self.queue.iter().map(|job| format!("{}#{}", job.key.node, job.key.variant)).collect()
    }

    fn state(&self) -> RunState {
        self.machine.run().state
    }

    /// The latest instance of `node` in `variant`.
    fn inst(&self, node: &str, variant: u32) -> &NodeInstance {
        self.machine.run().instance(&node.into(), variant).unwrap_or_else(|| panic!("no instance of {node} #{variant}"))
    }

    fn status(&self, node: &str, variant: u32) -> NodeStatus {
        self.inst(node, variant).status
    }

    fn has(&self, node: &str, variant: u32) -> bool {
        self.machine.run().instance(&node.into(), variant).is_some()
    }

    /// The one open review.
    fn review(&self) -> Review {
        let open = self.machine.open_reviews();
        assert_eq!(open.len(), 1, "open reviews: {open:?}");
        open[0].clone()
    }

    fn decide(&mut self, decision: ReviewDecision) -> Result<(), ExecError> {
        let id = self.review().id;
        let now = self.tick();
        let fx = self.machine.decide(&id, decision, now)?;
        self.apply(fx);
        Ok(())
    }

    fn ship(&mut self, variant: u32) {
        self.decide(ReviewDecision::Ship { variant }).expect("ship");
    }

    fn cancel(&mut self) {
        let now = self.tick();
        let fx = self.machine.cancel(now).expect("cancel");
        self.apply(fx);
    }

    fn resume(&mut self) {
        let now = self.tick();
        let fx = self.machine.resume(now).expect("resume");
        self.apply(fx);
    }

    /// Every job the runner was given for `node`, oldest first.
    fn jobs_of(&self, node: &str) -> Vec<Job> {
        self.fake.jobs_of(node)
    }

    fn finished_with(&self) -> Vec<RunState> {
        self.effects.iter().filter_map(|effect| if let Effect::Finished(state) = effect { Some(*state) } else { None }).collect()
    }

    /// Statuses of the latest instances, as `node#variant:Status`, for whole-run assertions.
    fn statuses(&self) -> Vec<String> {
        let run = self.machine.run();
        let mut seen: Vec<(NodeId, u32)> = Vec::new();
        for instance in &run.nodes {
            let id = (instance.node.clone(), instance.variant);
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        seen.iter()
            .filter_map(|(node, variant)| run.instance(node, *variant))
            .map(|instance| format!("{}#{}:{:?}", instance.node, instance.variant, instance.status))
            .collect()
    }
}

/// Brings the fix graph with three scripted variants to its open review.
fn at_review(variants: u32) -> Bench {
    let fake = Arc::new(FakeRunner::new());
    fake.on("triage", Script::port("bug").summary("It is a bug."));
    let tags: &[&str] = match variants {
        1 => &["a"],
        2 => &["a", "b"],
        _ => &["a", "b", "c"],
    };
    script_variants(&fake, 1, tags);
    let mut bench = Bench::start_with(fix_graph(variants), fake);
    bench.drain();
    assert_eq!(bench.state(), RunState::WaitingReview, "{:?}", bench.statuses());
    bench
}
