//! Projects, the PAY issue board, activity feeds and autopilots.

use crate::api::{
    Actor, Attachment, Autopilot, AutopilotId, AutopilotTrigger, BlobId, Issue, IssueEvent, IssueEventKind, IssueKey,
    IssueStatus, IssueType, Priority, Project, ProjectId, RunId, RunnableRef, Schedule, StartTriggers,
};

use super::data_sessions::path;
use super::{DAY, HOUR, MINUTE, World};

pub const PAYMENTS: &str = "payments";

pub fn projects(world: &mut World) {
    let project = |id: &str, name: &str, prefix: &str, repos: &[&str]| Project {
        id: ProjectId::new(id),
        name: name.to_string(),
        key_prefix: prefix.to_string(),
        repos: repos.iter().map(|repo| path(&format!("code/acme/{repo}"))).collect(),
    };
    world.projects = vec![
        project(PAYMENTS, "Payments", "PAY", &["payments-api", "checkout-web", "ledger"]),
        project("checkout", "Checkout", "CHK", &["checkout-web", "storefront"]),
        project("platform", "Platform", "PLT", &["infra", "gateway"]),
        project("data", "Data", "DAT", &["warehouse", "dbt-models"]),
        project("side", "Side projects", "SIDE", &[]),
    ];
    world.settings.current_project = Some(ProjectId::new(PAYMENTS));
}

struct Card {
    key: &'static str,
    status: IssueStatus,
    priority: Priority,
    owner: &'static str,
    title: &'static str,
    kind: IssueType,
    runner: Option<RunnableRef>,
    body: &'static str,
    age: i64,
}

pub fn fill(world: &mut World) {
    projects(world);
    let now = world.now;
    use IssueStatus::*;
    use IssueType::*;
    use Priority::*;
    let pay_triage = || Some(RunnableRef::pipeline("pay-triage"));
    let cards = [
        Card { key: "PAY-1440", status: Backlog, priority: Low, owner: "Alex Rivera", title: "Flaky checkout e2e on Safari", kind: Bug, runner: None,
            body: "`checkout.spec` fails about 1 in 8 runs on Safari 19. Looks like a race on the 3-D Secure iframe.", age: DAY },
        Card { key: "PAY-1442", status: Backlog, priority: Medium, owner: "You", title: "Add receipts to refund emails", kind: Story, runner: None,
            body: "Refund emails should carry the same receipt block as payment emails.", age: 5 * HOUR },
        Card { key: "PAY-1438", status: Todo, priority: Low, owner: "Alex Rivera", title: "Bump gateway SDK to v9", kind: Chore, runner: Some(RunnableRef::agent("codex")),
            body: "v9 changes webhook payloads; see the migration guide.", age: 2 * DAY },
        Card { key: "PAY-1436", status: InProgress, priority: Medium, owner: "Nina Kim", title: "Retry budget per merchant", kind: Story,
            runner: Some(RunnableRef::orchestrator("payments-lead")), body: "Cap automatic retries per merchant per hour.", age: 3 * DAY },
        Card { key: "PAY-1427", status: InProgress, priority: Medium, owner: "You", title: "Refund webhook times out", kind: Bug, runner: pay_triage(),
            body: "Refunds with many line items hit the 10s gateway timeout.", age: DAY },
        Card { key: "PAY-1435", status: InProgress, priority: High, owner: "Alex Rivera", title: "Audit PCI log redaction", kind: Chore,
            runner: Some(RunnableRef::agent("auditor")), body: "Check that card numbers never reach any log sink.", age: 2 * DAY },
        Card { key: "PAY-1423", status: InReview, priority: High, owner: "You", title: "Double charge on retry", kind: Bug, runner: pay_triage(),
            body: "A timeout on the first charge makes the retry charge the card a second time.", age: 3 * DAY },
        Card { key: "PAY-1431", status: InReview, priority: Low, owner: "Nina Kim", title: "Saved cards for guest checkout", kind: Story, runner: pay_triage(),
            body: "Let guests save a card for their next order without an account.", age: 4 * DAY },
        Card { key: "PAY-1419", status: Done, priority: High, owner: "Alex Rivera", title: "Webhook signature check", kind: Bug,
            runner: Some(RunnableRef::agent("fixer")), body: "Reject webhooks whose signature does not match.", age: 6 * DAY },
        Card { key: "PAY-1417", status: Done, priority: Low, owner: "Alex Rivera", title: "Update payout docs", kind: Docs, runner: None,
            body: "Payout schedule changed to T+2.", age: 8 * DAY },
    ];
    for card in cards {
        let key = IssueKey::new(card.key);
        let run = match card.key {
            "PAY-1423" => Some(RunId::new("run-1423")),
            "PAY-1427" => Some(RunId::new("run-1427")),
            "PAY-1431" => Some(RunId::new("run-1431")),
            _ => None,
        };
        world.issues.push(Issue {
            key: key.clone(),
            project: ProjectId::new(PAYMENTS),
            title: card.title.to_string(),
            body: card.body.to_string(),
            status: card.status,
            priority: card.priority,
            kind: card.kind,
            owner: card.owner.to_string(),
            repo: Some("payments-api".to_string()),
            created: now - card.age,
            updated: now - card.age / 4,
            filed_by: (card.key == "PAY-1442").then(|| RunnableRef::agent("analyst")),
            runner: card.runner,
            triggers: StartTriggers::default(),
            run_count: u32::from(run.is_some()) + u32::from(card.key == "PAY-1427"),
            run,
            pr_url: (card.key == "PAY-1419").then(|| "https://github.com/acme/payments-api/pull/2280".to_string()),
            images: Vec::new(),
        });
        world.issue_events.insert(
            key,
            vec![IssueEvent { seq: 1, at: now - card.age, actor: Actor::Me, kind: IssueEventKind::Created }],
        );
    }
    checkout_issue(world);
    feed_1440(world);
    feed_1427(world);
    autopilots(world);
}

/// An issue of another project whose run failed, so the project switcher has a failed run
/// to show (the run is in `data_runs`).
fn checkout_issue(world: &mut World) {
    let now = world.now;
    let key = IssueKey::new("DAT-58");
    world.issues.push(Issue {
        key: key.clone(),
        project: ProjectId::new("data"),
        title: "Nightly export misses refunds".to_string(),
        body: "The warehouse export skips refunds booked after 23:00.".to_string(),
        status: IssueStatus::InProgress,
        priority: Priority::Medium,
        kind: IssueType::Bug,
        owner: "Nina Kim".to_string(),
        repo: Some("warehouse".to_string()),
        created: now - 2 * DAY,
        updated: now - 3 * HOUR,
        filed_by: None,
        runner: Some(RunnableRef::pipeline("pay-triage")),
        triggers: StartTriggers::default(),
        run_count: 1,
        run: Some(RunId::new("run-dat-58")),
        pr_url: None,
        images: Vec::new(),
    });
    world.issue_events.insert(key, vec![IssueEvent { seq: 1, at: now - 2 * DAY, actor: Actor::Me, kind: IssueEventKind::Created }]);
}

/// The next 02:00 local time after `now`.
fn next_two_am(now: i64) -> i64 {
    let today = base::time::day_start(now, base::time::utc_offset(now)) + 2 * HOUR;
    if today > now { today } else { base::time::day_start(today + DAY, base::time::utc_offset(today + DAY)) + 2 * HOUR }
}

fn runnable(reference: RunnableRef, name: &str) -> Actor {
    let spawned = reference.scheme() == "spawned";
    Actor::Runnable { reference, name: name.to_string(), spawned }
}

fn feed_1440(world: &mut World) {
    let now = world.now;
    let report = Attachment {
        blob: BlobId::new("blob-nightly-e2e-report"),
        name: "nightly-e2e-report.png".to_string(),
        media_type: "image/png".to_string(),
        bytes: 380 * 1024,
    };
    if let Some(feed) = world.issue_events.get_mut(&IssueKey::new("PAY-1440")) {
        feed.push(IssueEvent {
            seq: 2,
            at: now - DAY,
            actor: Actor::Me,
            kind: IssueEventKind::Comment { text: "Filed from the nightly e2e report.".to_string(), images: vec![report], mentions: Vec::new() },
        });
    }
}

fn feed_1427(world: &mut World) {
    let now = world.now;
    let fixer = RunnableRef::agent("fixer");
    let codex = RunnableRef::agent("codex");
    let events = vec![
        (13, Actor::Me, IssueEventKind::Attached { runner: RunnableRef::pipeline("pay-triage") }),
        (12, runnable(RunnableRef::pipeline("pay-triage"), "PAY triage"), IssueEventKind::Note { text: "Picked up by rule 2 → Reproducer.".into() }),
        (9, runnable(RunnableRef::agent("reproducer"), "Reproducer"), IssueEventKind::Note { text: "Spawned Log digger to search the last 24h of logs.".into() }),
        (
            6,
            Actor::Me,
            IssueEventKind::Comment {
                text: "@Fixer the slow ones all have 400+ line items. Try batching `RefundBatcher` by 50 per call.".into(),
                images: Vec::new(),
                mentions: vec![fixer.clone()],
            },
        ),
        (
            6,
            Actor::System,
            IssueEventKind::Woken {
                target: fixer.clone(),
                name: "Fixer".into(),
                run: Some(RunId::new("run-1427")),
                number: 2,
                cause: "comment passed as input".into(),
            },
        ),
        (
            5,
            runnable(fixer, "Fixer"),
            IssueEventKind::Comment { text: "Picking this up. Will batch by 50 and rerun `refund.spec`.".into(), images: Vec::new(), mentions: Vec::new() },
        ),
        (
            2,
            Actor::Me,
            IssueEventKind::Comment { text: "@Codex write a second variant in parallel.".into(), images: Vec::new(), mentions: vec![codex.clone()] },
        ),
        (
            2,
            Actor::System,
            IssueEventKind::WakeFailed { target: codex, name: "Codex".into(), reason: "runtime laptop offline since 14:02 · reconnect it or retry".into() },
        ),
    ];
    if let Some(feed) = world.issue_events.get_mut(&IssueKey::new("PAY-1427")) {
        for (minutes, actor, kind) in events {
            let seq = feed.len() as u64 + 1;
            feed.push(IssueEvent { seq, at: now - minutes * MINUTE, actor, kind });
        }
    }
}

fn autopilots(world: &mut World) {
    let now = world.now;
    let project = ProjectId::new(PAYMENTS);
    world.autopilots = vec![
        Autopilot {
            id: AutopilotId::new("ap-nightly-e2e"),
            project: project.clone(),
            name: "Nightly e2e triage".into(),
            trigger: AutopilotTrigger::Schedule { schedule: Schedule::Day { hour: 2, minute: 0 } },
            target: RunnableRef::pipeline("pay-triage"),
            prompt: "Triage last night's e2e failures and file an issue for each new one.".into(),
            enabled: true,
            paused_reason: None,
            next_run: Some(now + 5 * HOUR + 40 * MINUTE),
            last_run: Some(now - 18 * HOUR),
            last_result: "filed 2 issues".into(),
            budget_usd: Some(1.0),
        },
        Autopilot {
            id: AutopilotId::new("ap-sentry"),
            project: project.clone(),
            name: "New Sentry error".into(),
            trigger: AutopilotTrigger::Event { source: "sentry: new error in payments-api".into() },
            target: RunnableRef::agent("reproducer"),
            prompt: "Reproduce the error and attach the logs.".into(),
            enabled: true,
            paused_reason: None,
            next_run: None,
            last_run: Some(now - 2 * HOUR),
            last_result: "3 issues today".into(),
            budget_usd: Some(0.5),
        },
        Autopilot {
            id: AutopilotId::new("ap-nightly-audit"),
            project: ProjectId::new("platform"),
            name: "Nightly audit".into(),
            trigger: AutopilotTrigger::Schedule { schedule: Schedule::Day { hour: 2, minute: 0 } },
            target: RunnableRef::pipeline("nightly-audit"),
            prompt: "Audit log redaction and dependency licences.".into(),
            enabled: true,
            paused_reason: None,
            next_run: Some(next_two_am(now)),
            last_run: Some(next_two_am(now) - 2 * DAY),
            last_result: "started a run".into(),
            budget_usd: Some(0.5),
        },
        Autopilot {
            id: AutopilotId::new("ap-deps"),
            project,
            name: "Weekly dependency bump".into(),
            trigger: AutopilotTrigger::Schedule { schedule: Schedule::Week { weekday: 0, hour: 9, minute: 0 } },
            target: RunnableRef::agent("codex"),
            prompt: "Bump dependencies with patch releases and open one PR.".into(),
            enabled: false,
            paused_reason: Some("GitLab token expires in 3 days".into()),
            next_run: None,
            last_run: Some(now - 6 * DAY),
            last_result: "opened 1 PR".into(),
            budget_usd: None,
        },
    ];
}
