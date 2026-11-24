//! Operations: autopilots on their clock, why a run waits and its Inbox items, the worktree
//! sweep, settings and projects.

use std::sync::Arc;
use std::time::{Duration, Instant};

use llm::Reply;
use model::{
    Autopilot, AutopilotId, AutopilotTrigger, Event, InboxKind, ModelRef, Outcome, PipelineId, ProjectId, Run, RunId,
    RunState, RunWait, Schedule,
};
use serde_json::json;

use super::pipelines::{deploy, pipeline_fx, wait_run};
use super::pipelines_more::{graph, input};
use super::{Fx, NOW, git, options};
use crate::api::{InboxApi, IssueApi, NewProject, OpsApi, RunApi};
use crate::{Engine, EngineOptions};

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "not {what} in time");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn project(fx: &Fx) -> ProjectId {
    fx.engine.create_project(NewProject { name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() }).expect("project").id
}

fn pilot(id: &str, project: &ProjectId, schedule: Schedule) -> Autopilot {
    Autopilot {
        id: AutopilotId::new(id),
        project: project.clone(),
        name: format!("Pilot {id}"),
        trigger: AutopilotTrigger::Schedule { schedule },
        target: catalog::Catalog::default_chat_agent(),
        prompt: format!("Tick of {id}"),
        enabled: true,
        paused_reason: None,
        next_run: None,
        last_run: None,
        last_result: String::new(),
        budget_usd: None,
    }
}

fn stored(fx: &Fx, id: &str) -> Autopilot {
    fx.engine.inner.store.autopilots(None).unwrap().into_iter().find(|a| a.id.as_str() == id).expect("autopilot")
}

fn background() -> EngineOptions {
    EngineOptions { background: true, ..options() }
}

#[test]
fn an_autopilot_fires_when_the_clock_reaches_it_under_its_budget() {
    let fx = Fx::with(background());
    let project = project(&fx);
    let mut every = pilot("ap", &project, Schedule::Minutes { minutes: 30 });
    every.budget_usd = Some(0.5);
    fx.engine.save_autopilot(every).unwrap();
    assert_eq!(stored(&fx, "ap").next_run, Some(NOW + 30 * MIN));
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(stored(&fx, "ap").last_run, None, "not due yet");

    fx.llm.push(Reply::text("Nothing to do."));
    fx.clock.advance(31 * MIN);
    fx.engine.inner.wakeup.poke();
    eventually("fired", || stored(&fx, "ap").last_run.is_some());
    let fired = stored(&fx, "ap");
    assert_eq!((fired.last_run, fired.last_result.as_str()), (Some(NOW + 31 * MIN), "started a chat"));
    assert_eq!(fired.next_run, Some(NOW + 61 * MIN));
    let chats = fx.engine.inner.store.sessions(&Default::default()).unwrap();
    assert_eq!(chats.len(), 1);
    let cap = fx.engine.inner.store.marker(&crate::chat::budget_marker(&chats[0].id)).unwrap();
    assert_eq!(cap.as_deref(), Some("0.5"), "the chat works under the autopilot's budget");
    fx.settle(&chats[0].id);
    fx.engine.shutdown();
}

#[test]
fn missed_ticks_fire_once_within_a_day_and_are_skipped_after_and_the_limit_blocks() {
    let fx = Fx::new();
    let project = project(&fx);
    let store = &fx.engine.inner.store;
    let mut recent = pilot("recent", &project, Schedule::Minutes { minutes: 60 });
    recent.next_run = Some(NOW - 2 * HOUR);
    let mut old = pilot("old", &project, Schedule::Day { hour: 3, minute: 0 });
    old.next_run = Some(NOW - 3 * DAY);
    let mut off = pilot("off", &project, Schedule::Minutes { minutes: 5 });
    off.enabled = false;
    off.next_run = Some(NOW - HOUR);
    for a in [&recent, &old, &off] {
        store.upsert_autopilot(a).unwrap();
    }
    fx.llm.push(Reply::text("Done."));
    let next = fx.engine.fire_due().unwrap();

    let recent = stored(&fx, "recent");
    assert_eq!((recent.last_run, recent.next_run), (Some(NOW), Some(NOW + HOUR)));
    let old = stored(&fx, "old");
    assert_eq!(old.last_run, None, "a tick missed by more than a day is skipped");
    assert!(old.next_run.is_some_and(|at| at > NOW && at <= NOW + DAY), "{old:?}");
    assert_eq!(stored(&fx, "off").last_run, None, "a disabled autopilot never fires");
    assert_eq!(next, Some(NOW + HOUR).min(old.next_run));
    let chats = store.sessions(&Default::default()).unwrap();
    assert_eq!(chats.len(), 1, "fired once");
    fx.settle(&chats[0].id);

    fx.engine.set_daily_limit(Some(0.0)).unwrap();
    let mut due = pilot("limited", &project, Schedule::Minutes { minutes: 5 });
    due.next_run = Some(NOW - MIN);
    store.upsert_autopilot(&due).unwrap();
    fx.engine.fire_due().unwrap();
    let limited = stored(&fx, "limited");
    assert!(limited.last_result.starts_with("skipped: ") && limited.last_result.contains("spend limit"), "{}", limited.last_result);
    assert_eq!(limited.next_run, Some(NOW + 5 * MIN));
    assert_eq!(store.sessions(&Default::default()).unwrap().len(), 1, "nothing started");
}

#[test]
fn a_tick_missed_while_closed_fires_on_start_and_the_thread_ends_with_the_engine() {
    let fx = Fx::new();
    let project = project(&fx);
    let mut missed = pilot("missed", &project, Schedule::Day { hour: 3, minute: 0 });
    missed.next_run = Some(NOW - 5 * HOUR);
    fx.engine.inner.store.upsert_autopilot(&missed).unwrap();
    fx.llm.push(Reply::text("Caught up."));

    let engine = Engine::open_with(fx.paths.clone(), fx.llm.clone(), fx.clock.clone(), background()).unwrap();
    eventually("fired on start", || stored(&fx, "missed").last_run == Some(NOW));
    let session = engine.inner.store.sessions(&Default::default()).unwrap().remove(0).id;
    eventually("the turn ended", || !engine.inner.live.is_running(&session));
    let weak = Arc::downgrade(&engine.inner);
    drop(engine);
    eventually("the scheduler let go of the engine", || weak.upgrade().is_none());
}

fn priced() -> ModelRef {
    ModelRef::parse("scripted/claude-fable-5")
}

fn spending(reply: Reply) -> Reply {
    reply.with_usage(model::Usage { input: 1_000, output: 200, ..Default::default() })
}

fn waits(fx: &Fx, run: &RunId) -> Vec<Option<RunWait>> {
    fx.drain();
    fx.log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::RunWaiting { run: id, waiting } if id == run => Some(*waiting),
            _ => None,
        })
        .collect()
}

#[test]
fn a_run_stopped_at_its_budget_files_an_inbox_item_resolved_by_a_raise() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.nodes.truncate(2);
    g.edges.truncate(1);
    g.limits.budget_usd = Some(0.01);
    if let model::NodeKind::Agent(agent) = &mut g.nodes[1].kind {
        agent.model = Some(priced());
    }
    deploy(&fx.engine, &g);
    fx.llm.push(spending(Reply::tool("w1", "write_file", json!({"path": "hello.txt", "content": "hi\n"}))));
    let run = fx.engine.start_run(&g.id, input()).unwrap();
    let stopped = wait_run(&fx.engine, &run.id, "out of budget", |r| r.state == RunState::WaitingAnswer || !r.state.is_open());
    assert_eq!(stopped.state, RunState::WaitingAnswer, "{stopped:#?}");
    assert_eq!(fx.engine.run_waiting(&run.id).unwrap(), Some(RunWait::Budget));
    assert_eq!(fx.engine.waiting_runs().unwrap(), vec![(run.id.clone(), RunWait::Budget)]);
    eventually("the wait published", || waits(&fx, &run.id).contains(&Some(RunWait::Budget)));
    // The item is filed just after the wait is published.
    let budget_item = || fx.engine.inbox(false).unwrap().into_iter().find(|i| i.run.as_ref() == Some(&run.id));
    eventually("the budget item filed", || budget_item().is_some());
    let item = budget_item().expect("budget item");
    assert_eq!((item.kind.clone(), item.outcome), (InboxKind::Failure, Some(Outcome::Blocked)));
    assert!(item.title.starts_with("Out of budget: $0.02 of $0.01"), "{}", item.title);
    let ids: Vec<RunId> = fx.engine.runs_for_pipeline(&g.id, 5).unwrap().into_iter().map(|r: Run| r.id).collect();
    assert_eq!(ids, vec![run.id.clone()]);

    fx.llm.push(Reply::tool("f1", "step_done", json!({"summary": "Wrote hi."})));
    fx.engine.raise_run_budget(&run.id, 1.0).unwrap();
    let done = wait_run(&fx.engine, &run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
    eventually("the item resolved", || fx.engine.inbox(false).unwrap().iter().all(|i| i.run.as_ref() != Some(&run.id)));
    assert_eq!(fx.engine.run_waiting(&run.id).unwrap(), None);
    eventually("no longer waiting", || waits(&fx, &run.id).last() == Some(&None));
}

#[test]
fn a_question_left_open_by_a_restart_is_resolved_when_the_run_ends() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.nodes.truncate(2);
    g.edges.truncate(1);
    deploy(&fx.engine, &g);
    fx.llm.push(Reply::tool("a1", "ask_user", json!({"question": "Which greeting?"})));
    let run = fx.engine.start_run(&g.id, input()).unwrap();
    wait_run(&fx.engine, &run.id, "asking", |r| r.state == RunState::WaitingAnswer);
    assert_eq!(fx.engine.run_waiting(&run.id).unwrap(), Some(RunWait::Question));
    fx.engine.shutdown();

    let (engine, _) = fx.reopen();
    let question = |engine: &Engine| engine.inbox(false).unwrap().into_iter().any(|i| matches!(i.kind, InboxKind::Question { .. }));
    assert!(question(&engine));
    engine.cancel_run(&run.id).unwrap();
    wait_run(&engine, &run.id, "cancelled", |r| r.state == RunState::Cancelled);
    eventually("the question resolved", || !question(&engine));
    engine.shutdown();
}

// ---------------------------------------------------------------- worktree sweep

/// A finished real run with registered worktrees `(path, branch)` of `repo`.
fn finished_run(fx: &Fx, id: &str, repo: &std::path::Path, places: &[(&std::path::Path, &str)], pr: bool) -> RunId {
    let inner = &fx.engine.inner;
    let run = Run {
        id: RunId::new(id),
        pipeline: PipelineId::new("fix"),
        version: 1,
        issue: None,
        number: 0,
        title: id.into(),
        input: String::new(),
        state: RunState::Done,
        test: false,
        started: NOW - 10 * DAY,
        finished: Some(NOW - 9 * DAY),
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: pr.then(|| "https://github.com/acme/app/pull/1".to_string()),
        nodes: Vec::new(),
    };
    inner.store.upsert_run(&run).unwrap();
    let entries: Vec<_> = places
        .iter()
        .map(|(path, branch)| {
            json!({"workdir": "code", "node": "coder", "variant": 0, "attempt": 1, "path": path, "branch": branch, "repo": repo, "setup_done": true})
        })
        .collect();
    let dir = crate::pipelines::runner_config(inner).state.join("runs").join(base::id::slug(id, 80));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("places.json"), serde_json::to_string(&entries).unwrap()).unwrap();
    std::fs::write(dir.join("run-id"), id).unwrap();
    run.id
}

fn worktree(fx: &Fx, repo: &std::path::Path, branch: &str, commit: bool) -> std::path::PathBuf {
    let path = fx.root().join("wt").join(branch);
    git(repo, &["worktree", "add", "-q", "-b", branch, path.to_str().unwrap(), "main"]);
    if commit {
        std::fs::write(path.join("new.txt"), "new\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["-c", "user.name=T", "-c", "user.email=t@e", "commit", "-q", "-m", "work"]);
    }
    path
}

#[test]
fn the_sweep_removes_merged_or_shipped_worktrees_of_old_runs_only() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let merged = worktree(&fx, &repo, "merged", false);
    let shipped = worktree(&fx, &repo, "shipped", true);
    let dirty = worktree(&fx, &repo, "dirty", false);
    std::fs::write(dirty.join("wip.txt"), "wip\n").unwrap();
    let unmerged = worktree(&fx, &repo, "unmerged", true);
    let recent = worktree(&fx, &repo, "recent", false);
    finished_run(&fx, "r1", &repo, &[(&merged, "merged"), (&dirty, "dirty"), (&unmerged, "unmerged")], false);
    finished_run(&fx, "r2", &repo, &[(&shipped, "shipped")], true);
    let young = finished_run(&fx, "r3", &repo, &[(&recent, "recent")], false);
    let mut run = fx.engine.inner.store.run(&young).unwrap().unwrap();
    run.finished = Some(NOW - DAY);
    fx.engine.inner.store.upsert_run(&run).unwrap();

    let planned = fx.engine.sweep_worktrees(true).unwrap();
    let paths: Vec<_> = planned.iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, vec![merged.clone(), shipped.clone()], "{planned:#?}");
    assert!(merged.is_dir(), "a dry run removes nothing");

    let swept = fx.engine.sweep_worktrees(false).unwrap();
    assert_eq!(swept, planned);
    assert!(!merged.exists() && !shipped.exists());
    assert!(dirty.is_dir() && unmerged.is_dir() && recent.is_dir());
    let branches = git(&repo, &["branch", "--format=%(refname:short)"]);
    assert!(!branches.lines().any(|b| b == "merged"), "a merged branch goes: {branches}");
    assert!(branches.lines().any(|b| b == "shipped"), "an unmerged branch stays: {branches}");
    assert!(fx.engine.sweep_worktrees(false).unwrap().is_empty(), "the registry forgot them");

    fx.engine.set_worktree_retention(0).unwrap();
    assert_eq!(fx.engine.settings().unwrap().worktree_retention_days, 0);
}

// ---------------------------------------------------------------- settings and projects

#[test]
fn settings_and_projects_change_through_their_own_calls() {
    let fx = Fx::new();
    let changed = |fx: &Fx| {
        fx.drain();
        let n = fx.log.lock().unwrap().iter().filter(|e| matches!(e, Event::SettingsChanged)).count();
        fx.log.lock().unwrap().clear();
        n
    };
    fx.engine.set_daily_limit(Some(12.5)).unwrap();
    fx.engine.set_slots(2).unwrap();
    assert!(fx.engine.set_slots(0).is_err());
    assert!(fx.engine.set_daily_limit(Some(-1.0)).is_err());
    let settings = fx.engine.settings().unwrap();
    assert_eq!((settings.daily_limit_usd, settings.slots, settings.worktree_retention_days), (Some(12.5), 2, 7));
    assert_eq!(changed(&fx), 2);

    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let new = |name: &str, prefix: &str, repos: Vec<std::path::PathBuf>| NewProject { name: name.into(), key_prefix: prefix.into(), repos };
    let pay = fx.engine.create_project(new("Payments", "pay", vec![repo.clone()])).unwrap();
    assert_eq!((pay.id.as_str(), pay.key_prefix.as_str(), pay.repos.clone()), ("payments", "PAY", vec![repo]));
    assert!(fx.engine.create_project(new("Other", "PAY", Vec::new())).is_err(), "prefixes are unique");
    assert!(fx.engine.create_project(new("Other", "P4Y", Vec::new())).is_err(), "letters only");
    assert!(fx.engine.create_project(new("Other", "OTH", vec![fx.root().join("missing")])).is_err(), "folders must exist");
    assert!(fx.engine.create_project(new(" ", "OTH", Vec::new())).is_err());
    let second = fx.engine.create_project(new("Payments", "", Vec::new())).unwrap();
    assert_eq!(second.id.as_str(), "payments-2");
    assert_eq!(second.key_prefix, "PAYA", "a prefix made from the name is made unique");
    let renamed = fx.engine.rename_project(&pay.id, "Billing").unwrap();
    assert_eq!(renamed.name, "Billing");
    fx.engine.set_current_project(&pay.id).unwrap();
    assert_eq!(fx.engine.settings().unwrap().current_project, Some(pay.id.clone()));
    assert!(fx.engine.set_current_project(&ProjectId::new("nope")).is_err());
    fx.engine.delete_project(&pay.id).unwrap();
    assert_eq!(fx.engine.settings().unwrap().current_project, None);
    assert!(changed(&fx) >= 4);
}

#[test]
fn project_counts_schedules_and_spend_of_spawned_sessions() {
    let fx = Fx::new();
    let project = project(&fx);
    let issue = fx.engine.create_issue(crate::api::NewIssue { project: project.clone(), title: "One".into(), ..Default::default() }).unwrap();
    let counts = fx.engine.project_counts().unwrap();
    assert_eq!(counts, vec![crate::api::ProjectCounts { project: project.clone(), open_issues: 1, ..Default::default() }]);
    // Waiting and failed runs on its issues.
    let shell = |id: &str, state: RunState, number: u32| Run {
        id: RunId::new(id),
        pipeline: PipelineId::new("fix"),
        version: 1,
        issue: Some(issue.key.clone()),
        number,
        title: id.into(),
        input: String::new(),
        state,
        test: false,
        started: NOW + i64::from(number),
        finished: None,
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: None,
        nodes: Vec::new(),
    };
    fx.engine.inner.store.upsert_run(&shell("r1", RunState::Failed, 1)).unwrap();
    let counts = fx.engine.project_counts().unwrap().remove(0);
    assert_eq!((counts.working_runs, counts.waiting_runs, counts.failed_runs), (0, 0, 1));
    fx.engine.inner.store.upsert_run(&shell("r2", RunState::WaitingReview, 2)).unwrap();
    let counts = fx.engine.project_counts().unwrap().remove(0);
    assert_eq!((counts.working_runs, counts.waiting_runs, counts.failed_runs), (0, 1, 0), "a later run replaces the failed one");
    fx.engine.update_issue(&issue.key, crate::api::IssuePatch { status: Some(model::IssueStatus::Done), ..Default::default() }).unwrap();
    assert_eq!(fx.engine.project_counts().unwrap()[0].open_issues, 0);

    let mut daily = pilot("daily", &project, Schedule::Day { hour: 2, minute: 0 });
    daily.target = model::RunnableRef::pipeline("triage");
    fx.engine.save_autopilot(daily.clone()).unwrap();
    let mut sooner = pilot("sooner", &project, Schedule::Minutes { minutes: 10 });
    sooner.target = daily.target.clone();
    fx.engine.save_autopilot(sooner).unwrap();
    let mut off = pilot("off", &project, Schedule::Minutes { minutes: 1 });
    off.enabled = false;
    fx.engine.save_autopilot(off).unwrap();
    assert_eq!(fx.engine.next_scheduled_runs().unwrap(), vec![(daily.target.clone(), NOW + 10 * MIN)]);
}

// ---------------------------------------------------------------- what waits for a person

fn inbox_item(id: &str, kind: InboxKind, outcome: Option<Outcome>, created: i64) -> model::InboxItem {
    model::InboxItem {
        id: model::InboxId::new(id),
        kind,
        title: id.into(),
        summary: String::new(),
        outcome,
        source: "Old app".into(),
        issue: None,
        session: None,
        run: None,
        links: Vec::new(),
        created,
        read: false,
        archived: false,
    }
}

#[test]
fn the_waiting_tile_the_needs_list_and_the_inbox_agree_and_reading_changes_nothing() {
    use crate::api::{InboxApi, NeedKind, Period, Target};
    let fx = Fx::new();
    let store = &fx.engine.inner.store;
    let items = [
        // Imported pull-request reports: one asks for a review, one only tells.
        inbox_item("pr-review", InboxKind::Report, Some(Outcome::NeedsReview), NOW - 5 * DAY),
        inbox_item("pr-merged", InboxKind::Report, Some(Outcome::Done), NOW - 4 * DAY),
        inbox_item("review:v1", InboxKind::Review { review: model::ReviewId::new("v1") }, Some(Outcome::NeedsReview), NOW - HOUR),
        inbox_item("ask:r1", InboxKind::Question { request: model::RequestId::new("q") }, Some(Outcome::Blocked), NOW - 2 * HOUR),
        inbox_item("failed:r2", InboxKind::Failure, Some(Outcome::Failed), NOW - 3 * HOUR),
    ];
    for item in &items {
        store.upsert_inbox_item(item).unwrap();
    }
    let check = |expected: usize| {
        let needs = fx.engine.needs_you().unwrap();
        let stats = fx.engine.home_stats(Period::Today).unwrap();
        let badge = fx.engine.inbox(false).unwrap().iter().filter(|i| i.needs_person()).count();
        assert_eq!(needs.len(), expected, "{needs:#?}");
        assert_eq!(stats.waiting as usize, expected);
        assert_eq!(badge, needs.iter().filter(|n| matches!(n.target, Target::Inbox(_))).count());
        assert_eq!(stats.oldest_waiting, needs.first().map(|n| n.since));
        needs
    };
    let needs = check(4);
    assert_eq!(needs[0].target, Target::Inbox(model::InboxId::new("pr-review")), "the oldest first, opening its item");
    assert_eq!(needs[0].kind, NeedKind::Review);
    assert!(!needs.iter().any(|n| n.target == Target::Inbox(model::InboxId::new("pr-merged"))));

    for item in &items {
        fx.engine.set_inbox_read(&item.id, true).unwrap();
    }
    check(4);
    fx.engine.archive_inbox(&model::InboxId::new("failed:r2"), true).unwrap();
    check(3);
}
