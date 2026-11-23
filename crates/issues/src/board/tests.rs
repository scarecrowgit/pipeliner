use std::collections::HashMap;

use model::{IssueKey, IssueStatus, Priority, Run, RunId, RunState, RunnableRef};

use super::*;
use crate::testing::{issue, run};

fn card(key: &str, status: IssueStatus, prio: Priority, updated: i64, runner: Option<RunnableRef>) -> Issue {
    let mut i = issue(status);
    i.key = IssueKey::new(key);
    i.title = key.to_string();
    i.priority = prio;
    i.updated = updated;
    i.runner = runner;
    i
}

fn keys_of(issues: &[&Issue]) -> Vec<String> {
    issues.iter().map(|i| i.key.to_string()).collect()
}

fn mockup() -> Vec<Issue> {
    use IssueStatus::*;
    use Priority::*;
    vec![
        card("PAY-1440", Backlog, Low, 10, None),
        card("PAY-1442", Backlog, Medium, 20, None),
        card("PAY-1438", Todo, Low, 5, Some(RunnableRef::agent("codex"))),
        card("PAY-1436", InProgress, Medium, 30, Some(RunnableRef::orchestrator("lead"))),
        card("PAY-1427", InProgress, Medium, 40, Some(RunnableRef::pipeline("triage"))),
        card("PAY-1435", InProgress, High, 1, Some(RunnableRef::agent("auditor"))),
        card("PAY-1423", InReview, High, 50, Some(RunnableRef::pipeline("triage"))),
        card("PAY-1431", InReview, Low, 60, Some(RunnableRef::pipeline("triage"))),
        card("PAY-1419", Done, High, 70, Some(RunnableRef::agent("fixer"))),
        card("PAY-1417", Done, Low, 80, None),
    ]
}

fn runs_for(issues: &mut [Issue]) -> HashMap<RunId, Run> {
    let mut map = HashMap::new();
    for (key, state) in [("PAY-1435", RunState::WaitingAnswer), ("PAY-1423", RunState::WaitingReview), ("PAY-1431", RunState::WaitingReview), ("PAY-1427", RunState::Running), ("PAY-1419", RunState::Done)] {
        let id = RunId::new(format!("run-{key}"));
        if let Some(i) = issues.iter_mut().find(|i| i.key.as_str() == key) {
            i.run = Some(id.clone());
        }
        map.insert(id.clone(), run(id.as_str(), state));
    }
    map
}

#[test]
fn the_board_matches_the_mockup() {
    let mut issues = mockup();
    let runs = runs_for(&mut issues);
    let lookup = |id: &RunId| runs.get(id);
    let board = build(&issues, RunsWith::All, &lookup);
    let counts: Vec<_> = board.columns.iter().map(Column::count).collect();
    assert_eq!(counts, [2, 1, 3, 2, 2]);
    let needs: Vec<_> = board.columns.iter().map(|c| c.needs_label()).collect();
    assert_eq!(needs, [None, None, Some("1 needs you".into()), Some("2 need you".into()), None]);
    assert_eq!(board.open, 8);
    assert_eq!(board.needs_you, 3);
    let statuses: Vec<_> = board.columns.iter().map(|c| c.status).collect();
    assert_eq!(statuses, IssueStatus::ALL);
}

#[test]
fn cards_sort_by_priority_then_recency_then_key() {
    let issues = mockup();
    let none = |_: &RunId| None;
    let board = build(&issues, RunsWith::All, &none);
    // High first, then Medium by updated (40 before 30).
    assert_eq!(keys_of(&board.columns[2].issues), ["PAY-1435", "PAY-1427", "PAY-1436"]);
    assert_eq!(keys_of(&board.columns[0].issues), ["PAY-1442", "PAY-1440"]);
    let ties = vec![
        card("HAR-10", IssueStatus::Todo, Priority::Low, 5, None),
        card("HAR-9", IssueStatus::Todo, Priority::Low, 5, None),
        card("HAR-2", IssueStatus::Todo, Priority::Low, 5, None),
    ];
    let board = build(&ties, RunsWith::All, &none);
    assert_eq!(keys_of(&board.columns[1].issues), ["HAR-2", "HAR-9", "HAR-10"]);
}

#[test]
fn filter_chips_count_by_runner_kind() {
    let issues = mockup();
    let counts: Vec<_> = chips(&issues).iter().map(|c| (c.label, c.count)).collect();
    assert_eq!(counts, [("All", 10), ("Orchestrators", 1), ("Agents", 3), ("Pipelines", 3), ("Unattached", 3)]);
    let mut spawned = card("PAY-1", IssueStatus::Todo, Priority::Low, 0, Some(RunnableRef::spawned("x")));
    spawned.key = IssueKey::new("PAY-1");
    let c = chips(&[spawned]);
    assert_eq!(c[2].count, 1);
}

#[test]
fn filtering_the_board() {
    let mut issues = mockup();
    let runs = runs_for(&mut issues);
    let lookup = |id: &RunId| runs.get(id);
    let board = build(&issues, RunsWith::Pipelines, &lookup);
    let counts: Vec<_> = board.columns.iter().map(Column::count).collect();
    assert_eq!(counts, [0, 0, 1, 2, 0]);
    assert_eq!(board.needs_you, 2);
    assert_eq!(board.open, 3);
    let board = build(&issues, RunsWith::Unattached, &lookup);
    assert_eq!(board.columns.iter().map(Column::count).sum::<usize>(), 3);
    assert_eq!(runner_kind(&issues[3]), Some(model::RunnableKind::Orchestrator));
    assert_eq!(runner_kind(&issues[0]), None);
}

#[test]
fn needs_you_ignores_missing_and_working_runs() {
    let mut i = issue(IssueStatus::InProgress);
    let none = |_: &RunId| None;
    assert!(!needs_you(&i, &none));
    i.run = Some(RunId::new("gone"));
    assert!(!needs_you(&i, &none));
    let running = run("gone", RunState::Running);
    assert!(!needs_you(&i, &|_: &RunId| Some(&running)));
    let waiting = run("gone", RunState::WaitingAnswer);
    assert!(needs_you(&i, &|_: &RunId| Some(&waiting)));
    assert_eq!(needs_label(0), None);
    assert_eq!(needs_label(1).as_deref(), Some("1 needs you"));
    assert_eq!(needs_label(5).as_deref(), Some("5 need you"));
}

#[test]
fn an_empty_board_has_five_empty_columns() {
    let none = |_: &RunId| None;
    let board = build(&[], RunsWith::All, &none);
    assert_eq!(board.columns.len(), 5);
    assert!(board.columns.iter().all(|c| c.issues.is_empty()));
    assert_eq!((board.open, board.needs_you), (0, 0));
    assert!(chips(&[]).iter().all(|c| c.count == 0));
}

#[test]
fn list_sorting() {
    let issues = vec![
        card("PAY-10", IssueStatus::Done, Priority::Low, 5, None),
        card("PAY-9", IssueStatus::Todo, Priority::High, 5, None),
        card("PAY-2", IssueStatus::Backlog, Priority::High, 9, None),
        card("PAY-30", IssueStatus::Todo, Priority::Medium, 1, None),
    ];
    let sorted = |by, dir| keys_of(&sort_list(&issues, by, dir));
    assert_eq!(sorted(ListColumn::Key, SortDir::Asc), ["PAY-2", "PAY-9", "PAY-10", "PAY-30"]);
    assert_eq!(sorted(ListColumn::Key, SortDir::Desc), ["PAY-30", "PAY-10", "PAY-9", "PAY-2"]);
    assert_eq!(sorted(ListColumn::Priority, ListColumn::Priority.default_dir()), ["PAY-2", "PAY-9", "PAY-30", "PAY-10"]);
    assert_eq!(sorted(ListColumn::Priority, SortDir::Asc), ["PAY-10", "PAY-30", "PAY-2", "PAY-9"]);
    assert_eq!(sorted(ListColumn::Updated, SortDir::Desc), ["PAY-2", "PAY-9", "PAY-10", "PAY-30"]);
    assert_eq!(sorted(ListColumn::Status, SortDir::Asc), ["PAY-2", "PAY-9", "PAY-30", "PAY-10"]);
    // Titles are plain text, so "PAY-10" sorts before "PAY-2" here.
    assert_eq!(sorted(ListColumn::Title, SortDir::Asc), ["PAY-10", "PAY-2", "PAY-30", "PAY-9"]);
    assert_eq!(ListColumn::Key.default_dir(), SortDir::Asc);
}
