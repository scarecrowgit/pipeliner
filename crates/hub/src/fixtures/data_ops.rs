//! Home's numbers, runtimes, settings and the folders the new-chat page offers.

use crate::api::{
    HomeStats, InboxId, IssueKey, Need, NeedKind, Period, RepoHost, ReviewId, RunId, RunnableKind, RunnableRef, Runtime,
    RuntimeKind, SessionId, SpendBucket, SpendByRunnable, SpendToday, Target, WorkdirEntry, Working, WorkingState,
};

use super::data_sessions::{FIX_1423, model, path};
use super::{DAY, HOUR, MINUTE, World};

pub fn runtimes(world: &mut World) {
    world.runtimes = vec![
        Runtime {
            name: "mac-mini".into(),
            kind: RuntimeKind::Local,
            available: true,
            online: true,
            slots_total: 4,
            slots_busy: 3,
            note: "Build, Fixer test runs · 61% CPU".into(),
        },
        Runtime {
            name: "cloud".into(),
            kind: RuntimeKind::Cloud,
            available: false,
            online: true,
            slots_total: 8,
            slots_busy: 5,
            note: "$0.02 per sandbox-hour".into(),
        },
        Runtime {
            name: "laptop".into(),
            kind: RuntimeKind::Remote,
            available: false,
            online: false,
            slots_total: 2,
            slots_busy: 0,
            note: "Codex queue waits for it · 1 run".into(),
        },
    ];
}

pub fn fill(world: &mut World) {
    let now = world.now;
    runtimes(world);
    world.settings.slots = 12;
    world.settings.daily_limit_usd = Some(10.0);
    world.settings.default_model = Some(model("opus-5.5"));

    let stats = |runs, finished, spend, limit, shipped| HomeStats {
        working_now: 6,
        slots: 12,
        waiting: 4,
        oldest_waiting: Some(now - 3 * HOUR),
        runs,
        finished_percent: finished,
        spend_usd: spend,
        limit_usd: Some(limit),
        shipped,
    };
    let ops = &mut world.ops;
    ops.stats.insert(Period::Today, stats(61, 92, 3.81, 10.0, 7));
    ops.stats.insert(Period::Days7, stats(402, 90, 43.51, 70.0, 52));
    ops.stats.insert(Period::Days30, stats(1_520, 90, 188.30, 300.0, 171));
    ops.stats.insert(Period::Days90, stats(4_212, 89, 612.40, 900.0, 418));
    ops.spend_today = SpendToday { cost_usd: 3.81, limit_usd: Some(10.0) };

    let offset = base::time::utc_offset(now);
    let today = base::time::day_start(now, offset);
    let week = base::time::week_start(now, offset);
    let days = |values: &[f64]| -> Vec<SpendBucket> {
        let n = values.len() as i64;
        values.iter().enumerate().map(|(i, cost)| SpendBucket { start: today - (n - 1 - i as i64) * DAY, cost_usd: *cost }).collect()
    };
    let week_7 = [6.20, 7.40, 8.80, 5.90, 7.10, 4.30, 3.81];
    ops.buckets.insert(Period::Today, days(&week_7));
    ops.buckets.insert(Period::Days7, days(&week_7));
    let month: Vec<f64> = (0..30).map(|i| 4.0 + ((i * 37) % 23) as f64 / 4.0).chain([3.81]).skip(1).collect();
    ops.buckets.insert(Period::Days30, days(&month));
    let weeks = [38.2, 41.5, 44.0, 52.3, 47.1, 39.8, 55.6, 61.2, 49.4, 43.7, 50.9, 58.3, 30.4];
    ops.buckets.insert(
        Period::Days90,
        weeks.iter().enumerate().map(|(i, cost)| SpendBucket { start: week - (12 - i as i64) * 7 * DAY, cost_usd: *cost }).collect(),
    );

    let spenders = |values: [f64; 7]| -> Vec<SpendByRunnable> {
        let names = [
            (RunnableRef::orchestrator("payments-lead"), "Payments lead"),
            (RunnableRef::pipeline("pay-triage"), "PAY triage"),
            (RunnableRef::agent("build"), "Build"),
            (RunnableRef::agent("fixer"), "Fixer"),
            (RunnableRef::agent("codex"), "Codex"),
            (RunnableRef::agent("auditor"), "Auditor"),
            (RunnableRef("others".into()), "Others · 6"),
        ];
        names
            .into_iter()
            .zip(values)
            .map(|((runnable, name), cost)| SpendByRunnable { runnable, name: name.to_string(), cost_usd: cost, spawned: false })
            .collect()
    };
    ops.by_runnable.insert(Period::Today, spenders([1.12, 0.94, 0.62, 0.41, 0.18, 0.15, 0.39]));
    ops.by_runnable.insert(Period::Days7, spenders([12.80, 10.40, 7.90, 5.10, 2.30, 1.90, 3.11]));
    ops.by_runnable.insert(Period::Days30, spenders([56.10, 47.20, 33.00, 22.40, 9.60, 7.80, 12.20]));
    ops.by_runnable.insert(Period::Days90, spenders([184.20, 151.60, 102.30, 71.90, 29.40, 24.10, 48.90]));

    ops.needs = vec![
        Need {
            kind: NeedKind::Permission,
            title: "Allow `bun test src/charges`".into(),
            source: "Build in chat · fix/PAY-1423".into(),
            since: now - 12 * MINUTE,
            target: Target::Session(SessionId::new(FIX_1423)),
        },
        Need {
            kind: NeedKind::Review,
            title: "Pick 1 of 3 variants".into(),
            source: "PAY triage · Fixer ×3 · PAY-1423".into(),
            since: now - 40 * MINUTE,
            target: Target::Review(ReviewId::new("review-1423")),
        },
        Need {
            kind: NeedKind::Question,
            title: "Which log sinks are in scope?".into(),
            source: "Auditor · PAY-1435".into(),
            since: now - 40 * MINUTE,
            target: Target::Inbox(InboxId::new("inbox-1435")),
        },
        Need {
            kind: NeedKind::Review,
            title: "Pick 1 of 2 specs".into(),
            source: "PAY triage · Analyst ×2 · PAY-1431".into(),
            since: now - 3 * HOUR,
            target: Target::Review(ReviewId::new("review-1431")),
        },
    ];

    let working = |name: &str, kind: &str, runnable_kind, spawned, task: &str, key: &str, runtime: &str, minutes: i64, cost, state, target| Working {
        name: name.into(),
        kind: kind.into(),
        runnable_kind,
        spawned,
        task: task.into(),
        key: key.into(),
        runtime: runtime.into(),
        since: now - minutes * MINUTE,
        cost_usd: cost,
        state,
        target,
    };
    let running = |percent| WorkingState::Running { percent: Some(percent) };
    let issue = |key: &str| Target::Issue(IssueKey::new(key));
    use RunnableKind::*;
    ops.working = vec![
        working("Payments lead", "Orchestrator", Orchestrator, false, "Retry budget per merchant", "PAY-1436", "cloud", 31, 0.44, running(40), issue("PAY-1436")),
        working("PAY triage", "Pipeline · Reproducer", Pipeline, false, "Refund webhook times out", "PAY-1427", "cloud", 20, 0.12, running(64), Target::Run(RunId::new("run-1427"))),
        working("Migration checker", "created by Payments lead", Agent, true, "Read SDK v9 diff for Codex", "PAY-1438", "cloud", 9, 0.08, running(50), issue("PAY-1438")),
        working("Log digger", "created by Reproducer", Agent, true, "Search 24h of logs", "PAY-1427", "cloud", 6, 0.02, running(70), Target::Run(RunId::new("run-1427"))),
        working(
            "Build",
            "Agent · chat",
            Agent,
            false,
            "Fix double charge on retry",
            "fix/PAY-1423",
            "mac-mini",
            12,
            0.21,
            WorkingState::Waiting { text: "asks to run bun test".into() },
            Target::Session(SessionId::new(FIX_1423)),
        ),
        working(
            "Auditor",
            "Agent",
            Agent,
            false,
            "Audit PCI log redaction",
            "PAY-1435",
            "cloud",
            40,
            0.15,
            WorkingState::Waiting { text: "asks which log sinks".into() },
            Target::Inbox(InboxId::new("inbox-1435")),
        ),
    ];

    let entry = |repo: &str, branch: &str, folder: Option<&str>, host, status: &str, owner: Option<&str>, sessions| WorkdirEntry {
        path: match folder {
            Some(folder) => path(&format!(".harness/worktrees/{repo}/{folder}")),
            None => path(&format!("code/acme/{repo}")),
        },
        repo: repo.into(),
        branch: Some(branch.into()),
        host,
        is_worktree: folder.is_some(),
        status: status.into(),
        owner: owner.map(str::to_string),
        sessions,
    };
    world.workdirs = vec![
        entry("payments-api", "main", None, RepoHost::GitHub, "clean", None, 4),
        entry("checkout-web", "feat/saved-cards", None, RepoHost::GitLab, "3 changed", None, 3),
        entry("infra", "main", None, RepoHost::Local, "no remote", None, 0),
    ];
    world.worktrees = vec![
        entry("payments-api", "fix/PAY-1423", Some("fix-PAY-1423"), RepoHost::GitHub, "2 uncommitted · 1 ahead of main", Some("Fixer"), 1),
        entry("payments-api", "pay-1436-retry-budget", Some("pay-1436"), RepoHost::GitHub, "clean", Some("Payments lead"), 1),
        entry("payments-api", "chat/refund-timeout", Some("chat-refund-timeout"), RepoHost::GitHub, "clean", Some("your chat"), 1),
    ];
}
