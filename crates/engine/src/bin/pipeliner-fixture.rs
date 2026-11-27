//! Writes a throwaway data + config dir full of awkward but realistic data, for looking at
//! the UI (`scripts/shoot.sh`) — never for the user's own data dir.
//!
//! ```sh
//! cargo run -p harness-engine --bin pipeliner-fixture -- <dir> [--repo <path>]
//! ```
//!
//! `<dir>/data` and `<dir>/config` are created (and must not exist yet, so a real data dir
//! can't be overwritten by mistake). `<dir>/routes.json` maps names to ids for screenshots:
//! `{"review_pick": "<task id>", "review_pr": ..., "task_running": ..., "chat": ..., ...}`.
//!
//! What's in it, on purpose: waiting design pick with images, a PR approval with a
//! 10k-line diff (a 6000-line file, a minified one-line file, 60 small files), a running
//! step with nested sub-agents, a failed run with a long multi-line error, a plan
//! approval, a long chat (code block, unbroken URL, tool calls), 40 finished runs over 30
//! days, long titles, non-Latin text. Starter templates supply the pipelines and agents.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use harness_core::*;
use harness_engine::{templates, Engine};
use serde_json::json;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut repo: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--repo" => repo = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("usage: pipeliner-fixture <dir> [--repo <path to this repo>]");
                return Ok(());
            }
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let dir = dir.context("usage: pipeliner-fixture <dir> [--repo <path>]")?;
    let data = dir.join("data");
    let conf = dir.join("config");
    if data.exists() || conf.exists() {
        bail!(
            "{} already has data/ or config/; pick an empty dir",
            dir.display()
        );
    }
    std::fs::create_dir_all(&data)?;
    std::fs::create_dir_all(&conf)?;
    // Absolute, so the skill folders below don't depend on the working directory.
    let (data, conf) = (data.canonicalize()?, conf.canonicalize()?);
    // Everything below (Config::save, paths) resolves against these.
    std::env::set_var("PIPELINER_DATA_DIR", &data);
    std::env::set_var("PIPELINER_CONFIG_DIR", &conf);
    std::env::set_var("PIPELINER_CACHE_DIR", dir.join("cache"));
    let repo = repo
        .or_else(|| std::env::current_dir().ok())
        .context("no repo path")?;

    std::fs::write(
        conf.join("config.toml"),
        format!(
            r#"default_model = "local/claude-sonnet-test"
fast_model = "local/claude-haiku-test"
daily_budget_usd = 40.0
monthly_budget_usd = 600.0
self_repo = "{}"
skill_dirs = ["{}", "{}"]

[[providers]]
id = "local"
kind = "openai"
base_url = "http://127.0.0.1:9/v1"
"#,
            repo.display(),
            conf.join("skills").display(),
            data.parent().unwrap_or(&data).join("home-skills").display()
        ),
    )?;
    let config = Config::load();
    let store = Store::open(&data.join("pipeliner.db"))?;
    let engine = Engine::new(store.clone(), config.clone());

    let tpls = templates::all(&config);
    let design_t = tpls
        .iter()
        .find(|t| t.id == "design-gpui")
        .or(tpls.first())
        .context("no templates")?;
    let design = engine.create_pipeline(design_t, "")?;
    for t in tpls.iter().filter(|t| t.id != design_t.id) {
        engine.create_pipeline(t, "")?;
    }
    if let Some(t) = tpls.iter().find(|t| t.id == "blank") {
        engine.create_pipeline(
            t,
            "Nightly dependency bumps, changelog digest and flaky-test quarantine for every service repo",
        )?;
    }
    long_agent(&store)?;

    let mut f = Fixture {
        store: store.clone(),
        pipeline: design,
        routes: BTreeMap::new(),
        repo,
    };
    f.history()?;
    f.failed_run()?;
    f.running_run()?;
    f.pick_run()?;
    f.pr_run()?;
    f.chat()?;
    skills_fixture(&store, &conf, &data, &f.routes)?;

    std::fs::write(
        dir.join("routes.json"),
        serde_json::to_string_pretty(&f.routes)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&f.routes)?);
    Ok(())
}

struct Fixture {
    store: Arc<Store>,
    pipeline: Pipeline,
    routes: BTreeMap<String, String>,
    repo: PathBuf,
}

fn ago(mins: i64) -> DateTime<Utc> {
    now() - Duration::minutes(mins)
}

const LONG_TITLE: &str =
    "Let people create tasks right on the Board and the Timeline, fill in the description, \
attach screenshots and pick which pipeline runs them — без перехода в чат";

impl Fixture {
    fn run(&mut self, title: &str, status: Status, started_mins_ago: i64) -> Result<Run> {
        let n = self.store.next_number("run");
        let r = Run {
            id: new_id(),
            number: n,
            pipeline_id: self.pipeline.id.clone(),
            pipeline_version: self.pipeline.version,
            graph: self.pipeline.graph.clone(),
            title: title.into(),
            input: format!(
                "{title}\n\nAcceptance: works with 200 tasks; keyboard only; no layout jumps."
            ),
            external_ref: Some(format!("GH-{}", 1200 + n)),
            status,
            chat_id: None,
            outputs: BTreeMap::new(),
            frontier: vec![],
            heads: BTreeMap::new(),
            worktrees: BTreeMap::new(),
            notes: BTreeMap::new(),
            pr_url: None,
            cost_usd: 0.0,
            error: None,
            created_at: ago(started_mins_ago),
            finished_at: None,
        };
        self.store.put_run(&r)?;
        Ok(r)
    }

    /// A finished pipeline step with an output on the run.
    fn step(
        &self,
        run: &mut Run,
        node: &str,
        mins_ago: i64,
        took: i64,
        summary: &str,
        cost: f64,
    ) -> Result<Task> {
        let name = run
            .graph
            .node(node)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| node.into());
        let mut t = Task::new(kind_of(run, node), name);
        t.run_id = Some(run.id.clone());
        t.node_id = Some(node.into());
        t.status = Status::Done;
        t.actor = actor_of(run, node);
        t.model = "local/claude-sonnet-test".into();
        t.summary = summary.into();
        t.cost_usd = cost;
        t.tokens_in = (cost * 90_000.0) as u64;
        t.tokens_out = (cost * 9_000.0) as u64;
        t.steps = (cost * 4.0) as u32 + 1;
        t.created_at = ago(mins_ago);
        t.started_at = Some(ago(mins_ago));
        t.finished_at = Some(ago(mins_ago - took));
        self.store.put_task(&t)?;
        run.outputs.insert(
            node.into(),
            NodeOutput {
                summary: summary.into(),
                port: None,
                value: json!(null),
                task_id: Some(t.id.clone()),
            },
        );
        run.cost_usd += cost;
        self.thread(&t, summary)?;
        Ok(t)
    }

    fn thread(&self, t: &Task, summary: &str) -> Result<()> {
        let mut m = Message::new(t.id.clone(), Role::System, format!("{} started", t.title));
        m.created_at = t.created_at;
        self.store.put_message(&m)?;
        let mut a = Message::new(
            t.id.clone(),
            Role::Assistant,
            "Reading the screens and the theme first.",
        );
        a.author = t.actor.clone();
        a.tool_calls = vec![ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"crates/app/src/screens/board.rs"}"#.into(),
        }];
        a.created_at = t.created_at + Duration::seconds(20);
        self.store.put_message(&a)?;
        let mut tr = Message::new(
            t.id.clone(),
            Role::Tool,
            "//! Board: columns by status…\n".repeat(30),
        );
        tr.tool_call_id = Some("c1".into());
        tr.created_at = t.created_at + Duration::seconds(21);
        self.store.put_message(&tr)?;
        // Several files in one read, then a two-part edit (shown as a diff when expanded).
        let mut b = Message::new(t.id.clone(), Role::Assistant, "");
        b.author = t.actor.clone();
        b.tool_calls = vec![
            ToolCall {
                id: "c2".into(),
                name: "read_file".into(),
                arguments: r#"{"files":[{"path":"crates/app/src/theme.rs","offset":160,"limit":20},{"path":"crates/app/src/ui/badge.rs"}]}"#.into(),
            },
            ToolCall {
                id: "c3".into(),
                name: "edit_file".into(),
                arguments: r#"{"path":"crates/app/src/screens/board.rs","edits":[{"old":"        .gap(px(8.))","new":"        .gap(theme::S2)"},{"old":"    .w(px(260.))\n    .child(search)","new":"    .w(px(260.))\n    .flex_none()\n    .child(search)"}]}"#.into(),
            },
        ];
        b.created_at = t.created_at + Duration::seconds(40);
        self.store.put_message(&b)?;
        for (id, text, secs) in [
            ("c2", "==> crates/app/src/theme.rs (lines 160–179) <==\n  160\tpub const S2: Pixels = px(8.);\n", 41),
            ("c3", "Edited crates/app/src/screens/board.rs: lines 214, 388", 42),
        ] {
            let mut r = Message::new(t.id.clone(), Role::Tool, text);
            r.tool_call_id = Some(id.into());
            r.created_at = t.created_at + Duration::seconds(secs);
            self.store.put_message(&r)?;
        }
        if !summary.is_empty() {
            let mut d = Message::new(t.id.clone(), Role::Assistant, summary);
            d.author = t.actor.clone();
            d.created_at = t.created_at + Duration::seconds(90);
            self.store.put_message(&d)?;
        }
        Ok(())
    }

    fn gate(&self, run: &Run, node: &str, title: &str, mins_ago: i64) -> Result<Task> {
        let mut t = Task::new(TaskKind::Gate, title);
        t.run_id = Some(run.id.clone());
        t.node_id = Some(node.into());
        t.status = Status::Waiting;
        t.actor = "you".into();
        t.created_at = ago(mins_ago);
        t.started_at = Some(ago(mins_ago));
        self.store.put_task(&t)?;
        self.store.log(
            "gate.waiting",
            Some(&run.id),
            Some(&t.id),
            format!("{title} is waiting for you"),
        );
        Ok(t)
    }

    /// 40 finished runs over the last 30 days, for Home's charts and the Board's Done column.
    fn history(&mut self) -> Result<()> {
        let titles = [
            "Timeline: zoom with ctrl+wheel",
            "Fix: review queue loses selection after a decision",
            "Agents screen: show which pipelines use an agent",
            "Settings: test the provider connection",
            "Chat: paste screenshots from the clipboard",
            "Board: drag cards between columns (needs design)",
            "Pipelines: duplicate a pipeline",
            "Home: spend by model chart",
        ];
        for i in 0..40i64 {
            let mins = 60 * 24 * (30 - i * 3 / 4) + 37 * i;
            let status = match i % 9 {
                4 => Status::Failed,
                7 => Status::Cancelled,
                _ => Status::Done,
            };
            let mut r = self.run(titles[i as usize % titles.len()], status, mins)?;
            self.step(
                &mut r,
                "triage",
                mins,
                1,
                "No — a small change to existing UI",
                0.04,
            )?;
            let mut b = self.step(
                &mut r,
                "build",
                mins - 2,
                25,
                "Done; tests and clippy pass.",
                1.2 + (i % 7) as f64 * 1.9,
            )?;
            if status == Status::Failed {
                b.status = Status::Failed;
                b.error = Some("cargo test failed: 2 tests".into());
                self.store.put_task(&b)?;
                r.error = b.error.clone();
            }
            r.pr_url = (status == Status::Done)
                .then(|| format!("https://github.com/example/app/pull/{}", 100 + i));
            r.finished_at = Some(ago(mins - 30));
            self.store.put_run(&r)?;
            self.store.log(
                "run.finished",
                Some(&r.id),
                None,
                format!("Run #{} finished", r.number),
            );
        }
        Ok(())
    }

    fn failed_run(&mut self) -> Result<()> {
        let mut r = self.run(
            "Timeline: virtualize rows so 2 000 tasks scroll smoothly",
            Status::Failed,
            190,
        )?;
        self.step(&mut r, "triage", 190, 1, "No — no visual change", 0.03)?;
        let mut b = self.step(&mut r, "build", 188, 40, "", 7.41)?;
        let err = format!(
            "cargo build failed (exit 101):\nerror[E0599]: no method named `uniform_list_with_ids` found for struct `TimelineScreen` in the current scope\n  --> crates/app/src/screens/timeline.rs:412:18\n{}\nerror: could not compile `pipeliner` (bin \"pipeliner\") due to 3 previous errors; a very long path follows: /home/user/.local/share/pipeliner/worktrees/run-{}/app/crates/app/src/screens/timeline/rows/virtualized_rows_with_sticky_headers_and_lane_grouping.rs",
            "   |\n412 |         self.uniform_list_with_ids(cx)\n   |              ^^^^^^^^^^^^^^^^^^^^^ method not found\n".repeat(3),
            r.number
        );
        b.status = Status::Failed;
        b.error = Some(err.clone());
        self.store.put_task(&b)?;
        r.error = Some(err);
        r.finished_at = Some(ago(148));
        self.store.put_run(&r)?;
        self.routes.insert("task_failed".into(), b.id);
        Ok(())
    }

    fn running_run(&mut self) -> Result<()> {
        let mut r = self.run(LONG_TITLE, Status::Running, 75)?;
        self.step(
            &mut r,
            "triage",
            75,
            1,
            "Yes — new entry points on two screens",
            0.07,
        )?;
        self.step(
            &mut r,
            "design",
            74,
            12,
            "I redesigned the task creation area on both screens.",
            15.93,
        )?;
        self.step(&mut r, "pick", 60, 3, "You picked B", 0.0)?;
        let mut build = Task::new(TaskKind::Agent, "GPUI build");
        build.run_id = Some(r.id.clone());
        build.node_id = Some("build".into());
        build.status = Status::Running;
        build.actor = "GPUI lead".into();
        build.model = "local/claude-opus-test".into();
        build.cost_usd = 12.40;
        build.steps = 31;
        build.created_at = ago(56);
        build.started_at = Some(ago(56));
        self.store.put_task(&build)?;
        self.thread(&build, "")?;
        let subs = [
            ("NewTaskComposer component (shared by Board and Timeline, with attachments and pipeline picker)", Status::Done, 1),
            ("Board: New task entry points + composer", Status::Running, 1),
            ("Timeline: New task button, per-run \"Add task\" rows, side composer", Status::Queued, 1),
            ("Fix clippy::too_many_arguments in composer::submit", Status::Failed, 2),
            ("Write unit tests for the task draft validation", Status::Running, 2),
        ];
        let mut first_parent = build.id.clone();
        for (i, (title, status, depth)) in subs.into_iter().enumerate() {
            let mut s = Task::new(TaskKind::Spawned, title);
            s.run_id = Some(r.id.clone());
            s.node_id = Some("build".into());
            s.parent_id = Some(if depth == 1 {
                build.id.clone()
            } else {
                first_parent.clone()
            });
            s.depth = depth;
            s.status = status;
            if i == 0 {
                // A finished sub-agent with a worktree: the screen offers a follow-up composer.
                s.cwd = Some(self.repo.display().to_string());
                s.branch = Some("harness/run-1-board-new-task".into());
            }
            s.actor = "Coder".into();
            s.model = "local/claude-sonnet-test".into();
            s.cost_usd = 0.8 * (i + 1) as f64;
            s.created_at = ago(50 - 8 * i as i64);
            s.started_at = (status != Status::Queued).then(|| ago(50 - 8 * i as i64));
            if status == Status::Done || status == Status::Failed {
                s.finished_at = Some(ago(44 - 8 * i as i64));
            }
            if status == Status::Failed {
                s.error =
                    Some("edit_file: old text not found in crates/app/src/ui/composer.rs".into());
            }
            self.store.put_task(&s)?;
            if i == 0 {
                self.routes.insert("task_done".into(), s.id.clone());
            }
            self.thread(
                &s,
                if status == Status::Done {
                    "Added `NewTaskComposer` in crates/app/src/ui/composer.rs."
                } else {
                    ""
                },
            )?;
            if i == 0 {
                first_parent = s.id.clone();
            }
            self.store.log(
                "task.spawned",
                Some(&r.id),
                Some(&s.id),
                format!("GPUI build spawned {title}"),
            );
        }
        r.frontier = vec!["build".into()];
        self.store.put_run(&r)?;
        self.routes.insert("task_running".into(), build.id);
        Ok(())
    }

    fn pick_run(&mut self) -> Result<()> {
        let mut r = self.run(
            "Board: group cards by pipeline, with a collapsible header per group",
            Status::Waiting,
            40,
        )?;
        self.step(
            &mut r,
            "triage",
            40,
            1,
            "Yes — the Board layout changes",
            0.05,
        )?;
        let design = self.step(&mut r, "design", 39, 14, "3 variants ready", 21.30)?;
        self.step(&mut r, "render", 25, 1, "Rendered 6 screens", 0.0)?;
        let gate = self.gate(
            &r,
            "pick",
            "Pick a design · Board: group cards by pipeline",
            24,
        )?;
        let renders = self.repo.join("design/renders");
        let imgs = |names: &[&str]| -> Vec<String> {
            names
                .iter()
                .map(|n| renders.join(n))
                .filter(|p| p.exists())
                .map(|p| p.display().to_string())
                .collect()
        };
        let variants = [
            ("Compact groups", "Groups are thin header rows with a count; cards keep their size.\n\nI kept the columns and only added a header row per pipeline.", imgs(&["board.png", "timeline.png"])),
            ("Swimlanes", "Each pipeline is a horizontal lane across all columns — easiest to scan when there are many runs, at the cost of vertical space. A very long sentence without breaks follows: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", imgs(&["board.png", "home.png"])),
            ("Tabs per pipeline", "", imgs(&["review.png"])),
        ];
        for (i, (title, summary, images)) in variants.into_iter().enumerate() {
            let v = Variant {
                id: new_id(),
                gate_task_id: gate.id.clone(),
                run_id: Some(r.id.clone()),
                source_task_id: Some(design.id.clone()),
                index: i as u32,
                title: title.into(),
                summary: summary.into(),
                model: "local/claude-opus-test".into(),
                cost_usd: 7.1,
                duration_secs: 600.0 + 120.0 * i as f64,
                branch: Some(format!(
                    "pipeliner/run-{}-design-{}",
                    r.number,
                    variant_letter(i as u32).to_ascii_lowercase()
                )),
                cwd: None,
                diff: html_diff(i),
                images,
                picked: false,
            };
            self.store.put_variant(&v)?;
        }
        r.frontier = vec!["pick".into()];
        self.store.put_run(&r)?;
        self.routes.insert("review_pick".into(), gate.id);
        Ok(())
    }

    fn pr_run(&mut self) -> Result<()> {
        let mut r = self.run(
            "Rewrite the mockups one tag per line and clip very long diff lines",
            Status::Waiting,
            130,
        )?;
        self.step(&mut r, "triage", 130, 1, "No — no visual change", 0.02)?;
        self.step(&mut r, "build", 129, 50, "I reformatted 12 mockups (render is pixel-identical) and clip diff lines over 400 characters. Tests, clippy and a release build pass.", 44.63)?;
        let pr = format!("https://github.com/example/app/pull/{}", 300 + r.number);
        self.step(&mut r, "pr", 79, 1, &format!("Opened {pr}"), 0.0)?;
        r.pr_url = Some(pr.clone());
        // An earlier approve whose merge failed: Review shows its error above the PR.
        let mut failed = self.gate(&r, "approve", &format!("Approve PR · {}", r.title), 79)?;
        failed.status = Status::Failed;
        failed.finished_at = Some(ago(78));
        failed.error = Some(format!(
            "Merge failed: gh pr merge {pr} --squash --delete-branch=false failed: X Pull request is not mergeable: the merge commit cannot be cleanly created.\nTo have the pull request merged after all the requirements have been met, add the `--auto` flag.\nRun the following to resolve the merge conflicts locally:\n  gh pr checkout {n} && git fetch origin main && git merge origin/main",
            n = 300 + r.number
        ));
        self.store.put_task(&failed)?;
        let gate = self.gate(&r, "approve", &format!("Approve PR · {}", r.title), 78)?;
        let v = Variant {
            id: new_id(),
            gate_task_id: gate.id.clone(),
            run_id: Some(r.id.clone()),
            source_task_id: None,
            index: 0,
            title: pr,
            summary: "**GPUI build** — reformatted mockups, clipped long lines.".into(),
            model: String::new(),
            cost_usd: r.cost_usd,
            duration_secs: 3000.0,
            branch: Some(format!("pipeliner/run-{}", r.number)),
            cwd: None,
            diff: huge_diff(),
            images: vec![],
            picked: false,
        };
        self.store.put_variant(&v)?;
        r.frontier = vec!["approve".into()];
        self.store.put_run(&r)?;
        self.routes.insert("review_pr".into(), gate.id);
        Ok(())
    }

    fn chat(&mut self) -> Result<()> {
        let mut task = Task::new(TaskKind::Chat, "Plan: split the settings screen into tabs");
        task.status = Status::Waiting;
        task.actor = "Orchestrator".into();
        task.created_at = ago(15);
        let chat = Chat {
            id: new_id(),
            title:
                "Settings feels crowded — can we split it into tabs and keep keyboard navigation?"
                    .into(),
            mode: ChatMode::Orchestrator,
            model: "local/claude-opus-test".into(),
            agent_id: None,
            pipeline_id: None,
            cwd: Some(self.repo.display().to_string()),
            task_id: Some(task.id.clone()),
            archived: false,
            created_at: ago(70),
            updated_at: ago(15),
        };
        task.chat_id = Some(chat.id.clone());
        self.store.put_chat(&chat)?;
        self.store.put_task(&task)?;
        for i in 0..30i64 {
            let (role, text) = match i % 5 {
                0 => (Role::User, format!("Point {i}: the provider list gets long; also keys should never show in full.")),
                1 => (Role::Assistant, "Looking at `settings.rs` first.\n\n```rust\n".to_string() + &"    .child(section(\"Providers\", providers(cx)))\n".repeat(12) + "```"),
                2 => (Role::Tool, "ok".into()),
                3 => (Role::Assistant, "See https://github.com/example/app/blob/main/crates/app/src/screens/settings/providers_and_models_with_a_really_long_path_name.rs#L1-L400 for the current layout.".into()),
                _ => (Role::Assistant, "Короче: три вкладки — Models, Skills, App. Hotkeys stay ctrl+1..3.".into()),
            };
            let mut m = Message::new(chat.id.clone(), role, text);
            m.created_at = ago(70 - i * 2);
            if role == Role::Assistant {
                m.author = "Orchestrator".into();
            }
            self.store.put_message(&m)?;
        }
        let mut plan = Message::new(chat.id.clone(), Role::Assistant, "Here is the plan.");
        plan.author = "Orchestrator".into();
        plan.card = Some(Card::Plan {
            task_id: task.id.clone(),
            steps: vec![
                "Split Settings into Models, Skills and App tabs".into(),
                "Move provider keys behind a reveal button and never log them".into(),
                "Keep ctrl+1..3 to switch tabs; add a test for the key handling".into(),
                "Update design/screens/settings.html and its render".into(),
            ],
        });
        plan.created_at = ago(15);
        self.store.put_message(&plan)?;
        self.routes.insert("chat".into(), chat.id);
        self.routes.insert("review_plan".into(), task.id);
        let p = self.pipeline.id.clone();
        self.routes.insert("pipeline".into(), p);
        if let Some(a) = self.store.agents().into_iter().next() {
            self.routes.insert("agent".into(), a.id);
        }
        Ok(())
    }
}

/// Installed skills in the fixture's own folders (the user's real ones are never read),
/// agents using some of them, and two staged sets waiting for review.
fn skills_fixture(
    store: &Arc<Store>,
    conf: &std::path::Path,
    data: &std::path::Path,
    routes: &BTreeMap<String, String>,
) -> Result<()> {
    use harness_core::skills::{self, Provenance, SourceKind};
    let mine = conf.join("skills");
    let home = data.parent().unwrap_or(data).join("home-skills");
    let write =
        |dir: &std::path::Path, name: &str, desc: &str, prov: Option<Provenance>| -> Result<()> {
            let d = dir.join(name);
            std::fs::create_dir_all(&d)?;
            std::fs::write(
                d.join("SKILL.md"),
                format!(
                "---\nname: {name}\ndescription: {desc}\n---\n\nFollow the {name} conventions.\n"
            ),
            )?;
            if let Some(p) = prov {
                std::fs::write(
                    d.join(skills::PROVENANCE_FILE),
                    serde_json::to_string_pretty(&p)?,
                )?;
            }
            Ok(())
        };
    let prov = |source: &str,
                kind,
                label: &str,
                commit: Option<&str>,
                mins: i64,
                chat: Option<&str>,
                actor: Option<&str>| Provenance {
        source: source.into(),
        kind,
        label: label.into(),
        commit: commit.map(String::from),
        installed_at: ago(mins),
        chat_title: chat.map(String::from),
        actor: actor.map(String::from),
    };
    write(
        &mine,
        "code-review",
        "Review a diff for bugs, risky changes and missing tests before it goes to a human.",
        None,
    )?;
    write(
        &mine,
        "brand-voice",
        "Write in the house voice: plain words, short sentences, no exclamation marks, no hype, and always spell out what the reader should do next. Use it for release notes, emails to customers, docs intros and anything else a customer reads, and never for internal commit messages.",
        None,
    )?;
    write(
        &mine,
        "git-conventions",
        "Branch names, commit messages and PR descriptions the way this team writes them.",
        Some(prov(
            "https://github.com/acme/team-skills/tree/main/git-conventions",
            SourceKind::Link,
            "acme/team-skills/git-conventions",
            Some("b41d7e0"),
            60 * 24 * 12,
            None,
            None,
        )),
    )?;
    write(
        &home,
        "sql-style",
        "Форматирование SQL: ключевые слова в верхнем регистре, один JOIN на строку, CTE вместо вложенных подзапросов.",
        Some(prov(
            "npx skills add dbt-labs/sql-style",
            SourceKind::Command,
            "npx skills add dbt-labs/sql-style",
            None,
            60 * 24 * 5,
            None,
            None,
        )),
    )?;
    write(
        &home,
        "web-research",
        "Search the web, read the sources and report what is known, with links.",
        Some(prov(
            "https://github.com/acme/agent-skills/tree/main/research",
            SourceKind::Link,
            "acme/agent-skills/research",
            Some("3f9c2a1"),
            60 * 30,
            Some(
                "Settings feels crowded — can we split it into tabs and keep keyboard navigation?",
            ),
            Some("Orchestrator"),
        )),
    )?;
    write(
        &home,
        "changelog",
        "Keep CHANGELOG.md in the Keep a Changelog format.",
        None,
    )?;

    // Agents use some of them.
    let agents = store.agents();
    for (i, a) in agents.iter().take(4).enumerate() {
        let mut a = a.clone();
        match i {
            0 => {
                a.skills = vec!["code-review".into(), "git-conventions".into()];
                a.skills_on_demand = vec!["web-research".into()];
            }
            1 => {
                a.skills_on_demand = vec![
                    "code-review".into(),
                    "sql-style".into(),
                    "brand-voice".into(),
                ]
            }
            2 => a.skills = vec!["brand-voice".into()],
            _ => a.skills_on_demand = vec!["changelog".into()],
        }
        store.put_agent(&a)?;
    }

    // Staged: a link, with one name that clashes with an installed skill.
    let config = Config::load();
    let stage = |source: &str,
                 kind: SourceKind,
                 label: &str,
                 commit: Option<&str>,
                 files: &[(&str, &str, &str)],
                 extra: &[(&str, &str)],
                 mins: i64|
     -> Result<skills::StagedSet> {
        let (id, src) = skills::begin_staging()?;
        for (name, desc, _) in files {
            let d = src.join(name);
            std::fs::create_dir_all(&d)?;
            std::fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n\nSteps for {name}.\n"),
            )?;
        }
        for (path, text) in extra {
            let p = src.join(path);
            std::fs::create_dir_all(p.parent().unwrap_or(&src))?;
            std::fs::write(p, text)?;
        }
        let mut set = skills::build_staged(
            &config,
            &id,
            source,
            kind,
            label,
            commit.map(String::from),
            &src,
        );
        set.created_at = ago(mins);
        Ok(set)
    };
    let set = stage(
        "https://github.com/acme/agent-skills/tree/main/documents",
        SourceKind::Link,
        "acme/agent-skills/documents",
        Some("3f9c2a1"),
        &[
            (
                "pdf-tools",
                "Read, split and fill PDF forms; extract tables to CSV.",
                "",
            ),
            (
                "spreadsheets",
                "Edit .xlsx and .csv files, keep formulas, write charts.",
                "",
            ),
            (
                "brand-voice",
                "Acme's voice for documents: formal, second person, no jargon.",
                "",
            ),
        ],
        &[
            (
                "spreadsheets/scripts/fill.py",
                "import sys\nprint(sys.argv)\n",
            ),
            ("spreadsheets/scripts/chart.sh", "#!/bin/sh\necho chart\n"),
            ("pdf-tools/forms.md", "# Forms\n"),
        ],
        8,
    )?;
    skills::save_staged(&set)?;

    // Staged by the Orchestrator in the fixture's chat.
    let chat_title = routes
        .get("chat")
        .and_then(|id| store.chat(id))
        .map(|c| c.title);
    let mut set = stage(
        "npx skills add keep-a-changelog/skill",
        SourceKind::Command,
        "npx skills add keep-a-changelog/skill",
        None,
        &[(
            "release-notes",
            "Turn merged PRs into release notes grouped by area.",
            "",
        )],
        &[],
        70,
    )?;
    set.actor = Some("Orchestrator".into());
    set.chat_id = routes.get("chat").cloned();
    set.chat_title = chat_title;
    skills::save_staged(&set)?;
    Ok(())
}

fn kind_of(run: &Run, node: &str) -> TaskKind {
    match run.graph.node(node).map(|n| &n.kind) {
        Some(NodeKind::Gate(_)) => TaskKind::Gate,
        Some(NodeKind::Decision(_)) => TaskKind::Decision,
        Some(NodeKind::Render(_)) => TaskKind::Render,
        Some(NodeKind::Tool(_)) => TaskKind::Tool,
        Some(NodeKind::Source(_)) => TaskKind::Source,
        Some(NodeKind::Router(_)) => TaskKind::Router,
        _ => TaskKind::Agent,
    }
}

fn actor_of(run: &Run, node: &str) -> String {
    match run.graph.node(node).map(|n| &n.kind) {
        Some(NodeKind::Gate(_)) => "you".into(),
        Some(NodeKind::Tool(_)) | Some(NodeKind::Render(_)) => "tool".into(),
        _ => run
            .graph
            .node(node)
            .map(|n| n.name.clone())
            .unwrap_or_default(),
    }
}

fn long_agent(store: &Arc<Store>) -> Result<()> {
    store.put_agent(&AgentDef {
        id: new_id(),
        name: "Release notes writer for the desktop app and the CLI".into(),
        description: "Reads merged PRs since the last tag, groups them by area and writes release notes in plain \
language for people who don't read diffs; flags anything that changes data on disk."
            .into(),
        role: "You write release notes.".into(),
        model: "local/claude-haiku-test".into(),
        tools: ToolAccess::default(),
        skills: vec![],
        skills_on_demand: vec![],
        max_steps: 20,
        version: 3,
        created_at: ago(60 * 24 * 9),
        updated_at: ago(60 * 24 * 2),
    })
}

fn html_diff(i: usize) -> String {
    let mut s = String::from(
        "diff --git a/design/screens/board.html b/design/screens/board.html\n--- a/design/screens/board.html\n+++ b/design/screens/board.html\n@@ -40,6 +40,24 @@\n",
    );
    for k in 0..(18 + 6 * i) {
        s.push_str(&format!("+      <div class=\"group-head\" data-pipeline=\"p{k}\">Design → GPUI build <span class=\"count\">{k}</span></div>\n"));
    }
    s.push_str(" <div class=\"col\">\n");
    s
}

/// ~10k lines: 60 small files, a 6000-line file and a minified one-line file.
fn huge_diff() -> String {
    let mut s = String::new();
    for f in 0..60 {
        s.push_str(&format!(
            "diff --git a/crates/app/src/screens/area_{f}.rs b/crates/app/src/screens/area_{f}.rs\n--- a/crates/app/src/screens/area_{f}.rs\n+++ b/crates/app/src/screens/area_{f}.rs\n@@ -10,7 +10,9 @@ impl Screen {{\n"
        ));
        for l in 0..(10 + f % 30) {
            match l % 4 {
                0 => s.push_str(&format!("-        .gap(px({l}.))\n")),
                1 => s.push_str(&format!("+        .gap(theme::S{})\n", l % 6)),
                _ => s.push_str("         .child(row)\n"),
            }
        }
    }
    s.push_str("diff --git a/design/screens/review.html b/design/screens/review.html\n--- a/design/screens/review.html\n+++ b/design/screens/review.html\n@@ -1,6000 +1,6000 @@\n");
    for l in 0..3000 {
        s.push_str(&format!(
            "-<div class=\"row r{l}\"><span>old line {l}</span></div>\n"
        ));
        s.push_str(&format!(
            "+<div class=\"row r{l}\"\n><span>new line {l}</span></div>\n"
        ));
    }
    s.push_str("diff --git a/design/tokens.min.css b/design/tokens.min.css\nnew file mode 100644\n--- /dev/null\n+++ b/design/tokens.min.css\n@@ -0,0 +1 @@\n+");
    for k in 0..4000 {
        s.push_str(&format!(
            ":root{{--c{k}:#{:06x}}}",
            (k as u64).wrapping_mul(2654435) % 0xffffff
        ));
    }
    s.push('\n');
    s
}
