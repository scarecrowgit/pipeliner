//! Random storms of inputs, valid and stale, in any order. Whatever happens, the machine keeps
//! its invariants, and once people and jobs cooperate again every run reaches its end.

use std::collections::BTreeSet;

use model::{ReviewState, StartFrom};

use super::edges::exclusive_scope;
use super::restore::self_edit;
use super::rounds::{planned, side_branches};
use super::*;
use crate::exec::{Hold, Snapshot};

/// xorshift64: the same storm for the same seed, on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// The snapshot without the numbers json may round.
fn strip_costs(snapshot: &Snapshot) -> Snapshot {
    let mut bare = snapshot.clone();
    bare.run.cost_usd = 0.0;
    bare.budget_usd = None;
    for instance in &mut bare.run.nodes {
        instance.cost_usd = 0.0;
    }
    for meta in &mut bare.meta {
        meta.job_cost = 0.0;
        meta.node_budget = None;
        if let Some(output) = &mut meta.output {
            output.cost_usd = 0.0;
        }
    }
    for round in &mut bare.reviews {
        for variant in &mut round.review.variants {
            variant.cost_usd = 0.0;
        }
    }
    bare
}

fn graph_for(seed: u64) -> GraphDef {
    let mut def = match seed % 9 {
        0 => fix_graph(3),
        1 => fix_graph(1),
        2 => planned("planner"),
        3 => planned("polish"),
        4 => side_branches(),
        5 => self_edit(),
        7 => exclusive_scope(review("review", Some("team"))),
        8 => exclusive_scope(command("join", false)),
        _ => fix_graph(2),
    };
    def.limits.max_parallel = 1 + (seed / 9 % 3) as u32;
    def.limits.max_loops = 2;
    def.limits.budget_usd = seed.is_multiple_of(3).then_some(2.0);
    def
}

struct Storm {
    bench: Bench,
    graph: GraphDef,
    rng: Rng,
    seed: u64,
}

impl Storm {
    fn new(seed: u64) -> Storm {
        let graph = graph_for(seed);
        let bench = Bench::start_with(graph.clone(), Arc::new(FakeRunner::new()));
        let storm = Storm { bench, graph, rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1), seed };
        storm.check("start");
        storm
    }

    /// Takes the machine's effects like a driver would, checking what a driver relies on.
    fn apply(&mut self, fx: Vec<Effect>) {
        for effect in &fx {
            match effect {
                Effect::Start(job) => {
                    assert!(!self.bench.queue.iter().any(|queued| queued.key == job.key), "seed {}: a second job for {:?}", self.seed, job.key);
                }
                Effect::CancelJob(key) => {
                    assert!(self.bench.queue.iter().any(|queued| &queued.key == key), "seed {}: cancel for a job that is not there: {key:?}", self.seed);
                }
                _ => {}
            }
        }
        let runs = fx.iter().filter(|effect| matches!(effect, Effect::Run(_))).count();
        assert!(runs <= 1, "seed {}: the run is reported once per step", self.seed);
        self.bench.apply(fx);
    }

    fn port_for(&mut self, job: &Job, any: bool) -> String {
        match &job.node.kind {
            NodeKind::Router(router) => {
                let pick = if any { self.rng.below(router.rules.len() + 1) } else { 0 };
                router.rules.get(pick).map_or_else(|| PORT_ELSE.to_string(), |rule| rule.id.clone())
            }
            _ => PORT_OUT.to_string(),
        }
    }

    fn output(&mut self, job: &Job) -> NodeOutput {
        let tag = format!("{}-{}-{}", job.key.node, job.key.variant, job.key.attempt);
        let mut output = if self.rng.chance(70) { work(&tag, "Worked.") } else { summary("Looked.") };
        output.cost_usd = self.rng.below(4) as f64 * 0.25;
        output
    }

    /// Ends a random job in flight in a random way.
    fn end_job(&mut self) {
        if self.bench.queue.is_empty() {
            return;
        }
        let pos = self.rng.below(self.bench.queue.len());
        let job = self.bench.queue.remove(pos).expect("a job");
        let told = self.bench.cancelled.iter().position(|key| key == &job.key).map(|at| self.bench.cancelled.remove(at)).is_some();
        let output = self.output(&job);
        let roll = self.rng.below(100);
        let result = if told && roll < 80 {
            JobResult::Cancelled { output }
        } else {
            match roll % 20 {
                0 => JobResult::Ask { question: "Which way?".into(), output },
                1 => JobResult::Failed { error: "It broke.".into(), output },
                2 => JobResult::OverBudget { output },
                3 => JobResult::Cancelled { output },
                _ => JobResult::Done { port: self.port_for(&job, true), output },
            }
        };
        let now = self.bench.tick();
        let fx = self.bench.machine.job_finished(&job.key, result, now).unwrap_or_else(|error| panic!("seed {}: {error}", self.seed));
        self.apply(fx);
    }

    fn decide(&mut self) {
        let Some(review) = self.bench.machine.open_reviews().first().map(|review| (*review).clone()) else { return };
        let index = self.rng.below(4) as u32;
        let start = match self.rng.below(3) {
            0 => StartFrom::All,
            1 => StartFrom::Over,
            _ => StartFrom::Variant { index },
        };
        let decision = match self.rng.below(5) {
            0 => ReviewDecision::Skip,
            1 | 2 => ReviewDecision::Ship { variant: index },
            _ => ReviewDecision::SendBack { note: "Again.".into(), start, variants: self.rng.below(4) as u32 },
        };
        let before = self.bench.machine.snapshot().clone();
        let now = self.bench.tick();
        match self.bench.machine.decide(&review.id, decision, now) {
            Ok(fx) => self.apply(fx),
            Err(_) => assert_eq!(self.bench.machine.snapshot(), &before, "seed {}: a refused decision left a trace", self.seed),
        }
    }

    /// Inputs that cannot be right: they must be refused and leave nothing behind.
    fn stale(&mut self) {
        let before = self.bench.machine.snapshot().clone();
        let nodes = &self.bench.machine.run().nodes;
        let key = if nodes.is_empty() {
            InstanceKey { node: "ghost".into(), variant: 0, attempt: 1 }
        } else {
            let instance = &nodes[self.rng.below(nodes.len())];
            InstanceKey { node: instance.node.clone(), variant: instance.variant, attempt: instance.attempt }
        };
        let in_flight = self.bench.queue.iter().any(|job| job.key == key);
        let waiting = self.bench.machine.holds().iter().any(|(held, hold)| held == &key && matches!(hold, Hold::Question { .. }));
        let closed: Vec<_> = self.bench.machine.snapshot().reviews.iter().map(|round| round.review.id.clone()).collect();
        let open: BTreeSet<_> = self.bench.machine.open_reviews().iter().map(|review| review.id.clone()).collect();
        if !in_flight {
            let done = JobResult::Done { port: PORT_OUT.into(), output: NodeOutput::default() };
            assert!(self.bench.machine.job_finished(&key, done, 1).is_err(), "seed {}", self.seed);
            assert!(self.bench.machine.note(&key, JobNote::Spent { cost_usd: 9.0, usage: Default::default() }, 1).is_err());
        }
        if !waiting {
            assert!(self.bench.machine.answer(&key, "x".into(), 1).is_err(), "seed {}", self.seed);
        }
        for id in closed.iter().filter(|id| !open.contains(*id)) {
            assert!(self.bench.machine.decide(id, ReviewDecision::Skip, 1).is_err(), "seed {}", self.seed);
        }
        assert!(self.bench.machine.raise_budget(f64::NAN, 1).is_err());
        assert_eq!(self.bench.machine.snapshot(), &before, "seed {}: a refused input left a trace", self.seed);
    }

    /// The process dies and comes back from what was stored.
    fn restart(&mut self) {
        let text = serde_json::to_string(self.bench.machine.snapshot()).expect("json");
        let stored: Snapshot = serde_json::from_str(&text).expect("snapshot");
        // serde_json is built without `float_roundtrip`, so a cost may come back one ulp off;
        // everything else must be the same.
        let again: Snapshot = serde_json::from_str(&serde_json::to_string(&stored).expect("json")).expect("snapshot");
        assert_eq!(again.run.nodes.len(), self.bench.machine.run().nodes.len());
        assert_eq!(strip_costs(&stored), strip_costs(self.bench.machine.snapshot()), "seed {}: json changed the snapshot", self.seed);
        self.bench.machine = Machine::restore(self.graph.clone(), stored);
        self.bench.queue.clear();
        self.bench.cancelled.clear();
    }

    fn act(&mut self) {
        let now = self.bench.tick();
        match self.rng.below(100) {
            0..=44 => self.end_job(),
            45..=54 => {
                if !self.bench.queue.is_empty() {
                    let key = self.bench.queue[self.rng.below(self.bench.queue.len())].key.clone();
                    let note = JobNote::Spent { cost_usd: 0.3, usage: Default::default() };
                    let fx = self.bench.machine.note(&key, note, now).unwrap_or_else(|error| panic!("seed {}: {error}", self.seed));
                    self.apply(fx);
                }
            }
            55..=66 => self.decide(),
            67..=72 => {
                for (key, hold) in self.bench.machine.holds() {
                    let outcome = match hold {
                        Hold::Question { .. } => self.bench.machine.answer(&key, "That way.".into(), now),
                        Hold::Budget => self.bench.machine.raise_budget(1.0, now),
                    };
                    if let Ok(fx) = outcome {
                        self.apply(fx);
                    }
                }
            }
            73..=77 => {
                if let Ok(fx) = self.bench.machine.cancel(now) {
                    self.apply(fx);
                }
            }
            78..=85 => {
                if let Ok(fx) = self.bench.machine.resume(now) {
                    self.apply(fx);
                }
            }
            86..=88 => {
                let fx = self.bench.machine.interrupt(now);
                self.apply(fx);
            }
            89..=94 => self.stale(),
            _ => self.restart(),
        }
    }

    /// What must hold after every input.
    fn check(&self, when: &str) {
        let seed = self.seed;
        let snapshot = self.bench.machine.snapshot();
        let run = &snapshot.run;
        assert_eq!(run.nodes.len(), snapshot.meta.len(), "seed {seed} {when}: instances and their meta are paired");
        let mut keys = BTreeSet::new();
        for (instance, meta) in run.nodes.iter().zip(&snapshot.meta) {
            if meta.spawn.is_none() {
                assert!(keys.insert((instance.node.clone(), instance.variant, instance.attempt)), "seed {seed} {when}: {} twice", instance.node);
            }
            if meta.in_flight {
                let key = InstanceKey { node: instance.node.clone(), variant: instance.variant, attempt: instance.attempt };
                assert!(self.bench.queue.iter().any(|job| job.key == key), "seed {seed} {when}: in flight without a job: {key:?}");
            }
            if instance.status == NodeStatus::Running {
                assert!(meta.in_flight, "seed {seed} {when}: {} runs without a job", instance.node);
            }
        }
        assert_eq!(self.bench.machine.jobs_in_flight(), self.bench.queue.len(), "seed {seed} {when}: every job in flight is known");
        let agents = self.bench.queue.iter().filter(|job| matches!(job.node.kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_))).count();
        assert!(agents <= self.graph.limits.max_parallel as usize, "seed {seed} {when}: {agents} agents at once");
        for key in &self.bench.cancelled {
            assert!(self.bench.queue.iter().any(|job| &job.key == key), "seed {seed} {when}: cancel for a job that is not there");
        }
        let closed = matches!(run.state, RunState::Done | RunState::Failed | RunState::Cancelled);
        assert_eq!(run.finished.is_some(), closed, "seed {seed} {when}: finished is set exactly on closed runs ({:?})", run.state);
        assert_eq!(run.error.is_some(), run.state == RunState::Failed, "seed {seed} {when}: only a failed run has an error");
        if run.state == RunState::WaitingReview {
            assert!(!self.bench.machine.open_reviews().is_empty(), "seed {seed} {when}");
        }
        if matches!(run.state, RunState::Done | RunState::Cancelled | RunState::Failed) {
            assert!(self.bench.machine.open_reviews().is_empty(), "seed {seed} {when}: a closed run has no open review");
        }
        if run.state == RunState::Done {
            let live: Vec<_> = self.bench.queue.iter().filter(|job| !self.bench.cancelled.contains(&job.key)).collect();
            assert!(live.is_empty(), "seed {seed} {when}: done with live jobs {live:?}");
        }
        let total: f64 = run.nodes.iter().map(|instance| instance.cost_usd).sum();
        assert!((total - run.cost_usd).abs() < 1e-6, "seed {seed} {when}: the run's cost is the sum of its instances");
    }

    /// Everyone cooperates from here: jobs succeed, people answer, raise and ship.
    fn calm(&mut self) {
        for _ in 0..2000 {
            let now = self.bench.tick();
            let state = self.bench.state();
            let fx = if !self.bench.queue.is_empty() {
                let job = self.bench.queue.pop_front().expect("a job");
                let told = self.bench.cancelled.iter().position(|key| key == &job.key).map(|at| self.bench.cancelled.remove(at)).is_some();
                let output = work(&format!("{}-{}", job.key.node, job.key.variant), "Worked.");
                let result = if told { JobResult::Cancelled { output } } else { JobResult::Done { port: self.port_for(&job, false), output } };
                self.bench.machine.job_finished(&job.key, result, now).expect("in flight")
            } else if matches!(state, RunState::Cancelled | RunState::Failed | RunState::Interrupted) {
                self.bench.machine.resume(now).expect("resume")
            } else if state == RunState::Done {
                self.check("the end");
                return;
            } else if let Some(review) = self.bench.machine.open_reviews().first().map(|review| (*review).clone()) {
                let variant = review.variants[0].index;
                self.bench.machine.decide(&review.id, ReviewDecision::Ship { variant }, now).expect("ship")
            } else if let Some((key, Hold::Question { .. })) = self.bench.machine.holds().into_iter().next() {
                self.bench.machine.answer(&key, "That way.".into(), now).expect("answer")
            } else if state == RunState::WaitingAnswer {
                self.bench.machine.raise_budget(5.0, now).expect("raise")
            } else {
                panic!("seed {}: stuck in {state:?}: {:?}", self.seed, self.bench.statuses());
            };
            self.apply(fx);
            self.check("calm");
        }
        panic!("seed {}: no end in sight: {:?} {:?}", self.seed, self.bench.state(), self.bench.statuses());
    }
}

#[test]
fn no_storm_of_inputs_corrupts_a_run_and_every_run_can_still_finish() {
    let seeds: u64 = std::env::var("STORM_SEEDS").ok().and_then(|text| text.parse().ok()).unwrap_or(600);
    let (mut reviews, mut sent_back, mut shipped, mut discards, mut failed_variants) = (0, 0, 0, 0, 0);
    for seed in 1..=seeds {
        let mut storm = Storm::new(seed);
        for step in 0..(20 + seed % 90) {
            storm.act();
            storm.check(&format!("step {step}"));
        }
        storm.calm();
        let snapshot = storm.bench.machine.snapshot();
        assert_eq!(snapshot.run.state, RunState::Done, "seed {seed}");
        assert!(snapshot.run.nodes.iter().all(|instance| instance.status != NodeStatus::Running), "seed {seed}");
        // What a shipped review let through was never thrown away.
        let gone: Vec<_> = storm.bench.fake.discards().into_iter().filter_map(|out| out.worktree).collect();
        for round in snapshot.reviews.iter().filter(|round| round.review.state == ReviewState::Shipped) {
            let current = snapshot.run.nodes.iter().zip(&snapshot.meta).any(|(instance, meta)| {
                !meta.superseded && instance.node == round.review.node && instance.attempt == round.attempt
            });
            let picked = round.review.variants.iter().find(|variant| Some(variant.index) == round.review.picked);
            if let (true, Some(worktree)) = (current, picked.and_then(|variant| variant.worktree.clone())) {
                assert!(!gone.contains(&worktree), "seed {seed}: the shipped worktree {worktree:?} was discarded");
            }
        }
        reviews += snapshot.reviews.len();
        sent_back += snapshot.reviews.iter().filter(|round| round.review.state == ReviewState::SentBack).count();
        shipped += snapshot.reviews.iter().filter(|round| round.review.state == ReviewState::Shipped).count();
        discards += storm.bench.fake.discards().len();
        failed_variants += snapshot.run.nodes.iter().filter(|instance| instance.status == NodeStatus::Failed).count();
    }
    // The storms must actually reach the interesting places.
    assert!(reviews > 200 && sent_back > 50 && shipped > 100 && discards > 100 && failed_variants > 10,
        "reviews {reviews}, sent back {sent_back}, shipped {shipped}, discards {discards}, failed variants {failed_variants}");
    eprintln!("storm: reviews {reviews}, sent back {sent_back}, shipped {shipped}, discards {discards}, failed variants {failed_variants}");
}
