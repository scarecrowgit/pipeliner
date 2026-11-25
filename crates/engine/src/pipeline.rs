//! Run engine: walks a pipeline graph, runs each node as tasks, fans out variants
//! into separate git worktrees, pauses at gates for you, and is resumable after restart.
//!
//! A run keeps a `frontier` of nodes ready to execute. Each node finishes with an
//! output (and a port for decisions/routers); the successors on that port are added
//! to the frontier. Gates block the run thread until you decide; if the app quits,
//! `Engine::start` re-drives active runs and gates pick up where they were.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use harness_core::config::expand_tilde;
use harness_core::*;
use serde_json::{json, Value};

use crate::agent::{delegate_spec, AgentRun, Handled};
use crate::git;
use crate::llm::ToolSpec;
use crate::prompt::{self, Stack};
use crate::tools::{self, ToolContext};
use crate::Engine;

/// Checks a graph before it is deployed or run.
pub fn validate(g: &PipelineGraph) -> Result<()> {
    let sources = g
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Source(_)))
        .count();
    if sources != 1 {
        bail!("a pipeline needs exactly one Source node (found {sources})");
    }
    for e in &g.edges {
        if g.node(&e.from).is_none() || g.node(&e.to).is_none() {
            bail!("a wire points at a node that no longer exists");
        }
    }
    for n in &g.nodes {
        let wd = match &n.kind {
            NodeKind::Agent(a) => a.workdir.clone(),
            NodeKind::Render(r) => r.workdir.clone(),
            NodeKind::Decision(d) => d.workdir.clone(),
            NodeKind::Tool(ToolNode::Command { workdir, .. })
            | NodeKind::Tool(ToolNode::PullRequest { workdir, .. }) => workdir.clone(),
            _ => None,
        };
        if let Some(w) = wd {
            if g.workdir(&w).is_none() {
                bail!("node “{}” uses a workdir that doesn't exist", n.name);
            }
        }
        if let NodeKind::Tool(ToolNode::PullRequest { workdir: None, .. }) = &n.kind {
            bail!(
                "node “{}” needs a workdir to open a pull request from",
                n.name
            );
        }
        if let NodeKind::Agent(a) = &n.kind {
            if a.variants > 1 {
                if let Some(w) = a.workdir.as_deref().and_then(|w| g.workdir(w)) {
                    if !w.worktree {
                        bail!("node “{}” makes {} variants: its workdir needs worktrees on so they don't overwrite each other", n.name, a.variants);
                    }
                }
            }
        }
        if let NodeKind::Render(RenderNode { workdir: None, .. }) = &n.kind {
            bail!("node “{}” needs a workdir to render from", n.name);
        }
    }
    Ok(())
}

pub fn start(
    engine: &Engine,
    pipeline_id: &str,
    input: &str,
    chat_id: Option<&str>,
) -> Result<Run> {
    let store = engine.store();
    let p = store.pipeline(pipeline_id).context("pipeline not found")?;
    validate(&p.graph)?;
    if !engine.config().is_configured() {
        bail!("add a model provider in Settings first");
    }
    let source = p
        .graph
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Source(_)))
        .unwrap()
        .id
        .clone();
    let number = store.next_number("run");
    let first: String = input
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Run")
        .to_string();
    let title = run_title(&first);
    let run = Run {
        id: new_id(),
        number,
        pipeline_id: p.id.clone(),
        pipeline_version: p.version,
        graph: p.graph.clone(),
        title,
        input: input.trim().to_string(),
        external_ref: None,
        status: Status::Running,
        chat_id: chat_id.map(String::from),
        outputs: BTreeMap::new(),
        frontier: vec![source],
        heads: BTreeMap::new(),
        worktrees: BTreeMap::new(),
        notes: BTreeMap::new(),
        pr_url: None,
        cost_usd: 0.0,
        error: None,
        created_at: now(),
        finished_at: None,
    };
    store.put_run(&run)?;
    store.log(
        "run.started",
        Some(&run.id),
        None,
        format!(
            "Run #{} of {} v{} started: {}",
            number, p.name, p.version, run.title
        ),
    );
    engine.spawn_run_thread(run.id.clone());
    Ok(run)
}

/// A short run title: the first sentence of the first line, at most 80 characters.
fn run_title(first_line: &str) -> String {
    let l = first_line.trim_start_matches('#').trim();
    let sentence = l
        .find(". ")
        .map(|i| &l[..i])
        .unwrap_or(l)
        .trim_end_matches('.');
    let mut t: String = sentence.chars().take(80).collect();
    if sentence.chars().count() > 80 {
        t = t.trim_end().to_string() + "…";
    }
    t
}

#[cfg(test)]
mod title_tests {
    #[test]
    fn first_sentence() {
        assert_eq!(
            super::run_title("Timeline hides queued work. Tasks that…"),
            "Timeline hides queued work"
        );
        assert_eq!(super::run_title("# Fix crash"), "Fix crash");
    }
}

enum Step {
    /// Node finished; follow edges on this port (None = default port).
    Next(Option<String>),
    /// Jump to these nodes instead of following edges (send back).
    Goto(Vec<Id>),
    /// Stop the run (rejected).
    Stop(Status),
}

/// Drives a run until it finishes, fails, or is cancelled. Blocks at gates.
pub fn drive(engine: &Engine, run_id: &str) -> Result<()> {
    let store = engine.store().clone();
    loop {
        let run = store.run(run_id).context("run not found")?;
        if !run.status.is_active() {
            return Ok(());
        }
        // Joins: run a node only once no other pending branch can still reach it.
        let pick = run
            .frontier
            .iter()
            .find(|f| {
                !run.frontier
                    .iter()
                    .any(|o| o != *f && reaches(&run.graph, o, f))
            })
            .or(run.frontier.first())
            .cloned();
        let Some(node_id) = pick else {
            finish(engine, &run, Status::Done, None)?;
            return Ok(());
        };
        let node = run
            .graph
            .node(&node_id)
            .cloned()
            .ok_or_else(|| anyhow!("node {node_id} is missing"))?;
        if run.status != Status::Running {
            store.update_run(run_id, |r| {
                if r.status.is_active() {
                    r.status = Status::Running;
                }
            })?;
        }
        let step = match exec_node(engine, &run, &node) {
            Ok(s) => s,
            Err(e) => {
                // Cancelled runs stop quietly.
                if store
                    .run(run_id)
                    .map(|r| r.status == Status::Cancelled)
                    .unwrap_or(true)
                {
                    return Ok(());
                }
                let msg = format!("{} failed: {e:#}", node.name);
                let run = store.update_run(run_id, |r| {
                    r.status = Status::Failed;
                    r.error = Some(msg.clone());
                    r.finished_at = Some(now());
                })?;
                store.log("run.failed", Some(run_id), None, msg.clone());
                post_to_chat(
                    engine,
                    &run,
                    &format!("Run #{} failed at {}: {e:#}", run.number, node.name),
                );
                return Ok(());
            }
        };
        if store
            .run(run_id)
            .map(|r| r.status == Status::Cancelled)
            .unwrap_or(true)
        {
            return Ok(());
        }
        match step {
            Step::Next(port) => {
                let next: Vec<Id> = run
                    .graph
                    .edges
                    .iter()
                    .filter(|e| e.from == node.id && (port.is_none() || e.port == port))
                    .map(|e| e.to.clone())
                    .collect();
                store.update_run(run_id, |r| {
                    r.frontier.retain(|n| n != &node.id);
                    for n in next {
                        if !r.frontier.contains(&n) {
                            r.frontier.push(n);
                        }
                    }
                })?;
            }
            Step::Goto(nodes) => {
                store.update_run(run_id, |r| r.frontier = nodes)?;
            }
            Step::Stop(status) => {
                let run = store.run(run_id).context("run not found")?;
                finish(engine, &run, status, None)?;
                return Ok(());
            }
        }
    }
}

fn finish(engine: &Engine, run: &Run, status: Status, error: Option<String>) -> Result<()> {
    let store = engine.store();
    let cost: f64 = store
        .tasks_for_run(&run.id)
        .iter()
        .map(|t| t.cost_usd)
        .sum();
    let run = store.update_run(&run.id, |r| {
        if r.status == Status::Cancelled && status != Status::Cancelled {
            return;
        }
        r.status = status;
        r.error = error;
        r.finished_at = Some(now());
        r.frontier.clear();
        r.cost_usd = cost;
    })?;
    let text = match (&status, &run.pr_url) {
        (Status::Done, Some(pr)) => format!(
            "Run #{} finished · {} · ${:.2}",
            run.number, pr, run.cost_usd
        ),
        (Status::Done, None) => format!("Run #{} finished · ${:.2}", run.number, run.cost_usd),
        _ => format!("Run #{} {}", run.number, status.label().to_lowercase()),
    };
    store.log("run.finished", Some(&run.id), None, text.clone());
    post_to_chat(engine, &run, &text);
    Ok(())
}

fn post_to_chat(engine: &Engine, run: &Run, text: &str) {
    if let Some(chat) = &run.chat_id {
        let mut m = Message::new(chat, Role::System, text);
        m.card = Some(Card::Run {
            run_id: run.id.clone(),
        });
        let _ = engine.store().put_message(&m);
    }
}

fn exec_node(engine: &Engine, run: &Run, node: &Node) -> Result<Step> {
    match &node.kind {
        NodeKind::Source(s) => exec_source(engine, run, node, s),
        NodeKind::Decision(d) => exec_decision(engine, run, node, d),
        NodeKind::Router(r) => exec_router(engine, run, node, r),
        NodeKind::Agent(a) => exec_agent(engine, run, node, a),
        NodeKind::Render(r) => exec_render(engine, run, node, r),
        NodeKind::Gate(g) => exec_gate(engine, run, node, g),
        NodeKind::Tool(t) => exec_tool(engine, run, node, t),
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn new_task(run: &Run, node: &Node, kind: TaskKind, actor: &str) -> Task {
    let mut t = Task::new(kind, node.name.clone());
    t.run_id = Some(run.id.clone());
    t.node_id = Some(node.id.clone());
    t.actor = actor.into();
    t.status = Status::Running;
    t.started_at = Some(now());
    t.priority = run.number as i64 * 100;
    t
}

fn close_task(store: &Store, id: &str, res: &Result<String>) {
    let _ = store.update_task(id, |t| {
        if t.status == Status::Cancelled {
            return;
        }
        match res {
            Ok(s) => {
                t.status = Status::Done;
                t.summary = s.chars().take(2000).collect();
            }
            Err(e) => {
                t.status = Status::Failed;
                t.error = Some(format!("{e:#}"));
            }
        }
        t.finished_at = Some(now());
    });
}

fn set_output(store: &Store, run_id: &str, node_id: &str, out: NodeOutput) -> Result<()> {
    store.update_run(run_id, |r| {
        r.outputs.insert(node_id.to_string(), out);
    })?;
    Ok(())
}

fn system_note(store: &Store, thread: &str, text: impl Into<String>) {
    let m = Message::new(thread, Role::System, text);
    let _ = store.put_message(&m);
}

/// Layer 4: the source item and what upstream steps produced, in graph order.
fn run_context(run: &Run, node: &Node) -> String {
    let mut s = format!("# Task\n{}\n", run.input);
    let order = upstream_order(&run.graph, &node.id);
    let mut parts = vec![];
    for id in order {
        if let (Some(n), Some(o)) = (run.graph.node(&id), run.outputs.get(&id)) {
            if o.summary.trim().is_empty() || matches!(n.kind, NodeKind::Source(_)) {
                continue;
            }
            parts.push(format!(
                "## {} ({})\n{}",
                n.name,
                n.kind.label(),
                o.summary.trim()
            ));
        }
    }
    if !parts.is_empty() {
        s.push_str("\n# Earlier steps of this run\n");
        s.push_str(&parts.join("\n\n"));
        s.push('\n');
    }
    s
}

/// Whether `to` is reachable from `from` along edges.
fn reaches(g: &PipelineGraph, from: &str, to: &str) -> bool {
    let mut seen: Vec<&str> = vec![];
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if seen.contains(&n) {
            continue;
        }
        seen.push(n);
        for e in g.edges.iter().filter(|e| e.from == n) {
            stack.push(&e.to);
        }
    }
    false
}

/// Upstream nodes of `id`, nearest last (BFS over incoming edges, reversed).
fn upstream_order(g: &PipelineGraph, id: &str) -> Vec<Id> {
    let mut seen = vec![];
    let mut q: VecDeque<Id> = g.incoming(id).iter().map(|e| e.from.clone()).collect();
    while let Some(n) = q.pop_front() {
        if seen.contains(&n) {
            continue;
        }
        seen.push(n.clone());
        for e in g.incoming(&n) {
            q.push_back(e.from.clone());
        }
    }
    seen.reverse();
    seen
}

struct Place {
    cwd: PathBuf,
    branch: Option<String>,
    /// Commit the step started from (diff base).
    base: Option<String>,
    shell: String,
    env: BTreeMap<String, String>,
}

/// Resolves (and creates if needed) the folder a step works in.
/// `variant`: Some(i) for fan-out variants (each gets its own worktree/branch).
fn place(
    engine: &Engine,
    run: &Run,
    node: &Node,
    workdir_id: &str,
    variant: Option<u32>,
) -> Result<Place> {
    let store = engine.store();
    let wd = run
        .graph
        .workdir(workdir_id)
        .context("workdir not found")?
        .clone();
    let root = expand_tilde(&wd.path);
    let shell = if wd.shell.is_empty() {
        crate::chat::shell_for(Some(&root))
    } else {
        wd.shell.clone()
    };
    if !wd.worktree || !git::is_repo(&root) {
        return Ok(Place {
            cwd: root,
            branch: None,
            base: None,
            shell,
            env: wd.env.clone(),
        });
    }
    let sub = git::subdir(&wd).unwrap_or_default();
    let run_dir = harness_core::paths::worktrees_dir().join(format!("run-{}", run.number));
    let slug = git::slug(&run.title);
    let head = run
        .heads
        .get(workdir_id)
        .cloned()
        .unwrap_or_else(|| wd.base_branch.clone());
    let (dest, branch) = match variant {
        Some(i) => {
            let l = variant_letter(i).to_ascii_lowercase();
            (
                run_dir.join(format!("{}-{}-{l}", workdir_id, git::slug(&node.id))),
                format!(
                    "{}run-{}-{}-{}-{l}",
                    wd.branch_prefix,
                    run.number,
                    slug,
                    git::slug(&node.id)
                ),
            )
        }
        None => {
            if let Some(p) = run.worktrees.get(workdir_id) {
                let p = PathBuf::from(p);
                if p.exists() {
                    let cwd = p.join(&sub);
                    let branch = git::git(&p, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
                    let base = git::head(&p).ok();
                    return Ok(Place {
                        cwd,
                        branch,
                        base,
                        shell,
                        env: wd.env.clone(),
                    });
                }
            }
            (
                run_dir.join(workdir_id),
                format!("{}run-{}-{}", wd.branch_prefix, run.number, slug),
            )
        }
    };
    let fresh = !dest.exists();
    git::add_worktree(&wd, &dest, &branch, &head)?;
    if fresh {
        store.log(
            "worktree.created",
            Some(&run.id),
            None,
            format!("Worktree {} on {}", dest.display(), branch),
        );
        if !wd.setup.trim().is_empty() {
            let never = std::sync::atomic::AtomicBool::new(false);
            let out = tools::run_command(
                &dest.join(&sub),
                "",
                &wd.env,
                &wd.setup,
                Duration::from_secs(1800),
                &never,
            )?;
            if !out.starts_with("exit code 0") {
                store.log(
                    "worktree.setup_failed",
                    Some(&run.id),
                    None,
                    tools::truncate_middle(&out, 2000),
                );
            }
        }
    }
    if variant.is_none() {
        store.update_run(&run.id, |r| {
            r.worktrees
                .insert(workdir_id.to_string(), dest.display().to_string());
            r.heads.insert(workdir_id.to_string(), branch.clone());
        })?;
    }
    // Variants diff against where they forked, so a send-back round still shows the whole change.
    let base = if variant.is_some() {
        git::git(&dest, &["merge-base", "HEAD", &head])
            .ok()
            .or_else(|| git::head(&dest).ok())
    } else {
        git::head(&dest).ok()
    };
    Ok(Place {
        cwd: dest.join(&sub),
        branch: Some(branch),
        base,
        shell,
        env: wd.env.clone(),
    })
}

fn other_workdirs(run: &Run, except: Option<&str>) -> Vec<(String, String)> {
    run.graph
        .workdirs
        .iter()
        .filter(|w| Some(w.id.as_str()) != except)
        .map(|w| {
            let p = run
                .worktrees
                .get(&w.id)
                .cloned()
                .unwrap_or_else(|| expand_tilde(&w.path).display().to_string());
            (w.id.clone(), p)
        })
        .collect()
}

fn wait_for_decision(engine: &Engine, run_id: &str, task_id: &str) -> Result<Decision> {
    let store = engine.store();
    let mut last_check = Instant::now();
    loop {
        if let Some(d) = store.take_decision(task_id) {
            return Ok(d);
        }
        if last_check.elapsed() > Duration::from_secs(1) {
            last_check = Instant::now();
            let r = store.run(run_id).context("run not found")?;
            if !r.status.is_active() {
                bail!("run is no longer active");
            }
            if store
                .task(task_id)
                .map(|t| t.status != Status::Waiting)
                .unwrap_or(true)
            {
                bail!("cancelled");
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

// ---------------------------------------------------------------------------
// nodes
// ---------------------------------------------------------------------------

fn exec_source(engine: &Engine, run: &Run, node: &Node, s: &SourceNode) -> Result<Step> {
    let store = engine.store();
    let mut input = run.input.clone();
    let mut ext = None;
    // `#123` or an issue URL with a configured repo: fetch the issue with gh.
    if !s.github_repo.is_empty() {
        let num = input
            .trim()
            .trim_start_matches('#')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        if num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty() {
            let out = git::run(
                &std::env::temp_dir(),
                "gh",
                &[
                    "issue",
                    "view",
                    &num,
                    "--repo",
                    &s.github_repo,
                    "--json",
                    "title,body,url",
                ],
            )?;
            let v: Value = serde_json::from_str(&out)?;
            input = format!(
                "{}\n\n{}",
                v["title"].as_str().unwrap_or(""),
                v["body"].as_str().unwrap_or("")
            );
            ext = v["url"].as_str().map(String::from);
            let title: String = v["title"].as_str().unwrap_or("").chars().take(80).collect();
            store.update_run(&run.id, |r| {
                r.input = input.clone();
                r.title = title;
                r.external_ref = ext.clone();
            })?;
        }
    }
    let mut t = new_task(run, node, TaskKind::Source, "source");
    t.summary = input.chars().take(400).collect();
    t.status = Status::Done;
    t.finished_at = Some(now());
    store.put_task(&t)?;
    set_output(
        store,
        &run.id,
        &node.id,
        NodeOutput {
            summary: String::new(),
            port: None,
            value: json!({"input": input, "ref": ext}),
            task_id: Some(t.id),
        },
    )?;
    Ok(Step::Next(None))
}

fn exec_router(engine: &Engine, run: &Run, node: &Node, r: &RouterNode) -> Result<Step> {
    let store = engine.store();
    let field = |f: &str| -> String {
        match f {
            "input" => run.input.clone(),
            "title" => run.title.clone(),
            other => other
                .strip_prefix("out.")
                .and_then(|id| run.outputs.get(id))
                .map(|o| o.summary.clone())
                .unwrap_or_default(),
        }
    };
    let mut port = "else".to_string();
    let mut why = "no rule matched".to_string();
    for (i, rule) in r.rules.iter().enumerate() {
        let v = field(&rule.field);
        let hit = match rule.op {
            RuleOp::Contains => v.to_lowercase().contains(&rule.value.to_lowercase()),
            RuleOp::Equals => v.trim().eq_ignore_ascii_case(rule.value.trim()),
            RuleOp::StartsWith => v
                .trim()
                .to_lowercase()
                .starts_with(&rule.value.to_lowercase()),
            RuleOp::Matches => simple_regex(&rule.value, &v),
        };
        if hit {
            port = format!("rule{}", i + 1);
            why = format!(
                "rule {}: {} {:?} “{}”",
                i + 1,
                rule.field,
                rule.op,
                rule.value
            );
            break;
        }
    }
    let mut t = new_task(run, node, TaskKind::Router, "router");
    t.status = Status::Done;
    t.summary = why.clone();
    t.finished_at = Some(now());
    store.put_task(&t)?;
    set_output(
        store,
        &run.id,
        &node.id,
        NodeOutput {
            summary: why,
            port: Some(port.clone()),
            value: Value::Null,
            task_id: Some(t.id),
        },
    )?;
    Ok(Step::Next(Some(port)))
}

/// `a|b|c` alternatives, case-insensitive substring (keeps us free of a regex dependency).
fn simple_regex(pattern: &str, text: &str) -> bool {
    let t = text.to_lowercase();
    pattern
        .split('|')
        .any(|p| !p.is_empty() && t.contains(&p.to_lowercase()))
}

fn exec_decision(engine: &Engine, run: &Run, node: &Node, d: &DecisionNode) -> Result<Step> {
    let store = engine.store().clone();
    let config = engine.config();
    let model = if d.model.is_empty() {
        config.fast().to_string()
    } else {
        d.model.clone()
    };
    let mut t = new_task(run, node, TaskKind::Decision, &Config::short_model(&model));
    t.model = model.clone();
    store.put_task(&t)?;
    store.log(
        "task.started",
        Some(&run.id),
        Some(&t.id),
        format!("{} started", node.name),
    );

    // Decisions only read: use the workdir as it is (or the run's worktree).
    let cwd = d.workdir.as_deref().and_then(|w| {
        run.worktrees
            .get(w)
            .map(PathBuf::from)
            .or_else(|| run.graph.workdir(w).map(|wd| expand_tilde(&wd.path)))
    });
    let cancel = engine.cancel_flag(&t.id);
    let agent = AgentRun {
        engine,
        task_id: t.id.clone(),
        thread_id: t.id.clone(),
        actor: node.name.clone(),
        model: model.clone(),
        stack: Stack {
            rules: config.rules.clone(),
            role: "You make one yes/no decision in a pipeline. Investigate only as much as needed, then call `answer` exactly once.".into(),
            instructions: format!("Question: {}", d.question),
            context: run_context(run, node),
            environment: prompt::environment(cwd.as_deref(), "", &[]),
            message: format!("Answer the question for this task: {}", d.question),
            ..Default::default()
        },
        tools: ToolContext {
            cwd,
            shell: String::new(),
            env: Default::default(),
            access: ToolAccess::ReadOnly,
            workdirs: vec![],
            skills_on_demand: vec![],
            config: config.clone(),
            cancel,
        },
        extra_tools: vec![ToolSpec {
            name: "answer",
            description: "Give your decision.".into(),
            parameters: json!({"type":"object","properties":{"yes":{"type":"boolean"},"reason":{"type":"string","description":"one or two sentences"}},"required":["yes","reason"]}),
        }],
        history: vec![],
        max_steps: d.max_steps.max(2),
    };
    let res = agent.run(&mut |_, call, args| {
        (call.name == "answer").then(|| Handled::Stop("recorded".into(), args.clone()))
    });
    engine.drop_cancel_flag(&t.id);
    let out = res.and_then(|o| {
        if o.value.is_null() {
            // The model answered in text: read yes/no from it.
            let l = o.text.trim().to_lowercase();
            let yes = l.starts_with("yes");
            if !yes && !l.starts_with("no") {
                bail!(
                    "the model did not answer yes or no: {}",
                    o.text.chars().take(200).collect::<String>()
                );
            }
            Ok(json!({"yes": yes, "reason": o.text}))
        } else {
            Ok(o.value)
        }
    });
    let summary_res: Result<String> = match &out {
        Ok(v) => Ok(format!(
            "{} — {}",
            if v["yes"].as_bool().unwrap_or(false) {
                "Yes"
            } else {
                "No"
            },
            v["reason"].as_str().unwrap_or("").trim()
        )),
        Err(e) => Err(anyhow!("{e:#}")),
    };
    close_task(&store, &t.id, &summary_res);
    let v = out?;
    let summary = summary_res.ok();
    let port = if v["yes"].as_bool().unwrap_or(false) {
        "yes"
    } else {
        "no"
    };
    set_output(
        &store,
        &run.id,
        &node.id,
        NodeOutput {
            summary: summary.unwrap_or_default(),
            port: Some(port.into()),
            value: v,
            task_id: Some(t.id),
        },
    )?;
    Ok(Step::Next(Some(port.into())))
}

/// Resolved agent settings for an Agent node.
struct Resolved {
    name: String,
    model: String,
    role: String,
    tools: ToolAccess,
    skills: Vec<String>,
    skills_on_demand: Vec<String>,
    max_steps: u32,
}

fn resolve_agent(engine: &Engine, node: &Node, a: &AgentNode) -> Resolved {
    let def = a.agent.as_deref().and_then(|id| engine.store().agent(id));
    let config = engine.config();
    let mut r = Resolved {
        name: node.name.clone(),
        model: config.default_model.clone(),
        role: String::new(),
        tools: ToolAccess::Full,
        skills: vec![],
        skills_on_demand: vec![],
        max_steps: a.max_steps,
    };
    if let Some(d) = def {
        r.name = d.name;
        r.model = d.model;
        r.role = d.role;
        r.tools = d.tools;
        r.skills = d.skills;
        r.skills_on_demand = d.skills_on_demand;
        r.max_steps = d.max_steps.max(a.max_steps);
    }
    if !a.model.is_empty() {
        r.model = a.model.clone();
    }
    if let Some(t) = a.tools {
        r.tools = t;
    }
    for s in &a.skills {
        if !r.skills.contains(s) {
            r.skills.push(s.clone());
        }
    }
    for s in &a.skills_on_demand {
        if !r.skills_on_demand.contains(s) {
            r.skills_on_demand.push(s.clone());
        }
    }
    r
}

fn exec_agent(engine: &Engine, run: &Run, node: &Node, a: &AgentNode) -> Result<Step> {
    let store = engine.store().clone();
    let r = resolve_agent(engine, node, a);
    let n = a.variants.max(1);
    let note = run.notes.get(&node.id).cloned();

    // Prepare places first (worktree creation is sequential and git-locked).
    let mut places = vec![];
    for i in 0..n {
        let variant = (n > 1).then_some(i);
        let p = match &a.workdir {
            Some(w) => Some(place(engine, run, node, w, variant)?),
            None => None,
        };
        places.push(p);
    }
    let run = store.run(&run.id).context("run not found")?;

    let results: Vec<Result<Value>> = std::thread::scope(|s| {
        let handles: Vec<_> = places
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let run = &run;
                let r = &r;
                let note = note.clone();
                s.spawn(move || {
                    run_variant(
                        engine,
                        run,
                        node,
                        a,
                        r,
                        (n > 1).then_some(i as u32),
                        p,
                        note,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow!("variant thread panicked")))
            })
            .collect()
    });

    let mut variants = vec![];
    let mut errors = vec![];
    for (i, res) in results.into_iter().enumerate() {
        match res {
            Ok(v) => variants.push(v),
            Err(e) => errors.push(format!("{}: {e:#}", variant_letter(i as u32))),
        }
    }
    if variants.is_empty() {
        bail!("{}", errors.join("; "));
    }
    let summary = if n == 1 {
        variants[0]["summary"].as_str().unwrap_or("").to_string()
    } else {
        format!(
            "{} variants ready{}",
            variants.len(),
            if errors.is_empty() {
                String::new()
            } else {
                format!(" ({} failed)", errors.len())
            }
        )
    };
    let task_id = variants[0]["task_id"].as_str().map(String::from);
    set_output(
        &store,
        &run.id,
        &node.id,
        NodeOutput {
            summary,
            port: None,
            value: json!({"variants": variants, "workdir": a.workdir}),
            task_id,
        },
    )?;
    // The send-back note is used up only once the step succeeded.
    store.update_run(&run.id, |r| {
        r.notes.remove(&node.id);
    })?;
    Ok(Step::Next(None))
}

#[allow(clippy::too_many_arguments)]
fn run_variant(
    engine: &Engine,
    run: &Run,
    node: &Node,
    a: &AgentNode,
    r: &Resolved,
    variant: Option<u32>,
    place: Option<Place>,
    note: Option<String>,
) -> Result<Value> {
    let store = engine.store().clone();
    let config = engine.config();
    let mut t = new_task(run, node, TaskKind::Agent, &r.name);
    if let Some(i) = variant {
        t.title = format!("{} {}", node.name, variant_letter(i));
        t.variant = Some(i);
    }
    t.model = r.model.clone();
    t.cwd = place.as_ref().map(|p| p.cwd.display().to_string());
    t.branch = place.as_ref().and_then(|p| p.branch.clone());
    t.status = Status::Queued;
    let cancel = engine.cancel_flag(&t.id);
    store.put_task(&t)?;
    let _slot = engine.acquire_slot();
    if cancel.load(Ordering::SeqCst)
        || store
            .run(&run.id)
            .map(|r| !r.status.is_active())
            .unwrap_or(true)
    {
        engine.drop_cancel_flag(&t.id);
        bail!("cancelled");
    }
    let start = Instant::now();
    let started = store.update_task(&t.id, |t| {
        if t.status == Status::Queued {
            t.status = Status::Running;
            t.started_at = Some(now());
        }
    })?;
    if started.status != Status::Running {
        engine.drop_cancel_flag(&t.id);
        bail!("cancelled");
    }
    store.log(
        "task.started",
        Some(&run.id),
        Some(&t.id),
        format!("{} started", t.title),
    );
    if let Some(p) = &place {
        system_note(
            &store,
            &t.id,
            format!(
                "Working in {}{}",
                p.cwd.display(),
                p.branch
                    .as_ref()
                    .map(|b| format!(" on branch {b}"))
                    .unwrap_or_default()
            ),
        );
    }

    let mut instructions = a.instructions.clone();
    if let Some(i) = variant {
        let hint = a.variant_hints.get(i as usize).cloned().unwrap_or_default();
        instructions.push_str(&format!(
            "\n\nYou are variant {} of {} produced in parallel; the user will compare them side by side and pick one.{}",
            variant_letter(i),
            a.variants,
            if hint.is_empty() { " Take a distinct approach from what an obvious first attempt would be.".into() } else { format!(" Strategy for this variant: {hint}") }
        ));
    }
    let mut message = "Do this step for the task above. When you are done, reply with a short report: what you did, which files changed, how you checked it, anything left.".to_string();
    if let Some(n) = &note {
        message = format!(
            "The user sent your previous attempt back with this note:\n\n{n}\n\nYour earlier work is still in the working directory. Revise it accordingly, then reply with a short report."
        );
    }
    let workdirs = other_workdirs(run, a.workdir.as_deref());
    let mut extra = vec![];
    if let Some(d) = &a.delegate {
        extra.push(delegate_spec(&d.name));
    }
    let agent = AgentRun {
        engine,
        task_id: t.id.clone(),
        thread_id: t.id.clone(),
        actor: r.name.clone(),
        model: r.model.clone(),
        stack: Stack {
            rules: config.rules.clone(),
            role: r.role.clone(),
            instructions,
            context: run_context(run, node),
            environment: prompt::environment(
                place.as_ref().map(|p| p.cwd.as_path()),
                place.as_ref().map(|p| p.shell.as_str()).unwrap_or(""),
                &workdirs,
            ),
            skills: r.skills.clone(),
            skills_on_demand: r.skills_on_demand.clone(),
            message,
        },
        tools: ToolContext {
            cwd: place.as_ref().map(|p| p.cwd.clone()),
            shell: place.as_ref().map(|p| p.shell.clone()).unwrap_or_default(),
            env: place.as_ref().map(|p| p.env.clone()).unwrap_or_default(),
            access: if place.is_some() {
                r.tools
            } else {
                ToolAccess::None
            },
            workdirs,
            skills_on_demand: r.skills_on_demand.clone(),
            config: config.clone(),
            cancel: cancel.clone(),
        },
        extra_tools: extra,
        history: vec![],
        max_steps: r.max_steps,
    };
    let delegate_cfg = a.delegate.clone();
    let res = agent.run(&mut |run, call, args| {
        if call.name != "delegate" {
            return None;
        }
        let d = delegate_cfg.as_ref()?;
        let role = if d.instructions.is_empty() {
            crate::chat::DELEGATE_ROLE
        } else {
            d.instructions.as_str()
        };
        Some(crate::chat::delegate(
            run,
            args,
            &d.model,
            &d.name,
            role,
            &d.skills_on_demand,
            d.max_steps,
        ))
    });
    engine.drop_cancel_flag(&t.id);
    if cancel.load(Ordering::SeqCst) {
        bail!("cancelled");
    }
    let out = match res {
        Ok(o) => o,
        Err(e) => {
            close_task(&store, &t.id, &Err(anyhow!("{e:#}")));
            return Err(e);
        }
    };

    // Capture the work: commit in the worktree and diff against the starting point.
    let mut diff = String::new();
    if let Some(p) = &place {
        if let Some(base) = &p.base {
            diff = git::diff_against(&p.cwd, base);
            let first = out
                .text
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim();
            let msg = format!(
                "{}: {}\n\nRun #{} · {}",
                node.name,
                first.chars().take(72).collect::<String>(),
                run.number,
                run.title
            );
            let _ = git::commit_all(&p.cwd, &msg);
        }
    }
    let (files, add, del) = git::diff_stat(&diff);
    let summary = out.text.clone();
    store.update_task(&t.id, |t| {
        t.output = json!({"files": files, "added": add, "removed": del});
    })?;
    close_task(&store, &t.id, &Ok(summary.clone()));
    let cost = store.task(&t.id).map(|t| t.cost_usd).unwrap_or(0.0);
    Ok(json!({
        "index": variant.unwrap_or(0),
        "task_id": t.id,
        "summary": summary,
        "model": r.model,
        "cwd": place.as_ref().map(|p| p.cwd.display().to_string()),
        "root": place.as_ref().and_then(|p| p.branch.as_ref().map(|_| worktree_root(&p.cwd))),
        "branch": place.as_ref().and_then(|p| p.branch.clone()),
        "base": place.as_ref().and_then(|p| p.base.clone()),
        "diff": diff,
        "cost": cost,
        "secs": start.elapsed().as_secs_f64(),
        "images": [],
    }))
}

fn worktree_root(cwd: &Path) -> String {
    git::repo_root(cwd)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| cwd.display().to_string())
}

/// Nearest upstream output that carries variants (agent or render node).
fn upstream_variants(run: &Run, node_id: &str) -> Option<(Id, Value)> {
    for id in upstream_order(&run.graph, node_id).into_iter().rev() {
        if let Some(o) = run.outputs.get(&id) {
            if o.value
                .get("variants")
                .map(|v| v.is_array())
                .unwrap_or(false)
            {
                return Some((id, o.value.clone()));
            }
        }
    }
    None
}

fn exec_render(engine: &Engine, run: &Run, node: &Node, rn: &RenderNode) -> Result<Step> {
    let store = engine.store().clone();
    let t = new_task(run, node, TaskKind::Render, "render");
    store.put_task(&t)?;
    let id = t.id.clone();
    let res = render_inner(engine, run, node, rn, t);
    if let Err(e) = &res {
        close_task(&store, &id, &Err(anyhow!("{e:#}")));
    }
    res
}

fn render_inner(
    engine: &Engine,
    run: &Run,
    node: &Node,
    rn: &RenderNode,
    mut t: Task,
) -> Result<Step> {
    let store = engine.store().clone();
    let config = engine.config();
    let (src_id, mut value) = match upstream_variants(run, &node.id) {
        Some(x) => x,
        None => {
            // Nothing upstream: render the run's worktree as a single variant.
            let w = rn.workdir.clone().context("render needs a workdir")?;
            let p = place(engine, run, node, &w, None)?;
            (
                String::new(),
                json!({"variants":[{"index":0,"cwd":p.cwd.display().to_string(),"base":p.base,"summary":"","images":[]}]}),
            )
        }
    };
    let pattern =
        glob::Pattern::new(&rn.files).unwrap_or_else(|_| glob::Pattern::new("**/*.html").unwrap());
    let mut total = 0;
    let mut errors = vec![];
    let cmd = if rn.command.is_empty() {
        config.render_command.clone()
    } else {
        rn.command.clone()
    };
    if let Some(vs) = value["variants"].as_array_mut() {
        for v in vs.iter_mut() {
            let Some(cwd) = v["cwd"].as_str().map(PathBuf::from) else {
                continue;
            };
            let base = v["base"].as_str().unwrap_or("HEAD~1").to_string();
            // Changed files relative to the worktree root; keep those matching the glob.
            let root = git::repo_root(&cwd).unwrap_or(cwd.clone());
            let changed = if v["base"].is_null() {
                vec![]
            } else {
                git::changed_files(&root, &base)
            };
            let letter = variant_letter(v["index"].as_u64().unwrap_or(0) as u32);
            let mut images = vec![];
            for f in changed {
                let abs = root.join(&f);
                let rel_to_cwd = abs
                    .strip_prefix(&cwd)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(PathBuf::from(&f));
                if !abs.is_file() || !(pattern.matches_path(&rel_to_cwd) || pattern.matches(&f)) {
                    continue;
                }
                let (w, h) = design_size(&abs).unwrap_or((rn.width, rn.height));
                let out = harness_core::paths::renders_dir()
                    .join(format!("run-{}", run.number))
                    .join(format!("{}-{}", git::slug(&node.name), letter))
                    .join(format!("{}.png", f.replace('/', "__")));
                match crate::render::render(&cmd, &abs, &out, w, h) {
                    Ok(()) => {
                        images.push(json!(out.display().to_string()));
                        total += 1;
                    }
                    Err(e) => errors.push(format!("{f}: {e:#}")),
                }
            }
            v["images"] = json!(images);
        }
    }
    let summary = if total == 0 && errors.is_empty() {
        "No mockup files changed".to_string()
    } else {
        format!(
            "Rendered {total} screen{}{}",
            if total == 1 { "" } else { "s" },
            if errors.is_empty() {
                String::new()
            } else {
                format!("; {} failed", errors.len())
            }
        )
    };
    if !errors.is_empty() {
        system_note(&store, &t.id, errors.join("\n"));
    }
    t.status = Status::Done;
    t.summary = summary.clone();
    t.finished_at = Some(now());
    store.put_task(&t)?;
    let _ = src_id;
    set_output(
        &store,
        &run.id,
        &node.id,
        NodeOutput {
            summary: String::new(),
            port: None,
            value,
            task_id: Some(t.id),
        },
    )?;
    Ok(Step::Next(None))
}

/// `<meta name="design:size" content="1440x2080">` in a mockup sets its render size.
fn design_size(p: &Path) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(p).ok()?;
    let i = text.find("name=\"design:size\"")?;
    let rest = &text[i..];
    let c = rest.find("content=\"")? + 9;
    let end = rest[c..].find('"')? + c;
    let (w, h) = rest[c..end].split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn exec_gate(engine: &Engine, run: &Run, node: &Node, g: &GateNode) -> Result<Step> {
    let store = engine.store().clone();
    // Resume: a gate task for this node already waiting.
    let existing = store
        .tasks_for_run(&run.id)
        .into_iter()
        .find(|t| t.node_id.as_deref() == Some(&node.id) && t.status == Status::Waiting);
    let task = match existing {
        Some(t) => t,
        None => create_gate(engine, run, node, g)?,
    };
    store.update_run(&run.id, |r| {
        if r.status.is_active() {
            r.status = Status::Waiting;
        }
    })?;
    let decision = wait_for_decision(engine, &run.id, &task.id)?;
    store.update_run(&run.id, |r| {
        if r.status.is_active() {
            r.status = Status::Running;
        }
    })?;
    let run = store.run(&run.id).context("run not found")?;
    if !run.status.is_active() {
        bail!("cancelled");
    }
    let variants = store.variants(&task.id);

    let done = |summary: String| {
        let _ = store.update_task(&task.id, |t| {
            t.status = Status::Done;
            t.summary = summary;
            t.finished_at = Some(now());
        });
    };
    // Normalize the decision.
    enum D {
        Accept(u32),
        Back(String, Option<Id>),
        Reject(String),
    }
    let d = match decision.kind {
        DecisionKind::Pick { index } => D::Accept(index),
        DecisionKind::Approve => D::Accept(0),
        DecisionKind::SendBack { note, target } => D::Back(note, target),
        DecisionKind::Reply { text } => D::Back(text, None),
        DecisionKind::Reject { reason } => D::Reject(reason),
    };
    match d {
        D::Accept(index) if g.kind == GateKind::Pick => {
            let picked = variants.iter().find(|v| v.index == index).cloned();
            if let Some(mut v) = picked.clone() {
                v.picked = true;
                store.put_variant(&v)?;
            }
            // The picked variant's branch and worktree become the run's state for that workdir.
            if let Some((src, val)) = upstream_variants(&run, &node.id) {
                let wd = val["workdir"].as_str().map(String::from).or_else(|| {
                    run.graph.node(&src).and_then(|n| match &n.kind {
                        NodeKind::Render(r) => r.workdir.clone(),
                        NodeKind::Agent(a) => a.workdir.clone(),
                        _ => None,
                    })
                });
                let chosen = val["variants"].as_array().and_then(|a| {
                    a.iter()
                        .find(|v| v["index"].as_u64() == Some(index as u64))
                        .cloned()
                });
                if let (Some(wd), Some(c)) = (wd, chosen) {
                    if let (Some(branch), Some(root)) = (c["branch"].as_str(), c["root"].as_str()) {
                        store.update_run(&run.id, |r| {
                            r.heads.insert(wd.clone(), branch.to_string());
                            r.worktrees.insert(wd.clone(), root.to_string());
                        })?;
                    }
                }
            }
            let letter = variant_letter(index);
            let summary = match &picked {
                Some(v) if variants.len() > 1 => {
                    format!("You picked {letter}: {}", first_line(&v.summary))
                }
                Some(v) => format!("You approved: {}", first_line(&v.summary)),
                None => "You approved".into(),
            };
            done(summary.clone());
            set_output(
                &store,
                &run.id,
                &node.id,
                NodeOutput {
                    summary,
                    port: None,
                    value: json!({"picked": index}),
                    task_id: Some(task.id.clone()),
                },
            )?;
            Ok(Step::Next(None))
        }
        D::Accept(_) => {
            // Approve PR: merge it.
            let url = run
                .pr_url
                .clone()
                .context("this run has no pull request to merge")?;
            let cwd = run
                .worktrees
                .values()
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            match git::merge_pr(&cwd, &url, "squash") {
                Ok(_) => {
                    done(format!("You approved; merged {url}"));
                    store.log(
                        "pr.merged",
                        Some(&run.id),
                        Some(&task.id),
                        format!("Merged {url}"),
                    );
                    set_output(
                        &store,
                        &run.id,
                        &node.id,
                        NodeOutput {
                            summary: format!("Merged {url}"),
                            port: None,
                            value: Value::Null,
                            task_id: Some(task.id.clone()),
                        },
                    )?;
                    Ok(Step::Next(None))
                }
                Err(e) => {
                    // Ask again so you can retry, or merge on GitHub and approve.
                    done(format!("Merge failed: {e:#}"));
                    store.update_task(&task.id, |t| {
                        t.status = Status::Failed;
                        t.error = Some(format!("Merge failed: {e:#}"));
                    })?;
                    Ok(Step::Goto(vec![node.id.clone()]))
                }
            }
        }
        D::Back(note, target) => {
            let target = target.or_else(|| g.back_to.clone()).or_else(|| {
                upstream_order(&run.graph, &node.id)
                    .into_iter()
                    .rev()
                    .find(|id| {
                        matches!(
                            run.graph.node(id).map(|n| &n.kind),
                            Some(NodeKind::Agent(_))
                        )
                    })
            });
            let target = target.context("nothing upstream to send this back to")?;
            done(format!("Sent back: {}", first_line(&note)));
            store.update_run(&run.id, |r| {
                r.notes.insert(target.clone(), note.clone());
            })?;
            let tname = run
                .graph
                .node(&target)
                .map(|n| n.name.clone())
                .unwrap_or_default();
            store.log(
                "run.sent_back",
                Some(&run.id),
                Some(&task.id),
                format!("Sent back to {tname}"),
            );
            Ok(Step::Goto(vec![target]))
        }
        D::Reject(reason) => {
            done(format!(
                "Rejected{}",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(": {reason}")
                }
            ));
            Ok(Step::Stop(Status::Cancelled))
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches('#')
        .trim()
        .chars()
        .take(140)
        .collect()
}

fn create_gate(engine: &Engine, run: &Run, node: &Node, g: &GateNode) -> Result<Task> {
    let store = engine.store().clone();
    let mut t = new_task(run, node, TaskKind::Gate, "you");
    t.status = Status::Waiting;
    t.title = if g.prompt.is_empty() {
        match g.kind {
            GateKind::Pick => format!("{} · {}", node.name, run.title),
            GateKind::ApprovePr => format!("Approve PR · {}", run.title),
        }
    } else {
        g.prompt.clone()
    };
    store.put_task(&t)?;
    let mut variants = vec![];
    match g.kind {
        GateKind::Pick => {
            if let Some((_, val)) = upstream_variants(run, &node.id) {
                for v in val["variants"].as_array().cloned().unwrap_or_default() {
                    let index = v["index"].as_u64().unwrap_or(0) as u32;
                    variants.push(Variant {
                        id: new_id(),
                        gate_task_id: t.id.clone(),
                        run_id: Some(run.id.clone()),
                        source_task_id: v["task_id"].as_str().map(String::from),
                        index,
                        title: first_line(v["summary"].as_str().unwrap_or("")),
                        summary: v["summary"].as_str().unwrap_or("").to_string(),
                        model: v["model"].as_str().unwrap_or("").to_string(),
                        cost_usd: v["cost"].as_f64().unwrap_or(0.0),
                        duration_secs: v["secs"].as_f64().unwrap_or(0.0),
                        branch: v["branch"].as_str().map(String::from),
                        cwd: v["cwd"].as_str().map(String::from),
                        diff: v["diff"].as_str().unwrap_or("").to_string(),
                        images: v["images"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        picked: false,
                    });
                }
            }
        }
        GateKind::ApprovePr => {
            let url = run.pr_url.clone().unwrap_or_default();
            let diff = run
                .worktrees
                .values()
                .next()
                .and_then(|p| {
                    let wd = run.graph.workdirs.first()?;
                    Some(git::diff_against(
                        Path::new(p),
                        &format!("origin/{}", wd.base_branch),
                    ))
                })
                .unwrap_or_default();
            let summary = run
                .outputs
                .iter()
                .filter_map(|(id, o)| {
                    let n = run.graph.node(id)?;
                    matches!(n.kind, NodeKind::Agent(_))
                        .then(|| format!("**{}** — {}", n.name, o.summary.trim()))
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            variants.push(Variant {
                id: new_id(),
                gate_task_id: t.id.clone(),
                run_id: Some(run.id.clone()),
                source_task_id: None,
                index: 0,
                title: if url.is_empty() {
                    "Pull request".into()
                } else {
                    url.clone()
                },
                summary,
                model: String::new(),
                cost_usd: run.cost_usd,
                duration_secs: (now() - run.created_at).num_seconds() as f64,
                branch: run.heads.values().next().cloned(),
                cwd: run.worktrees.values().next().cloned(),
                diff,
                images: vec![],
                picked: false,
            });
        }
    }
    for v in &variants {
        store.put_variant(v)?;
    }
    store.log(
        "gate.waiting",
        Some(&run.id),
        Some(&t.id),
        format!("{} is waiting for you", t.title),
    );
    Ok(t)
}

fn exec_tool(engine: &Engine, run: &Run, node: &Node, tn: &ToolNode) -> Result<Step> {
    let store = engine.store().clone();
    let mut t = new_task(
        run,
        node,
        TaskKind::Tool,
        match tn {
            ToolNode::Command { .. } => "shell",
            ToolNode::PullRequest { .. } => "git.pr",
            ToolNode::MergePr { .. } => "git.merge",
        },
    );
    store.put_task(&t)?;
    store.log(
        "task.started",
        Some(&run.id),
        Some(&t.id),
        format!("{} started", node.name),
    );
    let res: Result<String> = (|| match tn {
        ToolNode::Command {
            workdir,
            command,
            allow_failure,
        } => {
            let p = match workdir {
                Some(w) => place(engine, run, node, w, None)?,
                None => Place {
                    cwd: std::env::temp_dir(),
                    branch: None,
                    base: None,
                    shell: String::new(),
                    env: Default::default(),
                },
            };
            let cancel = engine.cancel_flag(&t.id);
            let out = tools::run_command(
                &p.cwd,
                &p.shell,
                &p.env,
                command,
                Duration::from_secs(3600),
                &cancel,
            )?;
            engine.drop_cancel_flag(&t.id);
            system_note(&store, &t.id, tools::truncate_middle(&out, 6000));
            if !out.starts_with("exit code 0") && !allow_failure {
                bail!("{}", out.lines().next().unwrap_or("command failed"));
            }
            Ok(out.lines().next().unwrap_or("").to_string())
        }
        ToolNode::PullRequest { workdir, draft } => {
            let w = workdir.as_deref().context("no workdir")?;
            let wd = run.graph.workdir(w).context("workdir not found")?.clone();
            let p = place(engine, run, node, w, None)?;
            let run_now = store.run(&run.id).context("run not found")?;
            let branch =
                run_now.heads.get(w).cloned().or(p.branch.clone()).context(
                    "this workdir has no run branch (is it a git repo with worktrees on?)",
                )?;
            let root = git::repo_root(&p.cwd)?;
            git::commit_all(&root, &format!("{}\n\nRun #{}", run.title, run.number))?;
            let pipeline_name = store
                .pipeline(&run.pipeline_id)
                .map(|p| p.name)
                .unwrap_or_default();
            let body = pr_body(&run_now, &pipeline_name);
            let title = run.title.to_string();
            let url = git::open_pr(&root, &branch, &wd.base_branch, &title, &body, *draft)?;
            store.update_run(&run.id, |r| r.pr_url = Some(url.clone()))?;
            store.put_artifact(&Artifact {
                id: new_id(),
                run_id: Some(run.id.clone()),
                task_id: Some(t.id.clone()),
                kind: ArtifactKind::PullRequest,
                title: title.clone(),
                value: url.clone(),
                created_at: now(),
            })?;
            store.log(
                "pr.opened",
                Some(&run.id),
                Some(&t.id),
                format!("Opened {url}"),
            );
            Ok(format!("Opened {url}"))
        }
        ToolNode::MergePr { method } => {
            let url = store
                .run(&run.id)
                .and_then(|r| r.pr_url)
                .context("no pull request in this run")?;
            let cwd = run
                .worktrees
                .values()
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            git::merge_pr(&cwd, &url, method)?;
            Ok(format!("Merged {url}"))
        }
    })();
    let ok = res
        .as_ref()
        .map(|s| s.clone())
        .map_err(|e| anyhow!("{e:#}"));
    close_task(&store, &t.id, &ok);
    let s = res?;
    t.summary = s.clone();
    set_output(
        &store,
        &run.id,
        &node.id,
        NodeOutput {
            summary: s,
            port: None,
            value: Value::Null,
            task_id: Some(t.id),
        },
    )?;
    Ok(Step::Next(None))
}

fn pr_body(run: &Run, pipeline: &str) -> String {
    let mut s = format!(
        "Opened by the Pipeliner pipeline “{pipeline}” · run #{}\n\n## Task\n\n{}\n\n## Run\n\n",
        run.number,
        run.input.trim()
    );
    for n in &run.graph.nodes {
        if let Some(o) = run.outputs.get(&n.id) {
            if o.summary.trim().is_empty() || matches!(n.kind, NodeKind::Source(_)) {
                continue;
            }
            s.push_str(&format!(
                "- **{}**: {}\n",
                n.name,
                o.summary.trim().replace('\n', " ")
            ));
        }
    }
    s.push_str("\n🤖 Generated with Pipeliner\n");
    s
}
