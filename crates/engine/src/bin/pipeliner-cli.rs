//! Headless driver for the engine (same database as the app). Useful for scripting
//! and for agents testing changes to the engine.
//!
//!   pipeliner-cli pipelines
//!   pipeliner-cli templates
//!   pipeliner-cli create <template-id> [name]
//!   pipeliner-cli run <pipeline name> <input...>
//!   pipeliner-cli start <pipeline name> <input...>   (queue a run for the owning process)
//!   pipeliner-cli runs
//!   pipeliner-cli show <run number>
//!   pipeliner-cli waiting
//!   pipeliner-cli delete-run <run number>  (removes the run, its worktrees, branches, renders)
//!   pipeliner-cli decide <task id> pick <n> | approve | back <note...> | reject
//!   pipeliner-cli chat <text...>          (direct chat, prints the reply)
//!   pipeliner-cli watch <run number>      (drives the run in this process until it waits or ends)
//!   pipeliner-cli serve                   (executes all runs until none is running)

use anyhow::{bail, Context, Result};
use harness_core::*;
use harness_engine::Engine;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    let store = Store::open_default()?;
    let engine = Engine::new(store.clone(), Config::load());
    // Only commands that execute work take part in engine ownership.
    if matches!(cmd, "run" | "serve" | "watch" | "decide" | "chat") {
        engine.start();
        if !engine.is_owner() {
            eprintln!(
                "note: another Pipeliner process owns the engine; it will execute runs and decisions."
            );
        }
    }
    match cmd {
        "pipelines" => {
            for p in store.pipelines() {
                println!(
                    "{}  v{}{}  {}",
                    p.id,
                    p.version,
                    if p.draft.is_some() { " (draft)" } else { "" },
                    p.name
                );
            }
        }
        "templates" => {
            for t in engine.templates() {
                println!("{:12} {} — {}", t.id, t.name, t.description);
            }
        }
        "create" => {
            let id = rest.first().context("template id")?;
            let t = engine
                .templates()
                .into_iter()
                .find(|t| t.id == id)
                .context("unknown template")?;
            let p = engine.create_pipeline(&t, &rest[1..].join(" "))?;
            println!("created {} ({})", p.name, p.id);
        }
        "run" => {
            let name = rest.first().context("pipeline name")?;
            let p = store
                .pipelines()
                .into_iter()
                .find(|p| &p.name == name || &p.id == name)
                .context("no such pipeline")?;
            let input = rest[1..].join(" ");
            let r = engine.start_run(&p.id, &input, None)?;
            println!("run #{} {}", r.number, r.id);
            drive_until_idle(&engine, &r.id)?;
        }
        "start" => {
            // Queue a run and exit; the owning process (app or `serve`) executes it.
            let name = rest.first().context("pipeline name")?;
            let p = store
                .pipelines()
                .into_iter()
                .find(|p| &p.name == name || &p.id == name)
                .context("no such pipeline")?;
            let r = engine.start_run(&p.id, &rest[1..].join(" "), None)?;
            println!("run #{} {}", r.number, r.id);
        }
        "watch" => {
            let r = find_run(&store, rest.first().context("run number")?)?;
            drive_until_idle(&engine, &r.id)?;
        }
        "serve" => {
            // Headless owner: executes runs until none is running (waiting ones stay for the app).
            if !engine.is_owner() {
                bail!("another Pipeliner process owns the engine");
            }
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let busy = store
                    .runs()
                    .into_iter()
                    .any(|r| r.status == Status::Running);
                if !busy {
                    for r in store.runs() {
                        println!("#{} {} {}", r.number, r.status.as_str(), r.title);
                    }
                    break;
                }
            }
        }
        "runs" => {
            for r in store.runs() {
                println!(
                    "#{} {:9} ${:.2} {}  {}",
                    r.number,
                    r.status.as_str(),
                    r.cost_usd,
                    r.title,
                    r.pr_url.unwrap_or_default()
                );
            }
        }
        "show" => {
            let r = find_run(&store, rest.first().context("run number")?)?;
            print_run(&store, &r);
        }
        "delete-run" => {
            let r = find_run(&store, rest.first().context("run number")?)?;
            if r.status == Status::Running {
                bail!("run #{} is running; cancel it first", r.number);
            }
            delete_run_files(&r);
            store.delete_run(&r.id)?;
            println!("deleted run #{} {}", r.number, r.title);
        }
        "waiting" => {
            for t in store.waiting() {
                println!("{}  {}", t.id, t.title);
                for v in store.variants(&t.id) {
                    println!(
                        "   {} {} images={} diff={}B",
                        variant_letter(v.index),
                        v.title,
                        v.images.len(),
                        v.diff.len()
                    );
                }
            }
        }
        "decide" => {
            let id = rest.first().context("task id")?;
            let kind = match rest.get(1).map(String::as_str) {
                Some("pick") => DecisionKind::Pick {
                    index: rest.get(2).context("index")?.parse()?,
                },
                Some("approve") => DecisionKind::Approve,
                Some("back") => DecisionKind::SendBack {
                    note: rest[2..].join(" "),
                    target: None,
                },
                Some("reject") => DecisionKind::Reject {
                    reason: rest[2..].join(" "),
                },
                _ => bail!("pick <n> | approve | back <note> | reject"),
            };
            engine.decide(id, kind)?;
            let t = store.task(id).context("task")?;
            if let Some(run) = t.run_id {
                drive_until_idle(&engine, &run)?;
            }
        }
        "chat" => {
            let c = engine.new_chat(ChatMode::Direct)?;
            engine.send(&c.id, &rest.join(" "))?;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let t = store.task(c.task_id.as_deref().unwrap_or("")).or_else(|| {
                    store
                        .chat(&c.id)
                        .and_then(|c| c.task_id)
                        .and_then(|t| store.task(&t))
                });
                if let Some(t) = t {
                    if t.status.is_finished() {
                        break;
                    }
                }
            }
            for m in store.messages(&c.id) {
                println!("[{:?}] {}", m.role, m.content);
            }
        }
        _ => {
            println!("see the doc comment in crates/engine/src/bin/pipeliner-cli.rs");
        }
    }
    Ok(())
}

/// Removes the run's git worktrees, their local branches and its rendered images.
fn delete_run_files(r: &Run) {
    use std::process::Command;
    let git = |cwd: &std::path::Path, args: &[&str]| {
        Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let dir = harness_core::paths::worktrees_dir().join(format!("run-{}", r.number));
    for wt in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let wt = wt.path();
        let branch = git(&wt, &["branch", "--show-current"]).unwrap_or_default();
        let Some(common) = git(
            &wt,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ) else {
            continue;
        };
        let repo = std::path::Path::new(&common)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let wt_s = wt.to_string_lossy().to_string();
        if git(&repo, &["worktree", "remove", "--force", &wt_s]).is_none() {
            eprintln!("could not remove worktree {wt_s}");
        }
        if !branch.is_empty() && git(&repo, &["branch", "-D", &branch]).is_none() {
            eprintln!("could not delete branch {branch}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(
        harness_core::paths::renders_dir().join(format!("run-{}", r.number)),
    );
}

fn find_run(store: &Store, n: &str) -> Result<Run> {
    let n: u64 = n.trim_start_matches('#').parse()?;
    store
        .runs()
        .into_iter()
        .find(|r| r.number == n)
        .context("no such run")
}

fn drive_until_idle(engine: &Engine, run_id: &str) -> Result<()> {
    let store = engine.store();
    let mut last = String::new();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let r = store.run(run_id).context("run")?;
        let line = format!(
            "#{} {} frontier={:?} ${:.3}",
            r.number,
            r.status.as_str(),
            r.frontier,
            r.cost_usd
        );
        if line != last {
            println!("{line}");
            last = line;
        }
        if r.status != Status::Running {
            print_run(store, &r);
            return Ok(());
        }
    }
}

fn print_run(store: &Store, r: &Run) {
    println!(
        "Run #{} [{}] {}  pr={:?} err={:?}",
        r.number,
        r.status.as_str(),
        r.title,
        r.pr_url,
        r.error
    );
    for t in store.tasks_for_run(&r.id) {
        println!(
            "  {:10} {:9} {:20} ${:.3} steps={} {}",
            t.kind.label(),
            t.status.as_str(),
            t.title.chars().take(20).collect::<String>(),
            t.cost_usd,
            t.steps,
            t.summary
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(100)
                .collect::<String>()
        );
        if let Some(e) = &t.error {
            println!(
                "             error: {}",
                e.chars().take(300).collect::<String>()
            );
        }
    }
}
