//! Tests of the dashboard aggregation (`stats`) and row building (`rows`).

use agent_tools::inbox::{InboxItem, Outcome};
use gpui::SharedString;
use pipeline::{AgentUsage, Run, RunStatus, StageStatus};
use ui::nav::{ChatSessionInfo, ChatStatus};

use crate::data::{Kind, Range, Target, day_start};
use crate::rows::{RowState, live_rows, need_rows};
use crate::stats::*;

/// 2026-11-22 12:00 UTC (a Sunday).
const NOW: u64 = 1_795_348_800;
const HOUR: u64 = 3600;
const DAY: u64 = 86_400;

fn run(key: &str, created: u64, status: RunStatus) -> RunSummary {
    // The built-in pipeline: triage, mockup, render, approval, merge-mockup, build,
    // pull-request.
    let mut r = Run::new(key, &pipeline::config::legacy_default());
    r.created = created;
    r.status = status;
    if status == RunStatus::WaitingApproval {
        r.stages[3].status = StageStatus::Waiting;
    }
    RunSummary::new(&r, &format!("Title of {key}"), false)
}

fn set_stage(
    r: &mut RunSummary,
    id: &str,
    status: StageStatus,
    started: u64,
    finished: u64,
) {
    let s = r.stages.iter_mut().find(|s| s.id == id).unwrap();
    s.status = status;
    s.started = started;
    s.finished = finished;
}

fn usage(who: &str, cost: f64) -> AgentUsage {
    AgentUsage {
        who: who.into(),
        model: "p/m".into(),
        tokens: 1000,
        cost_usd: cost,
    }
}

fn chat(id: &str, status: ChatStatus, agent: &str, updated: u64, cost: f64) -> ChatSessionInfo {
    ChatSessionInfo {
        id: SharedString::from(id.to_string()),
        title: SharedString::from(format!("Chat {id}")),
        status,
        pinned: false,
        updated,
        agent: SharedString::from(agent.to_string()),
        workdir: "repo · main".into(),
        cost_usd: cost,
    }
}

fn report(id: &str, created: u64, read: bool, archived: bool) -> InboxItem {
    InboxItem {
        id: id.into(),
        created,
        chat_id: String::new(),
        chat_title: String::new(),
        agent: "Build".into(),
        workdir: None,
        wd_label: String::new(),
        title: format!("Report {id}"),
        summary: String::new(),
        outcome: Outcome::NeedsReview,
        links: Vec::new(),
        read,
        archived,
        auto: false,
    }
}

#[test]
fn empty_sources_give_zero_kpis_and_no_rows() {
    let k = kpis(&[], &[], &[], Range::Quarter.start(NOW, 0));
    assert_eq!(k, Kpis::default());
    assert_eq!(k.done_pct(), None);
    assert!(live_rows(&[], &[], &"host".into(), NOW).is_empty());
    assert!(need_rows(&[], &[], &[], NOW).is_empty());
    assert!(spend_by_agent(&[], &[], 0).is_empty());
    let days = daily_spend(&[], &[], NOW, 0);
    assert!(days.iter().all(|d| d.total() == 0.));
}

#[test]
fn range_starts_at_local_midnight() {
    // UTC: today starts at 00:00 of 2026-11-22.
    assert_eq!(Range::Today.start(NOW, 0), NOW - 12 * HOUR);
    assert_eq!(Range::Week.start(NOW, 0), NOW - 12 * HOUR - 6 * DAY);
    // UTC+3: local time is 15:00, midnight was 15 hours ago.
    assert_eq!(Range::Today.start(NOW, 3 * 3600), NOW - 15 * HOUR);
    assert_eq!(day_start(NOW, -5 * 3600, 0), NOW - 7 * HOUR);
}

#[test]
fn kpis_count_working_waiting_runs_spend_and_shipped() {
    let today = Range::Today.start(NOW, 0);
    let mut working = run("HAR-3", NOW - HOUR, RunStatus::Running);
    working.cost_usd = 0.5;
    let mut waiting = run("HAR-2", NOW - 2 * HOUR, RunStatus::WaitingApproval);
    set_stage(
        &mut waiting,
        "mockup",
        StageStatus::Done,
        NOW - 2 * HOUR,
        NOW - 90 * 60,
    );
    set_stage(
        &mut waiting,
        "approval",
        StageStatus::Waiting,
        0,
        0,
    );
    let mut shipped = run("HAR-1", NOW - 3 * HOUR, RunStatus::Done);
    shipped.cost_usd = 1.25;
    shipped.pr_url = Some("https://example.com/pr/1".into());
    set_stage(
        &mut shipped,
        "pull-request",
        StageStatus::Done,
        NOW - HOUR,
        NOW - HOUR,
    );
    let failed = run("HAR-0", NOW - 4 * HOUR, RunStatus::Failed);
    let old = run("HAR-9", NOW - 3 * DAY, RunStatus::Done);
    let runs = [working, waiting, shipped, failed, old];

    let chats = [
        chat("a", ChatStatus::Running, "Build", NOW - 60, 0.2),
        chat("b", ChatStatus::Waiting, "Plan", NOW - 30 * 60, 0.1),
        chat("c", ChatStatus::Idle, "Build", NOW - 2 * DAY, 3.0),
    ];
    let inbox = [
        report("r1", NOW - 5 * HOUR, false, false),
        report("r2", NOW - 6 * HOUR, true, false),
        report("r3", NOW - 7 * HOUR, false, true),
    ];

    let k = kpis(&runs, &chats, &inbox, today);
    assert_eq!((k.working_runs, k.working_chats, k.working()), (1, 1, 2));
    // Approval + waiting chat + one unread report; the oldest is the report.
    assert_eq!(k.waiting, 3);
    assert_eq!(k.oldest_wait, Some(NOW - 5 * HOUR));
    // Created today: HAR-3, HAR-2, HAR-1, HAR-0; finished: HAR-1 (done), HAR-0 (failed).
    assert_eq!((k.runs, k.finished, k.done), (4, 2, 1));
    assert_eq!(k.done_pct(), Some(50));
    assert!((k.spend_runs - 1.75).abs() < 1e-9);
    assert!((k.spend_chats - 0.3).abs() < 1e-9);
    assert_eq!(k.shipped, 1);

    // A wider range picks up the old run and the idle chat.
    let k = kpis(&runs, &chats, &inbox, Range::Week.start(NOW, 0));
    assert_eq!(k.runs, 5);
    assert!((k.spend_chats - 3.3).abs() < 1e-9);
}

#[test]
fn spend_per_day_buckets_by_local_day() {
    let mut a = run("HAR-1", NOW - HOUR, RunStatus::Done);
    a.cost_usd = 1.0;
    let mut b = run("HAR-2", NOW - DAY, RunStatus::Done);
    b.cost_usd = 2.0;
    let mut too_old = run("HAR-3", NOW - 8 * DAY, RunStatus::Done);
    too_old.cost_usd = 9.0;
    let chats = [
        chat("a", ChatStatus::Idle, "Build", NOW - 6 * DAY, 0.5),
        chat("b", ChatStatus::Idle, "Build", NOW - 2 * HOUR, 0.25),
    ];
    let days = daily_spend(&[a, b, too_old], &chats, NOW, 0);
    assert_eq!(days[6].start, NOW - 12 * HOUR);
    assert_eq!(days[0].start, NOW - 12 * HOUR - 6 * DAY);
    assert_eq!((days[6].runs, days[6].chats), (1.0, 0.25));
    assert_eq!(days[5].runs, 2.0);
    assert_eq!(days[0].chats, 0.5);
    let total: f64 = days.iter().map(DaySpend::total).sum();
    assert!((total - 3.75).abs() < 1e-9);
}

#[test]
fn axis_top_is_a_nice_number() {
    assert_eq!(nice_ceil(0.), 0.);
    assert_eq!(nice_ceil(0.39), 0.5);
    assert_eq!(nice_ceil(1.0), 1.0);
    assert_eq!(nice_ceil(1.2), 2.0);
    assert_eq!(nice_ceil(3.81), 5.0);
    assert_eq!(nice_ceil(8.8), 10.0);
    assert_eq!(nice_ceil(41.2), 50.0);
}

#[test]
fn spend_by_agent_groups_roles_and_chat_agents() {
    let mut r = run("HAR-1", NOW - HOUR, RunStatus::Done);
    r.usage = vec![
        usage("Triage", 0.01),
        usage("Mockup", 0.4),
        usage("Build", 0.6),
        usage("Coder 1", 0.3),
        usage("Coder 2", 0.2),
    ];
    // Cost not broken down by agent lands in "Pipeline".
    r.cost_usd = 1.51 + 0.1;
    let old = {
        let mut o = run("HAR-0", NOW - 40 * DAY, RunStatus::Done);
        o.usage = vec![usage("Mockup", 5.0)];
        o.cost_usd = 5.0;
        o
    };
    let chats = [
        chat("a", ChatStatus::Idle, "Build", NOW - HOUR, 0.7),
        chat("b", ChatStatus::Idle, "Build", NOW - 2 * HOUR, 0.1),
        chat("c", ChatStatus::Idle, "Plan", NOW - HOUR, 0.0),
    ];
    let by = spend_by_agent(&[r, old], &chats, Range::Week.start(NOW, 0));
    let names: Vec<(&str, f64)> = by.iter().map(|a| (a.name.as_str(), a.cost)).collect();
    assert_eq!(names[0].0, "Build");
    assert!((names[0].1 - 0.8).abs() < 1e-9);
    assert_eq!(names[1], ("Build", 0.6));
    assert_eq!(names[2].0, "Coders");
    assert!((names[2].1 - 0.5).abs() < 1e-9);
    assert_eq!(names[3], ("Mockup", 0.4));
    assert_eq!(names[4].0, "Pipeline");
    assert_eq!(names[5], ("Triage", 0.01));
    // Zero-cost Plan chat and the 40-day-old run are left out.
    assert_eq!(by.len(), 6);
    assert_eq!(by[0].kind, Some(Kind::Chat));
    assert_eq!(by[1].kind, Some(Kind::Pipeline));
}

#[test]
fn spend_by_agent_folds_the_tail_into_others() {
    let chats: Vec<ChatSessionInfo> = (0..10)
        .map(|i| {
            chat(
                &i.to_string(),
                ChatStatus::Idle,
                &format!("Agent {i}"),
                NOW,
                1.0 + i as f64,
            )
        })
        .collect();
    let by = spend_by_agent(&[], &chats, 0);
    assert_eq!(by.len(), AGENT_ROWS);
    let others = by.last().unwrap();
    assert_eq!(others.name, "Others · 4");
    assert_eq!(others.kind, None);
    assert!((others.cost - (1. + 2. + 3. + 4.)).abs() < 1e-9);
}

#[test]
fn coder_roles() {
    let subs = ["Coder".to_string(), "Reviewer".to_string()];
    assert_eq!(role_of("Coder 3", &subs), "Coders");
    assert_eq!(role_of("Coder", &subs), "Coders");
    assert_eq!(role_of("Reviewer 1", &subs), "Reviewers");
    assert_eq!(role_of("Coderbot", &subs), "Coderbot");
    assert_eq!(role_of("Coder 3", &[]), "Coder 3");
    assert_eq!(role_of("Mockup", &subs), "Mockup");
}

#[test]
fn working_now_rows_show_stage_agent_elapsed_and_progress() {
    let mut r = run("HAR-4", NOW - HOUR, RunStatus::Running);
    r.active = true;
    r.cost_usd = 0.42;
    set_stage(
        &mut r,
        "triage",
        StageStatus::Done,
        NOW - HOUR,
        NOW - 50 * 60,
    );
    set_stage(
        &mut r,
        "mockup",
        StageStatus::Done,
        NOW - 50 * 60,
        NOW - 30 * 60,
    );
    set_stage(
        &mut r,
        "approval",
        StageStatus::Done,
        0,
        NOW - 25 * 60,
    );
    set_stage(
        &mut r,
        "build",
        StageStatus::Running,
        NOW - 20 * 60,
        0,
    );
    let done = run("HAR-1", NOW - DAY, RunStatus::Done);
    let chats = [
        chat("a", ChatStatus::Running, "Build", NOW - 120, 0.2),
        chat("b", ChatStatus::Idle, "Build", NOW - 120, 0.2),
    ];
    let rows = live_rows(&[r, done], &chats, &"box".into(), NOW);
    assert_eq!(rows.len(), 2);
    let run_row = &rows[0];
    assert_eq!(run_row.kind, Kind::Pipeline);
    // A model step works under its own name.
    assert_eq!(run_row.name.as_ref(), "Build");
    assert_eq!(run_row.sub.as_ref(), "Pipeline · Build");
    assert_eq!(run_row.reference.as_ref(), "HAR-4");
    assert_eq!(run_row.elapsed.as_ref(), "20m");
    assert_eq!(run_row.cost.as_ref(), "$0.42");
    assert_eq!(run_row.runtime.as_ref(), "box");
    assert_eq!(
        run_row.state,
        RowState::Running("Build · 3/7".into(), Some(3. / 7.))
    );
    assert_eq!(run_row.target, Target::Issue("HAR-4".into()));
    assert_eq!(rows[1].kind, Kind::Chat);
    assert_eq!(rows[1].target, Target::Chat("a".into()));
}

#[test]
fn needs_you_lists_approvals_chats_and_unread_reports() {
    let mut w = run("HAR-2", NOW - 2 * HOUR, RunStatus::WaitingApproval);
    set_stage(
        &mut w,
        "triage",
        StageStatus::Done,
        NOW - 2 * HOUR,
        NOW - 2 * HOUR,
    );
    set_stage(
        &mut w,
        "mockup",
        StageStatus::Done,
        NOW - 2 * HOUR,
        NOW - 40 * 60,
    );
    set_stage(&mut w, "approval", StageStatus::Waiting, 0, 0);
    let chats = [chat("b", ChatStatus::Waiting, "Plan", NOW - 5 * 60, 0.)];
    let inbox = [
        report("new", NOW - 60 * 60, false, false),
        report("old", NOW - 3 * HOUR, false, false),
        report("seen", NOW - HOUR, true, false),
    ];
    let needs = need_rows(&[w.clone()], &chats, &inbox, NOW);
    let titles: Vec<&str> = needs.iter().map(|n| n.title.as_ref()).collect();
    assert_eq!(
        titles,
        [
            "Review Mockup for HAR-2",
            "Answer Chat b",
            "Report old",
            "Report new"
        ]
    );
    assert_eq!(needs[0].age.as_ref(), "40m");
    assert_eq!(needs[0].target, Target::Issue("HAR-2".into()));
    assert_eq!(needs[1].action.as_ref(), "Answer");
    assert_eq!(needs[2].target, Target::Inbox("old".into()));
    // The waiting run is also listed in "Working now", as waiting.
    let live = live_rows(&[w], &[], &"box".into(), NOW);
    assert_eq!(
        live[0].state,
        RowState::Waiting("awaits your approval".into())
    );
    assert_eq!(live[0].name.as_ref(), "You");
}
