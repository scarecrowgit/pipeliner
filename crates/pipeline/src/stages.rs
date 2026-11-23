//! The step engine. A run walks its pipeline's steps in order (decisions may jump, approvals
//! may send it back); each step kind has its handler below. It blocks (agents, git, builds)
//! and runs on a worker thread; every change is saved and announced via `notify`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_tools::Workspace;
use agent_tools::inbox::{self, InboxItem, Link, Outcome as InboxOutcome};
use anyhow::{Context as _, anyhow, bail};
use llm::{ToolSpec, Usage};
use serde_json::{Value, json};

use crate::agent::{self, AgentSpec, Reply, StageTool, clip};
use crate::def::{Delegate, PipelineDef, StepDef, StepKind, WorkdirDef};
use crate::exec;
use crate::model::{
    AgentUsage, CoderTask, Decision, Issue, IssueStatus, LogKind, LogLine, Render, Run,
    RunStatus, RunWorkdir, StageStatus, now, slug, update_issue,
};

/// What a worker thread should do.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// From the first step.
    Start,
    /// The user approved the waiting step.
    Approved,
    /// The user asked for changes at the waiting step.
    Redesign(String),
    /// Continue from the step that failed.
    Retry,
}

pub struct Ctx {
    pub issue: Issue,
    pub run: Arc<Mutex<Run>>,
    pub cancel: Arc<AtomicBool>,
    pub notify: Box<dyn Fn() + Send + Sync>,
}

const LOG_LIMIT: usize = 3000;

impl Ctx {
    pub fn update(&self, f: impl FnOnce(&mut Run)) {
        {
            let mut r = self.run.lock().expect("run lock");
            f(&mut r);
            let _ = r.save();
        }
        (self.notify)();
    }

    fn snapshot(&self) -> Run {
        self.run.lock().expect("run lock").clone()
    }

    fn def(&self) -> PipelineDef {
        self.run
            .lock()
            .expect("run lock")
            .def
            .clone()
            .unwrap_or_default()
    }

    pub fn log(&self, who: &str, kind: LogKind, text: impl Into<String>) {
        let line = LogLine { at: now(), who: who.to_string(), kind, text: text.into() };
        self.update(|r| push_log(r, line));
    }

    fn logger(&self) -> impl Fn(LogLine) + '_ {
        move |line| self.update(|r| push_log(r, line))
    }

    fn add_usage(&self, who: &str, model: &str, u: &Usage) {
        let n = u.input_tokens + u.output_tokens + u.cache_read_tokens + u.cache_write_tokens;
        let cost = model
            .split_once('/')
            .and_then(|(p, m)| llm::pricing::price(p, m))
            .map(|p| llm::pricing::cost(p, u))
            .unwrap_or(0.);
        self.update(|r| {
            r.tokens += n;
            r.cost_usd += cost;
            match r.usage.iter_mut().find(|x| x.who == who && x.model == model) {
                Some(x) => {
                    x.tokens += n;
                    x.cost_usd += cost;
                }
                None => r.usage.push(AgentUsage {
                    who: who.to_string(),
                    model: model.to_string(),
                    tokens: n,
                    cost_usd: cost,
                }),
            }
        });
    }

    fn issue_status(&self, status: IssueStatus) {
        let _ = update_issue(&self.issue.key, |i| i.status = status);
        (self.notify)();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn push_log(r: &mut Run, line: LogLine) {
    r.log.push(line);
    if r.log.len() > LOG_LIMIT {
        let drop = r.log.len() - LOG_LIMIT;
        r.log.drain(..drop);
    }
}

/// Board column while a step of `kind` runs.
fn status_for(kind: StepKind) -> IssueStatus {
    match kind {
        StepKind::Decision => IssueStatus::Triage,
        StepKind::Approval => IssueStatus::Approval,
        _ => IssueStatus::Build,
    }
}

/// Run `job` to its next stop (approval, done, failure, cancel).
pub fn dispatch(ctx: &Ctx, job: Job) {
    ctx.update(|r| {
        r.status = RunStatus::Running;
        r.error = None;
    });
    let res = match job {
        Job::Start => {
            ctx.update(|r| r.cursor = 0);
            walk(ctx)
        }
        Job::Approved => approved(ctx).and_then(|_| walk(ctx)),
        Job::Redesign(notes) => send_back(ctx, notes).and_then(|_| walk(ctx)),
        Job::Retry => walk(ctx),
    };
    if let Err(e) = res {
        let cancelled = ctx.cancelled();
        let msg = if cancelled { "Cancelled".to_string() } else { format!("{e:#}") };
        ctx.log("pipeline", LogKind::Error, msg.clone());
        ctx.update(|r| {
            for s in r.stages.iter_mut().filter(|s| s.status == StageStatus::Running) {
                s.status = StageStatus::Failed;
                s.finished = now();
                s.summary = clip(&msg, 200);
            }
            r.status = if cancelled { RunStatus::Cancelled } else { RunStatus::Failed };
            r.error = Some(msg);
        });
        ctx.issue_status(if cancelled { IssueStatus::Backlog } else { IssueStatus::Failed });
    }
}

/// The waiting approval at the cursor was approved: mark it done, move on.
fn approved(ctx: &Ctx) -> anyhow::Result<()> {
    ctx.update(|r| {
        let i = r.cursor;
        r.set_stage(i, StageStatus::Done, "approved");
        r.cursor = i + 1;
    });
    Ok(())
}

/// Changes requested at the approval at the cursor: go back to its `back_to` step.
fn send_back(ctx: &Ctx, notes: String) -> anyhow::Result<()> {
    let def = ctx.def();
    let r = ctx.snapshot();
    let step = def.steps.get(r.cursor).ok_or_else(|| anyhow!("no waiting step"))?;
    let back = def
        .step_index(&step.back_to)
        .ok_or_else(|| anyhow!("`{}` has no step to send back to", step.name))?;
    ctx.update(|r| {
        r.feedback.push(notes);
        let i = r.cursor;
        r.set_stage(i, StageStatus::Pending, "changes requested");
        for j in back..r.stages.len() {
            if j != i {
                r.stages[j].status = StageStatus::Pending;
            }
        }
        r.cursor = back;
    });
    Ok(())
}

enum Flow {
    Next,
    /// Nothing to do; mark skipped.
    Skip,
    /// Jump to step index (`None` = end); steps in between are skipped.
    Jump(Option<usize>),
    /// Stop and wait for the user.
    Wait,
}

/// Run steps from the cursor until the end, an approval stop or an error.
fn walk(ctx: &Ctx) -> anyhow::Result<()> {
    let def = ctx.def();
    loop {
        if ctx.cancelled() {
            bail!("cancelled");
        }
        let i = ctx.snapshot().cursor;
        let Some(step) = def.steps.get(i).cloned() else {
            return finish(ctx);
        };
        ctx.update(|r| r.set_stage(i, StageStatus::Running, ""));
        ctx.issue_status(status_for(step.kind));
        let (summary, flow) = match step.kind {
            StepKind::Agent => (agent_step(ctx, &def, &step)?, Flow::Next),
            StepKind::Decision => decision_step(ctx, &def, &step)?,
            StepKind::Render => (render_step(ctx, &def, &step)?, Flow::Next),
            StepKind::Approval => approval_step(ctx, &def, &step, i),
            StepKind::Command => (command_step(ctx, &def, &step)?, Flow::Next),
            StepKind::Merge => (merge_step(ctx, &def, &step)?, Flow::Next),
            StepKind::PullRequest => (pr_step(ctx, &def, &step)?, Flow::Next),
            StepKind::Report => (report_step(ctx, &def)?, Flow::Next),
        };
        match flow {
            Flow::Next => ctx.update(|r| {
                r.set_stage(i, StageStatus::Done, summary);
                r.cursor = i + 1;
            }),
            Flow::Skip => ctx.update(|r| {
                r.set_stage(i, StageStatus::Skipped, summary);
                r.cursor = i + 1;
            }),
            Flow::Jump(to) => ctx.update(|r| {
                r.set_stage(i, StageStatus::Done, summary);
                let end = to.unwrap_or(r.stages.len()).max(i + 1);
                for j in i + 1..end {
                    r.set_stage(j, StageStatus::Skipped, "skipped");
                }
                r.cursor = end;
            }),
            Flow::Wait => {
                ctx.update(|r| {
                    r.set_stage(i, StageStatus::Waiting, summary);
                    r.status = RunStatus::WaitingApproval;
                });
                ctx.issue_status(IssueStatus::Approval);
                ctx.log("pipeline", LogKind::Step, format!("{} — waiting for you", step.name));
                return Ok(());
            }
        }
    }
}

fn finish(ctx: &Ctx) -> anyhow::Result<()> {
    let has_pr = ctx.snapshot().pr_url.is_some();
    ctx.update(|r| r.status = RunStatus::Done);
    ctx.issue_status(if has_pr { IssueStatus::Review } else { IssueStatus::Done });
    ctx.log("pipeline", LogKind::Step, "Run finished");
    Ok(())
}

// ── Workdirs ────────────────────────────────────────────────────────────

/// `base`, or `base-2`, `base-3`… if the branch exists in `repo`.
fn unique_branch(repo: &Path, base: &str, cancel: &AtomicBool) -> String {
    let exists = |b: &str| {
        exec::git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")], cancel).is_ok()
    };
    if !exists(base) {
        return base.to_string();
    }
    (2..100)
        .map(|n| format!("{base}-{n}"))
        .find(|b| !exists(b))
        .unwrap_or_else(|| format!("{base}-{}", now()))
}

/// The run's copy of workdir `id`: its worktree (made on first use, setup run once) or the
/// folder itself.
fn workdir(ctx: &Ctx, def: &PipelineDef, id: &str) -> anyhow::Result<RunWorkdir> {
    if let Some(w) = ctx.snapshot().workdir(id).cloned().filter(|w| w.path.is_dir()) {
        return Ok(w);
    }
    let wd: WorkdirDef = def
        .workdir(id)
        .cloned()
        .ok_or_else(|| anyhow!("the pipeline has no workdir `{id}`"))?;
    if !wd.path.is_dir() {
        bail!("workdir `{}`: {} does not exist", wd.name, wd.path.display());
    }
    let rw = if wd.worktree {
        let base = format!("{}{}", wd.branch_prefix, slug(&ctx.issue.key, &ctx.issue.title));
        let branch = unique_branch(&wd.path, &base, &ctx.cancel);
        let path = agent_tools::workdir::create_worktree(&wd.path, &wd.base_branch, &branch)
            .with_context(|| format!("creating a worktree of {}", wd.path.display()))?;
        ctx.log(
            "pipeline",
            LogKind::Step,
            format!("{}: worktree {} on branch {branch}", wd.name, crate::def::tilde(&path)),
        );
        RunWorkdir {
            id: wd.id.clone(),
            name: wd.name.clone(),
            path,
            branch: Some(branch),
            repo: wd.path.clone(),
        }
    } else {
        RunWorkdir {
            id: wd.id.clone(),
            name: wd.name.clone(),
            path: wd.path.clone(),
            branch: None,
            repo: wd.path.clone(),
        }
    };
    ctx.update(|r| {
        r.workdirs.retain(|w| w.id != rw.id);
        r.workdirs.push(rw.clone());
    });
    if wd.worktree && !wd.setup.trim().is_empty() {
        ctx.log("pipeline", LogKind::Step, format!("{} setup: {}", wd.name, wd.setup));
        match exec::shell(&rw.path, &wd.setup, &env_of(&wd), Duration::from_secs(60 * 45), &ctx.cancel) {
            Ok(o) if o.ok => ctx.log("pipeline", LogKind::Step, format!("{} setup finished", wd.name)),
            Ok(o) => ctx.log("pipeline", LogKind::Error, format!("{} setup failed:\n{}", wd.name, o.tail())),
            Err(e) => {
                if ctx.cancelled() {
                    return Err(e);
                }
                ctx.log("pipeline", LogKind::Error, format!("{} setup: {e}", wd.name))
            }
        }
    }
    Ok(rw)
}

fn env_of(wd: &WorkdirDef) -> Vec<(String, String)> {
    wd.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn step_env(def: &PipelineDef, step: &StepDef) -> Vec<(String, String)> {
    def.workdir(&step.workdir).map(env_of).unwrap_or_default()
}

// ── Context for agents ──────────────────────────────────────────────────

fn issue_text(issue: &Issue) -> String {
    let body = if issue.body.trim().is_empty() { "(no description)" } else { issue.body.trim() };
    format!("Issue {} ({}): {}\n\n{}", issue.key, issue.kind, issue.title, body)
}

/// What earlier steps did, for the next agent.
fn history(run: &Run, def: &PipelineDef, upto: usize) -> String {
    let mut out = String::new();
    for (i, s) in run.stages.iter().enumerate().take(upto) {
        if matches!(s.status, StageStatus::Pending) {
            continue;
        }
        let kind = def.steps.get(i).map(|d| d.kind.label()).unwrap_or("");
        out.push_str(&format!("\n- {} ({kind}, {:?}): {}", s.name, s.status, s.summary));
        if let Some(n) = run.notes.get(&s.id).filter(|n| !n.trim().is_empty()) {
            out.push_str(&format!("\n  {}", clip(n, 3000).replace('\n', "\n  ")));
        }
    }
    if !run.renders.is_empty() {
        out.push_str(&format!(
            "\n\nRendered for review: {}",
            run.renders.iter().map(|r| r.screen.clone()).collect::<Vec<_>>().join(", ")
        ));
    }
    if out.is_empty() { String::new() } else { format!("\n\nEarlier steps of this run:{out}") }
}

fn feedback_text(run: &Run) -> String {
    if run.feedback.is_empty() {
        return String::new();
    }
    format!(
        "\n\nThe user reviewed this run and asked for changes (latest last):\n{}",
        run.feedback
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. {f}", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// `read_workdir`: read files from the run's workdirs (read-only).
fn read_workdir_tool<'a>(ctx: &'a Ctx, def: &'a PipelineDef) -> StageTool<'a> {
    let names = def
        .workdirs
        .iter()
        .map(|w| format!("`{}` ({})", w.id, w.name))
        .collect::<Vec<_>>()
        .join(", ");
    StageTool {
        spec: ToolSpec {
            name: "read_workdir".into(),
            description: format!(
                "Read a file or list a folder in a workdir of this run (read-only). Workdirs: \
                 {names}. HTML is returned without styles and SVG paths."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workdir": { "type": "string" },
                    "path": { "type": "string", "description": "Relative path; a folder lists it" }
                },
                "required": ["workdir"]
            }),
        },
        call: Box::new(move |input| {
            let id = str_of(input, "workdir");
            let rel = str_of(input, "path");
            if rel.contains("..") {
                return Reply::err("`..` is not allowed");
            }
            let root = match ctx.snapshot().workdir(&id).map(|w| w.path.clone()) {
                Some(p) => p,
                None => match def.workdir(&id) {
                    Some(w) => w.path.clone(),
                    None => return Reply::err(format!("No workdir `{id}`.")),
                },
            };
            let p = root.join(rel.trim_start_matches('/'));
            if p.is_dir() {
                let mut names: Vec<String> = std::fs::read_dir(&p)
                    .map(|e| {
                        e.flatten()
                            .map(|e| {
                                let n = e.file_name().to_string_lossy().into_owned();
                                if e.path().is_dir() { format!("{n}/") } else { n }
                            })
                            .filter(|n| !n.starts_with('.'))
                            .collect()
                    })
                    .unwrap_or_default();
                names.sort();
                return Reply::ok(names.join("\n"));
            }
            match std::fs::read_to_string(&p) {
                Ok(t) if p.extension().is_some_and(|x| x == "html") => {
                    Reply::ok(clip(&compact_html(&t), 60_000))
                }
                Ok(t) => Reply::ok(clip(&t, 60_000)),
                Err(e) => Reply::err(format!("{}: {e}", p.display())),
            }
        }),
    }
}

/// HTML without styles and SVG paths, whitespace collapsed (what a coder needs).
pub fn compact_html(html: &str) -> String {
    let mut s = html.to_string();
    for (open, close, keep) in [("<style", "</style>", ""), ("<svg", "</svg>", "<svg/>")] {
        while let Some(a) = s.find(open) {
            let Some(b) = s[a..].find(close) else { break };
            s.replace_range(a..a + b + close.len(), keep);
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out.replace("> <", "><")
}

fn system_prompt(def: &PipelineDef, step: &StepDef, root: &Path, run: &Run, extra: &str) -> String {
    let wd = def.workdir(&step.workdir).map(|w| w.name.clone()).unwrap_or_default();
    format!(
        "You are {name}, a step of the \"{pipeline}\" delivery pipeline. You work on one issue \
         in the workdir \"{wd}\" at {root}{branch}. Workdirs of the run can be read with \
         `read_workdir`.{extra}\n\n{instructions}{skills}{notes}",
        name = step.name,
        pipeline = def.name,
        root = root.display(),
        branch = run
            .workdir(&step.workdir)
            .and_then(|w| w.branch.clone())
            .map(|b| format!(" (branch {b})"))
            .unwrap_or_default(),
        instructions = step.instructions.trim(),
        skills = agent::forced_skills(&step.skills),
        notes = agent::project_notes(root),
    )
}

// ── Agent ───────────────────────────────────────────────────────────────

type Done = Option<(String, Option<bool>, String)>;

fn agent_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef) -> anyhow::Result<String> {
    let wd = workdir(ctx, def, &step.workdir)?;
    let run = ctx.snapshot();
    let env = step_env(def, step);
    let done: Mutex<Done> = Mutex::new(None);
    let counter = Mutex::new(run.tasks.len());
    let orchestrates = step.delegate.is_some();

    let finish = StageTool {
        spec: ToolSpec {
            name: "step_done".into(),
            description: if orchestrates {
                "Finish: what was implemented, whether you verified it (builds, tests and \
                 lints pass) and the checks you ran with their results."
            } else {
                "Finish your step: a short summary of what you did, for the next steps and the user."
            }
            .into(),
            input_schema: if orchestrates {
                json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string", "description": "Markdown" },
                        "verified": { "type": "boolean" },
                        "checks": { "type": "string", "description": "Commands run and their results" }
                    },
                    "required": ["summary", "verified", "checks"]
                })
            } else {
                json!({
                    "type": "object",
                    "properties": { "summary": { "type": "string" } },
                    "required": ["summary"]
                })
            },
        },
        call: Box::new(|input| {
            let verified = input.get("verified").and_then(Value::as_bool);
            *done.lock().expect("lock") =
                Some((str_of(input, "summary"), verified, str_of(input, "checks")));
            Reply::finish("Recorded.")
        }),
    };
    let mut tools = vec![
        read_workdir_tool(ctx, def),
        agent::load_skill_tool(step.skills_on_demand.clone()),
        finish,
    ];
    if let Some(dlg) = step.delegate.clone() {
        let wd_path = wd.path.clone();
        let env = env.clone();
        let step_id = step.id.clone();
        let step_name = step.name.clone();
        let counter = &counter;
        tools.push(StageTool {
            spec: ToolSpec {
                name: "delegate".into(),
                description: format!(
                    "Give one focused task to a {} ({}) working in the same workdir. It reports \
                     back; you get its summary and the git status / diff stat. Give exact files, \
                     the behaviour to build and how to check it. Tasks run one at a time.",
                    dlg.name, dlg.model
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Short task name" },
                        "instructions": { "type": "string" }
                    },
                    "required": ["title", "instructions"]
                }),
            },
            call: Box::new(move |input| {
                let title = str_of(input, "title");
                let instructions = str_of(input, "instructions");
                let n = {
                    let mut c = counter.lock().expect("lock");
                    *c += 1;
                    *c
                };
                let idx = ctx.snapshot().tasks.len();
                ctx.update(|r| {
                    r.tasks.push(CoderTask {
                        title: title.clone(),
                        status: StageStatus::Running,
                        summary: String::new(),
                        step: step_id.clone(),
                    })
                });
                let res = run_delegate(ctx, def, &dlg, n, &wd_path, &env, &step_name, &title, &instructions);
                let (status, summary) = match &res {
                    Ok(s) => (StageStatus::Done, s.clone()),
                    Err(e) => (StageStatus::Failed, format!("{e:#}")),
                };
                ctx.update(|r| {
                    if let Some(t) = r.tasks.get_mut(idx) {
                        t.status = status;
                        t.summary = clip(&summary, 2000);
                    }
                });
                let diff = diff_summary(&wd_path, &ctx.cancel);
                match res {
                    Ok(s) => Reply::ok(format!("{} {n} finished:\n{s}\n\n{diff}", dlg.name)),
                    Err(e) => Reply::err(format!("{} {n} failed: {e:#}\n\n{diff}", dlg.name)),
                }
            }),
        });
    }
    let extra = if orchestrates {
        " You lead sub-agents through `delegate`. Do not commit or push unless your instructions say so."
    } else {
        " Do not commit or push unless your instructions say so."
    };
    let mut spec = AgentSpec {
        name: step.name.clone(),
        model: step.model.clone(),
        system: system_prompt(def, step, &wd.path, &run, extra) + "\n\nFinish with `step_done`.",
        workspace: Some((Workspace::new(wd.path.clone()).with_env(env), step.tools.names())),
        tools,
        max_steps: step.max_steps.max(1),
        max_tokens: 32_000,
    };
    let task = format!(
        "{}{}{}",
        issue_text(&ctx.issue),
        history(&run, def, run.cursor),
        feedback_text(&run)
    );
    let out = agent::run(&mut spec, &task, &ctx.logger(), &ctx.cancel)?;
    drop(spec);
    ctx.add_usage(&step.name, &step.model, &out.usage);
    let (summary, verified, checks) = done
        .into_inner()
        .expect("lock")
        .unwrap_or((out.text, None, String::new()));
    let id = step.id.clone();
    let line = summary
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("done")
        .to_string();
    ctx.update(|r| {
        r.notes.insert(id, summary.clone());
        if let Some(v) = verified {
            r.verified = v;
            r.checks = checks.clone();
        }
    });
    Ok(match verified {
        Some(true) => format!("verified · {}", clip(&line, 120)),
        Some(false) => format!("not verified · {}", clip(&line, 120)),
        None => clip(&line, 160),
    })
}

#[allow(clippy::too_many_arguments)]
fn run_delegate(
    ctx: &Ctx,
    def: &PipelineDef,
    dlg: &Delegate,
    n: usize,
    root: &Path,
    env: &[(String, String)],
    lead: &str,
    title: &str,
    instructions: &str,
) -> anyhow::Result<String> {
    let done: Mutex<Option<String>> = Mutex::new(None);
    let finish = StageTool {
        spec: ToolSpec {
            name: "task_done".into(),
            description: "Finish your task: what you changed and how you checked it.".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "summary": { "type": "string" } },
                "required": ["summary"]
            }),
        },
        call: Box::new(|input| {
            *done.lock().expect("lock") = Some(str_of(input, "summary"));
            Reply::finish("Reported.")
        }),
    };
    let who = format!("{} {n}", dlg.name);
    let system = format!(
        "You are {who}, working for {lead} in the \"{pipeline}\" pipeline at {root}. You \
         implement one task. Do not commit. Finish with `task_done`.\n\n{instr}{skills}{notes}",
        pipeline = def.name,
        root = root.display(),
        instr = dlg.instructions.trim(),
        skills = agent::forced_skills(&dlg.skills),
        notes = agent::project_notes(root),
    );
    let mut spec = AgentSpec {
        name: who.clone(),
        model: dlg.model.clone(),
        system,
        workspace: Some((Workspace::new(root).with_env(env.to_vec()), dlg.tools.names())),
        tools: vec![
            read_workdir_tool(ctx, def),
            agent::load_skill_tool(dlg.skills_on_demand.clone()),
            finish,
        ],
        max_steps: dlg.max_steps.max(1),
        max_tokens: 32_000,
    };
    ctx.log(&who, LogKind::Step, format!("Task: {title}"));
    let out = agent::run(
        &mut spec,
        &format!("Task: {title}\n\n{instructions}"),
        &ctx.logger(),
        &ctx.cancel,
    )?;
    drop(spec);
    ctx.add_usage(&who, &dlg.model, &out.usage);
    Ok(done.into_inner().expect("lock").unwrap_or(out.text))
}

fn diff_summary(wt: &Path, cancel: &AtomicBool) -> String {
    let status = exec::git(wt, &["status", "--short"], cancel).unwrap_or_default();
    let stat = exec::git(wt, &["diff", "--stat"], cancel).unwrap_or_default();
    format!("git status:\n{}\n{}", status.trim(), stat.trim())
}

// ── Decision ────────────────────────────────────────────────────────────

type Answer = Option<(bool, String, Vec<String>)>;

fn decision_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef) -> anyhow::Result<(String, Flow)> {
    let wd = workdir(ctx, def, &step.workdir)?;
    let run = ctx.snapshot();
    let answer: Mutex<Answer> = Mutex::new(None);
    let decide = StageTool {
        spec: ToolSpec {
            name: "decide".into(),
            description: "Answer the question: yes or no, with a short reason.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "yes": { "type": "boolean" },
                    "reason": { "type": "string", "description": "One or two sentences" },
                    "notes": {
                        "type": "array", "items": { "type": "string" },
                        "description": "Optional details for later steps (e.g. affected files)"
                    }
                },
                "required": ["yes", "reason"]
            }),
        },
        call: Box::new(|input| {
            let Some(yes) = input.get("yes").and_then(Value::as_bool) else {
                return Reply::err("`yes` (boolean) is required");
            };
            *answer.lock().expect("lock") =
                Some((yes, str_of(input, "reason"), strings_of(input, "notes")));
            Reply::finish("Recorded.")
        }),
    };
    let mut spec = AgentSpec {
        name: step.name.clone(),
        model: step.model.clone(),
        system: system_prompt(def, step, &wd.path, &run, "")
            + &format!(
                "\n\nAnswer this question about the issue with `decide` (exactly once):\n{}",
                step.question.trim()
            ),
        workspace: Some((
            Workspace::new(wd.path.clone()).with_env(step_env(def, step)),
            step.tools.names(),
        )),
        tools: vec![decide, read_workdir_tool(ctx, def)],
        max_steps: step.max_steps.max(1),
        max_tokens: 4096,
    };
    let task = format!("{}{}", issue_text(&ctx.issue), history(&run, def, run.cursor));
    let out = agent::run(&mut spec, &task, &ctx.logger(), &ctx.cancel)?;
    drop(spec);
    ctx.add_usage(&step.name, &step.model, &out.usage);
    let (yes, reason, notes) = answer
        .into_inner()
        .expect("lock")
        .ok_or_else(|| anyhow!("{} finished without an answer", step.name))?;
    ctx.log(&step.name, LogKind::Step, format!("{}: {reason}", if yes { "yes" } else { "no" }));
    let id = step.id.clone();
    ctx.update(|r| {
        r.decisions.retain(|d| d.step != id);
        r.decisions.push(Decision {
            step: id.clone(),
            yes,
            reason: reason.clone(),
            notes: notes.clone(),
        });
        let mut note = reason.clone();
        if !notes.is_empty() {
            note.push_str(&format!("\n{}", notes.join(", ")));
        }
        r.notes.insert(id, note);
    });
    let summary = format!("{} · {}", if yes { "yes" } else { "no" }, clip(&reason, 140));
    if yes || step.on_no.is_empty() {
        return Ok((summary, Flow::Next));
    }
    let to = if step.on_no == "end" { None } else { def.step_index(&step.on_no) };
    Ok((summary, Flow::Jump(to)))
}

// ── Render ──────────────────────────────────────────────────────────────

/// `screens/*.html`-style glob match (`*` within a segment, `**` across).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) if p.get(1) == Some(&b'*') => {
                let rest = p[2..].strip_prefix(b"/").unwrap_or(&p[2..]);
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            (Some(b'*'), _) => (0..=s.len())
                .take_while(|i| *i == 0 || s[*i - 1] != b'/')
                .any(|i| go(&p[1..], &s[i..])),
            (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
            _ => false,
        }
    }
    go(pattern.trim().as_bytes(), path.as_bytes())
}

fn render_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef) -> anyhow::Result<String> {
    let wd = workdir(ctx, def, &step.workdir)?;
    let run = ctx.snapshot();
    let pattern = if step.files.trim().is_empty() { "**/*.html" } else { step.files.trim() };
    let base = def
        .workdir(&step.workdir)
        .map(|w| w.base_branch.clone())
        .unwrap_or_default();
    // Changed against the base: uncommitted changes plus commits on the run's branch.
    let mut files: Vec<String> =
        exec::git(&wd.path, &["status", "--porcelain", "--untracked-files=all"], &ctx.cancel)?
            .lines()
            .filter(|l| !l.starts_with(" D") && !l.starts_with("D "))
            .filter_map(|l| {
                l.get(3..)
                    .map(|p| p.rsplit(" -> ").next().unwrap_or(p).trim().to_string())
            })
            .collect();
    if wd.branch.is_some()
        && !base.is_empty()
        && let Ok(out) = exec::git(
            &wd.path,
            &["diff", "--name-only", "--diff-filter=AM", &format!("{base}...HEAD")],
            &ctx.cancel,
        )
    {
        files.extend(out.lines().map(str::to_string));
    }
    files.retain(|f| glob_match(pattern, f));
    files.sort();
    files.dedup();
    let round = run.feedback.len() + 1;
    let dir = run.dir().join("renders");
    let mut renders = Vec::new();
    for f in &files {
        let stem = Path::new(f)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| f.clone());
        ctx.log("pipeline", LogKind::Step, format!("Rendering {f}"));
        let after = dir.join(format!("{}.{stem}.r{round}.png", step.id));
        exec::render(&wd.path.join(f), &after, &ctx.cancel).with_context(|| format!("rendering {f}"))?;
        // Before: the file at the base branch, rendered from a temp copy beside it so
        // relative assets still resolve.
        let before = match exec::git(&wd.path, &["show", &format!("{base}:{f}")], &ctx.cancel) {
            Ok(text) if !base.is_empty() => {
                let p = dir.join(format!("{}.{stem}.before.png", step.id));
                if !p.exists() {
                    let tmp = wd
                        .path
                        .join(f)
                        .with_file_name(format!(".harness-before-{stem}.html"));
                    std::fs::write(&tmp, text)?;
                    let r = exec::render(&tmp, &p, &ctx.cancel);
                    let _ = std::fs::remove_file(&tmp);
                    r.with_context(|| format!("rendering {f} (before)"))?;
                }
                Some(p)
            }
            _ => None,
        };
        renders.push(Render { step: step.id.clone(), screen: stem, before, after });
    }
    let n = renders.len();
    let id = step.id.clone();
    ctx.update(|r| {
        r.renders.retain(|x| x.step != id);
        r.renders.extend(renders);
    });
    Ok(match n {
        0 => "nothing changed".into(),
        1 => "1 file rendered".into(),
        n => format!("{n} files rendered"),
    })
}

// ── Approval ────────────────────────────────────────────────────────────

fn approval_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef, i: usize) -> (String, Flow) {
    let run = ctx.snapshot();
    if step.skip_if_nothing_to_review {
        // The renders of the nearest render step before this one.
        let render = def.steps[..i].iter().rev().find(|s| s.kind == StepKind::Render);
        if let Some(r) = render
            && !run.renders.iter().any(|x| x.step == r.id)
        {
            return ("nothing to review".into(), Flow::Skip);
        }
    }
    ("waiting for you".into(), Flow::Wait)
}

// ── Command ─────────────────────────────────────────────────────────────

fn command_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef) -> anyhow::Result<String> {
    let wd = workdir(ctx, def, &step.workdir)?;
    ctx.log(&step.name, LogKind::Tool, format!("$ {}", step.command));
    let out = exec::shell(
        &wd.path,
        &step.command,
        &step_env(def, step),
        Duration::from_secs(step.timeout_secs.clamp(5, 60 * 60 * 3)),
        &ctx.cancel,
    )?;
    let tail = out.tail();
    let id = step.id.clone();
    let text = clip(
        format!("{}\n{}", out.stdout.trim(), out.stderr.trim()).trim(),
        4000,
    );
    ctx.update(|r| {
        r.notes.insert(id, text);
    });
    if !out.ok {
        ctx.log(&step.name, LogKind::Error, tail.clone());
        if !step.allow_failure {
            bail!("`{}` failed:\n{tail}", step.command);
        }
        return Ok("failed (allowed)".into());
    }
    Ok(tail
        .lines()
        .last()
        .map(|l| clip(l, 160))
        .unwrap_or_else(|| "ok".into()))
}

// ── Merge ───────────────────────────────────────────────────────────────

fn merge_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef) -> anyhow::Result<String> {
    let Some(wd) = ctx.snapshot().workdir(&step.workdir).cloned() else {
        return Ok("nothing to merge".into());
    };
    let Some(branch) = wd.branch.clone() else {
        return Ok("not a worktree".into());
    };
    let msg = format!("{}: {}", ctx.issue.key, ctx.issue.title);
    exec::git(&wd.path, &["add", "-A"], &ctx.cancel)?;
    if !exec::git(&wd.path, &["status", "--porcelain"], &ctx.cancel)?
        .trim()
        .is_empty()
    {
        exec::git(&wd.path, &["commit", "-q", "-m", &msg], &ctx.cancel)?;
    }
    let base = def
        .workdir(&step.workdir)
        .map(|w| w.base_branch.clone())
        .unwrap_or_default();
    let main = &wd.repo;
    let on_base = exec::git(main, &["rev-parse", "--abbrev-ref", "HEAD"], &ctx.cancel)
        .map(|b| b.trim() == base)
        .unwrap_or(false);
    let clean = exec::git(main, &["status", "--porcelain"], &ctx.cancel)
        .map(|s| s.trim().is_empty())
        .unwrap_or(false);
    if on_base
        && clean
        && exec::git(main, &["merge", "--no-ff", "--no-edit", &branch], &ctx.cancel).is_ok()
    {
        ctx.log("pipeline", LogKind::Step, format!("Merged {branch} into {base}"));
        Ok(format!("merged into {base}"))
    } else {
        ctx.log(
            "pipeline",
            LogKind::Step,
            format!(
                "Kept on branch {branch}: {} is not a clean checkout of {base}",
                crate::def::tilde(main)
            ),
        );
        Ok(format!("kept on {branch}"))
    }
}

// ── Pull request & report ───────────────────────────────────────────────

/// Markdown summary of a run for PR bodies and Inbox reports.
pub fn run_summary(run: &Run, def: &PipelineDef) -> String {
    let mut b = String::new();
    for (i, s) in run.stages.iter().enumerate() {
        if matches!(s.status, StageStatus::Pending) {
            continue;
        }
        let kind = def.steps.get(i).map(|d| d.kind.label()).unwrap_or("");
        b.push_str(&format!("- **{}** ({kind}): {}\n", s.name, s.summary));
    }
    let long: Vec<String> = run
        .stages
        .iter()
        .filter_map(|s| {
            let n = run.notes.get(&s.id)?;
            let is_agent = def.step(&s.id).is_some_and(|d| d.kind == StepKind::Agent);
            (is_agent && !n.trim().is_empty()).then(|| format!("### {}\n\n{}\n", s.name, n.trim()))
        })
        .collect();
    if !long.is_empty() {
        b.push('\n');
        b.push_str(&long.join("\n"));
    }
    if !run.checks.trim().is_empty() {
        b.push_str(&format!(
            "\n### Verification\n\n{}\n\n{}\n",
            if run.verified { "Verified." } else { "**Not verified.**" },
            run.checks.trim()
        ));
    }
    b
}

fn post_inbox(
    ctx: &Ctx,
    def: &PipelineDef,
    title: String,
    pr: Option<String>,
    wd: Option<&RunWorkdir>,
) -> anyhow::Result<()> {
    let run = ctx.snapshot();
    let item = InboxItem {
        id: inbox::new_id(),
        created: now(),
        chat_id: String::new(),
        chat_title: format!("{} · {}", ctx.issue.key, def.name),
        agent: def.name.clone(),
        wd_label: wd
            .map(|w| {
                let repo = w
                    .repo
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match &w.branch {
                    Some(b) => format!("{repo} · {b}"),
                    None => repo,
                }
            })
            .unwrap_or_default(),
        workdir: wd.map(|w| w.path.clone()),
        title,
        summary: run_summary(&run, def),
        outcome: if run.checks.trim().is_empty() || run.verified {
            InboxOutcome::NeedsReview
        } else {
            InboxOutcome::Blocked
        },
        links: pr.iter().map(|u| Link::new(u, None)).collect(),
        read: false,
        archived: false,
        auto: false,
    };
    inbox::add(item).context("posting to the Inbox")
}

fn pr_step(ctx: &Ctx, def: &PipelineDef, step: &StepDef) -> anyhow::Result<String> {
    let wd = workdir(ctx, def, &step.workdir)?;
    let branch = wd.branch.clone().ok_or_else(|| {
        anyhow!("pull requests need a worktree workdir (`{}` is used in place)", wd.name)
    })?;
    let base = def
        .workdir(&step.workdir)
        .map(|w| w.base_branch.clone())
        .unwrap_or_else(|| "main".into());
    let title = format!("{}: {}", ctx.issue.key, ctx.issue.title);
    let run = ctx.snapshot();

    exec::git(&wd.path, &["add", "-A"], &ctx.cancel)?;
    if !exec::git(&wd.path, &["status", "--porcelain"], &ctx.cancel)?
        .trim()
        .is_empty()
    {
        let msg = format!("{title}\n\n{}", clip(&run_summary(&run, def), 4000));
        exec::git(&wd.path, &["commit", "-q", "-m", &msg], &ctx.cancel)?;
        ctx.log("pipeline", LogKind::Step, "Committed the changes");
    }
    let ahead = exec::git(&wd.path, &["rev-list", "--count", &format!("{base}..HEAD")], &ctx.cancel)
        .map(|s| s.trim() != "0")
        .unwrap_or(false);
    if !ahead {
        bail!("nothing to open a pull request for: {branch} has no changes against {base}");
    }

    let mut pr_url = None;
    if step.push {
        exec::git(&wd.path, &["push", "-u", "origin", &branch], &ctx.cancel)?;
        ctx.log("pipeline", LogKind::Step, format!("Pushed {branch}"));
        std::fs::create_dir_all(run.dir())?;
        let body_path = run.dir().join("pr-body.md");
        let body = format!(
            "Opened by the Harness pipeline \"{}\" for **{}**: {}\n\n## Issue\n\n{}\n\n## Run\n\n{}",
            def.name,
            ctx.issue.key,
            ctx.issue.title,
            if ctx.issue.body.trim().is_empty() { "(no description)" } else { ctx.issue.body.trim() },
            run_summary(&run, def)
        );
        std::fs::write(&body_path, body)?;
        let mut cmd = std::process::Command::new("gh");
        cmd.current_dir(&wd.path)
            .args(["pr", "create", "--base", &base, "--head", &branch, "--title", &title, "--body-file"])
            .arg(&body_path);
        if step.draft {
            cmd.arg("--draft");
        }
        let out = exec::run(cmd, Duration::from_secs(120), &ctx.cancel)?;
        if !out.ok {
            bail!("gh pr create: {}", out.tail());
        }
        let url = inbox::find_pr_urls(&out.stdout)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("gh printed no PR URL: {}", out.tail()))?;
        ctx.log("pipeline", LogKind::Step, format!("Opened {url}"));
        pr_url = Some(url);
    }
    let url = pr_url.clone();
    let _ = update_issue(&ctx.issue.key, |i| i.pr_url = url);
    ctx.update(|r| r.pr_url = pr_url.clone());
    post_inbox(ctx, def, title, pr_url.clone(), Some(&wd))?;
    Ok(pr_url.unwrap_or_else(|| format!("branch {branch}")))
}

fn report_step(ctx: &Ctx, def: &PipelineDef) -> anyhow::Result<String> {
    let title = format!("{}: {}", ctx.issue.key, ctx.issue.title);
    let pr = ctx.snapshot().pr_url;
    post_inbox(ctx, def, title, pr, None)?;
    Ok("posted to the Inbox".into())
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn str_of(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn strings_of(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// HTML files (stems) in `dir/screens` — used for hints in the editor.
pub fn screens_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.join("screens"))
        .map(|e| {
            e.flatten()
                .filter_map(|e| {
                    let p: PathBuf = e.path();
                    if p.extension()? != "html" {
                        return None;
                    }
                    Some(p.file_stem()?.to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_html() {
        let html = "<html><head><style>a{b:c}</style></head><body>\n  <div>\n    <svg><path d=\"M1\"/></svg> Hi\n  </div></body></html>";
        assert_eq!(
            compact_html(html),
            "<html><head></head><body><div><svg/> Hi </div></body></html>"
        );
    }

    #[test]
    fn globs() {
        assert!(glob_match("screens/*.html", "screens/home.html"));
        assert!(!glob_match("screens/*.html", "screens/x/home.html"));
        assert!(glob_match("**/*.html", "a/b/c.html"));
        assert!(glob_match("**/*.html", "c.html"));
        assert!(!glob_match("*.html", "a/c.html"));
        assert!(glob_match("docs/**", "docs/a/b.md"));
    }
}
