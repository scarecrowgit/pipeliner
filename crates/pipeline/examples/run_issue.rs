//! Run the pipeline headless (no UI), printing the run log.
//!
//! ```text
//! PIPELINE=<id> cargo run -p pipeline --example run_issue -- new "Title" "Description"  # run to approval
//! cargo run -p pipeline --example run_issue -- approve HAR-1               # approve, build, open the PR
//! cargo run -p pipeline --example run_issue -- changes HAR-1 "Make it smaller"
//! cargo run -p pipeline --example run_issue -- retry HAR-1
//! ```
//! Uses the real config (`~/.config/harness/pipeline.toml`) and data dir, so the issue
//! and its run show up on the board.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use pipeline::model::{self, Run};
use pipeline::{Job, def, stages};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (pipelines, errors) = def::load_all();
    for e in errors {
        eprintln!("pipeline file: {e}");
    }
    let pick = |id: &str| {
        pipelines
            .iter()
            .find(|p| p.id == id)
            .or_else(|| pipelines.iter().find(|p| p.id == "mockup-gpui-build"))
            .or(pipelines.first())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no pipelines"))
    };
    let (issue, run, job) = match args.first().map(String::as_str) {
        Some("new") => {
            let title = args.get(1).cloned().unwrap_or_default();
            let body = args.get(2).cloned().unwrap_or_default();
            let def = pick(&std::env::var("PIPELINE").unwrap_or_default())?;
            let issue = model::create_issue(&title, &body, "Feature", &def.id)?;
            let run = Run::new(&issue.key, &def);
            run.save()?;
            model::update_issue(&issue.key, |i| i.run = Some(run.id.clone()))?;
            println!("Created {} — run {}", issue.key, run.id);
            (issue, run, Job::Start)
        }
        Some(cmd @ ("approve" | "changes" | "retry")) => {
            let key = args.get(1).cloned().unwrap_or_default();
            let issue = model::load_issues()
                .into_iter()
                .find(|i| i.key == key)
                .ok_or_else(|| anyhow::anyhow!("no issue {key}"))?;
            let run = issue
                .run
                .as_deref()
                .and_then(model::load_run)
                .ok_or_else(|| anyhow::anyhow!("{key} has no run"))?;
            let job = match cmd {
                "approve" => Job::Approved,
                "changes" => Job::Redesign(args.get(2).cloned().unwrap_or_default()),
                _ => Job::Retry,
            };
            (issue, run, job)
        }
        _ => {
            eprintln!("usage: run_issue new <title> <body> | approve <KEY> | changes <KEY> <notes> | retry <KEY>");
            std::process::exit(2);
        }
    };
    let shown = Arc::new(Mutex::new(run.log.len()));
    let run = Arc::new(Mutex::new(run));
    let printer = {
        let run = run.clone();
        let shown = shown.clone();
        move || {
            let Ok(r) = run.try_lock() else { return };
            let mut n = shown.lock().unwrap();
            for l in r.log.iter().skip(*n) {
                println!("[{}] {:?} {}", l.who, l.kind, l.text.replace('\n', "\n    "));
            }
            *n = r.log.len();
        }
    };
    let ctx = stages::Ctx {
        issue,
        run: run.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        notify: Box::new(printer),
    };
    stages::dispatch(&ctx, job);
    let r = run.lock().unwrap();
    println!("\nStatus: {:?}", r.status);
    for s in &r.stages {
        println!("  {:<16} {:?} {}", s.name, s.status, s.summary);
    }
    for rd in &r.renders {
        println!("  render {}: {}", rd.screen, rd.after.display());
    }
    if let Some(u) = &r.pr_url {
        println!("  PR: {u}");
    }
    if let Some(e) = &r.error {
        println!("  error: {e}");
    }
    println!("  tokens: {}", r.tokens);
    Ok(())
}
