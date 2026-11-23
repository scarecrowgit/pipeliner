//! Everything a frame needs from the pipeline service, read once per render: the pipeline
//! being edited, the list of pipelines (for the switcher), the latest runs of this
//! pipeline (light copies without the log), what each step is doing right now, and the
//! problems worth showing. Plus small formatting helpers.

use std::path::PathBuf;

use gpui::Hsla;
use pipeline::{CoderTask, PipelineDef, Pipelines, Run, RunStatus, Stage, StageStatus};
use ui::Status;
use ui::theme::*;

/// One entry of the pipeline switcher.
#[derive(Clone, Debug)]
pub struct PipeItem {
    pub id: String,
    pub name: String,
    pub steps: usize,
    /// Runs of it with a live worker.
    pub active: usize,
}

/// What the editor needs from one run (copied without the log).
#[derive(Clone, Debug)]
pub struct RunView {
    pub key: String,
    pub title: String,
    pub created: u64,
    pub status: RunStatus,
    /// A worker is running for it.
    pub active: bool,
    pub stages: Vec<Stage>,
    pub tasks: Vec<CoderTask>,
    pub pr_url: Option<String>,
    pub error: Option<String>,
    pub tokens: u64,
    pub cost_usd: f64,
}

impl RunView {
    pub fn new(run: &Run, title: &str, active: bool) -> Self {
        Self {
            key: run.issue.clone(),
            title: title.to_string(),
            created: run.created,
            status: run.status,
            active,
            stages: run.stages.clone(),
            tasks: run.tasks.clone(),
            pr_url: run.pr_url.clone(),
            error: run.error.clone(),
            tokens: run.tokens,
            cost_usd: run.cost_usd,
        }
    }

    /// The stage a worker is in (only while one is active).
    pub fn running_stage(&self) -> Option<&Stage> {
        if !self.active {
            return None;
        }
        self.stages.iter().find(|s| s.status == StageStatus::Running)
    }

    /// Where a failed or cancelled run stopped.
    pub fn stopped_at(&self) -> Option<&Stage> {
        if !matches!(self.status, RunStatus::Failed | RunStatus::Cancelled) {
            return None;
        }
        self.stages
            .iter()
            .find(|s| s.status == StageStatus::Failed)
            .or_else(|| {
                self.stages
                    .iter()
                    .find(|s| matches!(s.status, StageStatus::Pending | StageStatus::Running))
            })
    }

    pub fn waiting_stage(&self) -> Option<&Stage> {
        if self.status != RunStatus::WaitingApproval {
            return None;
        }
        self.stages.iter().find(|s| s.status == StageStatus::Waiting)
    }

    /// Short status for the run list and its color.
    pub fn status_label(&self) -> (String, Hsla) {
        match self.status {
            RunStatus::Running => match self.running_stage() {
                Some(s) => (format!("{} · running", s.name), INFO),
                None if self.active => ("Starting".into(), INFO),
                None => ("Stopped".into(), TEXT_MUTED),
            },
            RunStatus::WaitingApproval => match self.waiting_stage() {
                Some(s) => (format!("{} · waiting", s.name), WARNING),
                None => ("Waiting for you".into(), WARNING),
            },
            RunStatus::Done if self.pr_url.is_some() => ("PR opened".into(), SUCCESS),
            RunStatus::Done => ("Done".into(), SUCCESS),
            RunStatus::Failed => (
                match self.stopped_at() {
                    Some(s) => format!("Failed · {}", s.name),
                    None => "Failed".into(),
                },
                DANGER,
            ),
            RunStatus::Cancelled => ("Cancelled".into(), TEXT_MUTED),
        }
    }
}

pub struct Snap {
    /// The pipeline being edited (`None` when there are none).
    pub def: Option<PipelineDef>,
    pub pipes: Vec<PipeItem>,
    /// Pipeline files that could not be read or written.
    pub errors: Vec<String>,
    /// Latest run of every issue on this pipeline, newest issue first.
    pub runs: Vec<RunView>,
}

impl Snap {
    pub fn read(p: &Pipelines, pid: Option<&str>) -> Self {
        let def = pid
            .and_then(|id| p.pipeline(id))
            .or(p.default_pipeline())
            .cloned();
        let id = def.as_ref().map(|d| d.id.clone()).unwrap_or_default();
        let runs = p
            .issues
            .iter()
            .filter(|_| def.is_some())
            .filter_map(|i| {
                let active = p.is_active(&i.key);
                // Runs of older versions name no pipeline: they belong to the issue's.
                let fallback = p.pipeline_of(i).map(|d| d.id.clone()).unwrap_or_default();
                p.with_run(&i.key, |r| {
                    let of = if r.pipeline.is_empty() { fallback.as_str() } else { r.pipeline.as_str() };
                    (of == id).then(|| RunView::new(r, &i.title, active))
                })
                .flatten()
            })
            .collect();
        let pipes = p
            .pipelines
            .iter()
            .map(|d| PipeItem {
                id: d.id.clone(),
                name: d.name.clone(),
                steps: d.steps.len(),
                active: p.active_in(&d.id),
            })
            .collect();
        Self { def, pipes, errors: p.pipeline_errors.clone(), runs }
    }

    pub fn active(&self) -> usize {
        self.runs.iter().filter(|r| r.active).count()
    }
    pub fn waiting(&self) -> usize {
        self.runs.iter().filter(|r| r.status == RunStatus::WaitingApproval).count()
    }
    pub fn failed(&self) -> usize {
        self.runs
            .iter()
            .filter(|r| matches!(r.status, RunStatus::Failed | RunStatus::Cancelled))
            .count()
    }
    pub fn prs(&self) -> usize {
        self.runs.iter().filter(|r| r.pr_url.is_some()).count()
    }
    pub fn cost(&self) -> f64 {
        self.runs.iter().map(|r| r.cost_usd).sum()
    }
    pub fn tokens(&self) -> u64 {
        self.runs.iter().map(|r| r.tokens).sum()
    }

    /// Runs per step right now (by step id); `None` = the End node.
    pub fn counts(&self, step: Option<&str>) -> Counts {
        let mut c = Counts::default();
        for r in &self.runs {
            match step {
                None => {
                    if r.status == RunStatus::Done {
                        c.done += 1;
                    }
                }
                Some(id) => {
                    let Some(s) = r.stages.iter().find(|s| s.id == id) else { continue };
                    match s.status {
                        StageStatus::Running if r.active => c.running += 1,
                        StageStatus::Waiting => c.waiting += 1,
                        StageStatus::Done => c.done += 1,
                        _ => {}
                    }
                    if r.stopped_at().is_some_and(|x| x.id == id) {
                        c.failed += 1;
                    }
                }
            }
        }
        c
    }

    /// Sub-agent tasks of a delegating step: running now / finished.
    pub fn task_counts(&self, step: &str) -> Counts {
        let mut c = Counts::default();
        for r in &self.runs {
            for t in r.tasks.iter().filter(|t| t.step == step || t.step.is_empty()) {
                match t.status {
                    StageStatus::Running if r.active => c.running += 1,
                    StageStatus::Done => c.done += 1,
                    StageStatus::Failed => c.failed += 1,
                    _ => {}
                }
            }
        }
        c
    }

    /// Problems worth showing: the definition's, unreadable files, failed runs.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for e in &self.errors {
            out.push(Problem {
                title: "A pipeline file could not be read".into(),
                detail: e.clone(),
                step: None,
                workdir: None,
                issue: None,
                severe: true,
            });
        }
        if let Some(def) = &self.def {
            for p in def.problems() {
                // Point at the step a problem names (`Step `Build` has no workdir.`).
                let step = def
                    .steps
                    .iter()
                    .find(|s| p.starts_with(&format!("Step `{}`", s.name)))
                    .map(|s| s.id.clone());
                let workdir = def
                    .workdirs
                    .iter()
                    .find(|w| p.starts_with(&format!("Workdir `{}`", w.name)))
                    .map(|w| w.id.clone());
                out.push(Problem { title: p, detail: String::new(), step, workdir, issue: None, severe: false });
            }
        }
        for r in self.runs.iter().filter(|r| matches!(r.status, RunStatus::Failed | RunStatus::Cancelled)) {
            let at = r.stopped_at();
            out.push(Problem {
                title: format!(
                    "{} {} at {}",
                    r.key,
                    if r.status == RunStatus::Cancelled { "was cancelled" } else { "failed" },
                    at.map(|s| s.name.as_str()).unwrap_or("an unknown step")
                ),
                detail: r.error.clone().unwrap_or_else(|| r.title.clone()),
                step: at.map(|s| s.id.clone()),
                workdir: None,
                issue: Some(r.key.clone()),
                severe: true,
            });
        }
        out
    }
}

pub struct Problem {
    pub title: String,
    pub detail: String,
    /// Step to show.
    pub step: Option<String>,
    /// Workdir to show.
    pub workdir: Option<String>,
    pub issue: Option<String>,
    /// Failed runs and unreadable files (red) vs definition problems (amber).
    pub severe: bool,
}

/// Runs at a node right now, and how many went through it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub running: usize,
    pub waiting: usize,
    pub failed: usize,
    pub done: usize,
}

impl Counts {
    pub fn dot(&self) -> Status {
        if self.running > 0 {
            Status::Running
        } else if self.waiting > 0 {
            Status::Waiting
        } else if self.failed > 0 {
            Status::Error
        } else if self.done > 0 {
            Status::Live
        } else {
            Status::Draft
        }
    }

    /// `1 running`, `2 waiting`, `1 failed`, `3 done`.
    pub fn word(&self, done: &str) -> Option<(String, Hsla)> {
        if self.running > 0 {
            Some((format!("{} running", self.running), INFO))
        } else if self.waiting > 0 {
            Some((format!("{} waiting", self.waiting), WARNING))
        } else if self.failed > 0 {
            Some((format!("{} failed", self.failed), DANGER))
        } else if self.done > 0 {
            Some((format!("{} {done}", self.done), SUCCESS))
        } else {
            None
        }
    }
}

// ── Timeline ────────────────────────────────────────────────────────────

/// One stage on a run's timeline, as fractions of the run's duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub status: StageStatus,
    pub start: f32,
    pub end: f32,
}

/// Stage segments of `r` (pending and skipped stages left out) and the run's duration in
/// seconds. A stage without a start time (an approval wait) starts where the previous one
/// ended; running and waiting stages last until `now`.
pub fn segments(r: &RunView, now: u64) -> (Vec<Segment>, u64) {
    let t0 = r.created;
    let mut prev = t0;
    let mut spans = Vec::new();
    for s in &r.stages {
        if matches!(s.status, StageStatus::Pending | StageStatus::Skipped) {
            continue;
        }
        let start = if s.started > 0 { s.started } else { prev }.max(t0);
        let end = if s.finished > 0 {
            s.finished
        } else if matches!(s.status, StageStatus::Running | StageStatus::Waiting) {
            now
        } else {
            start
        }
        .max(start);
        spans.push((s.status, start, end));
        prev = prev.max(end);
    }
    let live = r.active || r.status == RunStatus::WaitingApproval;
    let t_end = if live { now.max(prev) } else { prev };
    let total = t_end.saturating_sub(t0);
    let span = total.max(1) as f32;
    let segs = spans
        .into_iter()
        .map(|(status, a, b)| Segment {
            status,
            start: (a - t0) as f32 / span,
            end: (b - t0) as f32 / span,
        })
        .collect();
    (segs, total)
}

pub fn segment_color(s: StageStatus) -> Hsla {
    match s {
        StageStatus::Done => SUCCESS,
        StageStatus::Running => INFO,
        StageStatus::Waiting => WARNING,
        StageStatus::Failed => DANGER,
        StageStatus::Pending | StageStatus::Skipped => LINE_STRONG,
    }
}

// ── Formatting ──────────────────────────────────────────────────────────

/// `42s`, `3m 05s`, `1h 12m`, `2d 4h`.
pub fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        3600..86_400 => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d {}h", secs / 86_400, secs % 86_400 / 3600),
    }
}

/// `950`, `12.4k`, `397k`, `1.2M`.
pub fn tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..10_000 => format!("{:.1}k", n as f64 / 1000.),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1000.).round()),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// `$0.42`; `—` when the price is unknown (0).
pub fn cost(usd: f64) -> String {
    if usd > 0. { format!("${usd:.2}") } else { "—".into() }
}

/// `nr/cc/claude-haiku-5-5` → `claude-haiku-5-5`.
pub fn model_short(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// First line of a (possibly multi-line) text.
pub fn first_line(s: &str) -> &str {
    s.trim().lines().next().unwrap_or("")
}

/// Skills installed on this machine, from the folders the pipeline loads them from
/// (`~/.claude/skills`, `~/.claude/skills/synced/*`, `~/.config/harness/skills`). Sorted.
pub fn installed_skills() -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut roots = vec![home.join(".claude/skills")];
    if let Ok(entries) = std::fs::read_dir(home.join(".claude/skills/synced")) {
        roots.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    roots.push(pipeline::def::config_dir().join("skills"));
    let mut out: Vec<String> = roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flat_map(|it| it.flatten())
        .filter(|e| e.path().join("SKILL.md").is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(id: &str, status: StageStatus, started: u64, finished: u64) -> Stage {
        Stage {
            id: id.into(),
            name: id.into(),
            kind: "agent".into(),
            status,
            started,
            finished,
            summary: String::new(),
        }
    }

    fn run(status: RunStatus, active: bool, stages: Vec<Stage>) -> RunView {
        RunView {
            key: "HAR-1".into(),
            title: "t".into(),
            created: 100,
            status,
            active,
            stages,
            tasks: Vec::new(),
            pr_url: None,
            error: None,
            tokens: 0,
            cost_usd: 0.,
        }
    }

    fn snap(runs: Vec<RunView>) -> Snap {
        Snap { def: None, pipes: Vec::new(), errors: Vec::new(), runs }
    }

    #[test]
    fn approval_wait_starts_after_the_previous_step() {
        let r = run(
            RunStatus::WaitingApproval,
            false,
            vec![
                stage("a", StageStatus::Done, 100, 110),
                stage("b", StageStatus::Done, 110, 150),
                stage("ok", StageStatus::Waiting, 0, 0),
                stage("c", StageStatus::Pending, 0, 0),
            ],
        );
        let (segs, total) = segments(&r, 200);
        assert_eq!(total, 100);
        assert_eq!(segs.len(), 3);
        assert_eq!((segs[2].start, segs[2].end), (0.5, 1.0));
        let s = snap(vec![r]);
        assert_eq!(s.counts(Some("ok")).waiting, 1);
        assert_eq!(s.counts(Some("b")).done, 1);
    }

    #[test]
    fn running_needs_an_active_worker_and_failures_count_where_they_stopped() {
        let stages = vec![stage("a", StageStatus::Done, 100, 105), stage("b", StageStatus::Running, 105, 0)];
        let mut r = run(RunStatus::Running, true, stages);
        assert_eq!(snap(vec![r.clone()]).counts(Some("b")).running, 1);
        r.active = false;
        assert_eq!(snap(vec![r.clone()]).counts(Some("b")).running, 0);
        r.status = RunStatus::Failed;
        r.stages[1].status = StageStatus::Failed;
        let s = snap(vec![r]);
        assert_eq!(s.counts(Some("b")).failed, 1);
        assert_eq!(s.counts(None).done, 0);
    }

    #[test]
    fn formats() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(185), "3m 05s");
        assert_eq!(duration(4320), "1h 12m");
        assert_eq!(tokens(396_701), "397k");
        assert_eq!(tokens(1_250_000), "1.2M");
        assert_eq!(cost(0.), "—");
        assert_eq!(model_short("nr/cc/claude-haiku-5-5"), "claude-haiku-5-5");
        assert_eq!(first_line("  cargo test\ncargo clippy"), "cargo test");
    }
}
