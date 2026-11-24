use std::path::PathBuf;

use model::{
    BlobId, FileChange, InboxId, InboxItem, InboxKind, IssueKey, Link, LogLevel, NodeId, NodeInstance, NodeStatus,
    Outcome, PipelineId, Render, RequestId, Review, ReviewId, ReviewNote, ReviewState, Run, RunId, RunLogLine,
    RunState, SessionId, SpanKind, TestResult, Usage, Variant,
};

use super::store;

fn run(id: &str, issue: Option<&str>, number: u32, started: i64, state: RunState) -> Run {
    Run {
        id: RunId::new(id),
        pipeline: PipelineId::new("fix"),
        version: 3,
        issue: issue.map(IssueKey::new),
        number,
        title: format!("Run {id}"),
        input: "the body".into(),
        state,
        test: false,
        started,
        finished: None,
        cost_usd: 0.0,
        usage: Usage::default(),
        error: None,
        pr_url: None,
        nodes: Vec::new(),
    }
}

fn node(id: &str, variant: u32) -> NodeInstance {
    NodeInstance { question: None,
        node: NodeId::new(id),
        name: id.to_uppercase(),
        variant,
        attempt: 2,
        status: NodeStatus::Running,
        kind: SpanKind::Spawned,
        port: Some("out".into()),
        session: Some(SessionId::new("s1")),
        worktree: Some(PathBuf::from("/home/u/.harness/worktrees/x")),
        branch: Some("fix/x".into()),
        started: Some(5),
        finished: None,
        cost_usd: 0.25,
        summary: "did it".into(),
        spawned_by: Some(NodeId::new("lead")),
    }
}

fn line(text: &str) -> RunLogLine {
    RunLogLine { seq: 0, at: 1, node: Some(NodeId::new("fixer")), variant: 1, level: LogLevel::Tool, text: text.into() }
}

#[test]
fn a_run_round_trips_with_its_node_instances() {
    let store = store();
    let mut r = run("r1", Some("HAR-1"), 1, 100, RunState::WaitingReview);
    r.test = true;
    r.finished = Some(900);
    r.cost_usd = 1.5;
    r.usage = Usage { input: 1, output: 2, cache_read: 3, cache_write: 4 };
    r.error = Some("budget".into());
    r.pr_url = Some("https://x/pull/1".into());
    r.nodes = vec![node("fixer", 1), node("fixer", 2)];
    store.upsert_run(&r).unwrap();
    assert_eq!(store.run(&r.id).unwrap(), Some(r.clone()));
    r.state = RunState::Done;
    r.nodes.push(node("review", 0));
    store.upsert_run(&r).unwrap();
    assert_eq!(store.run(&r.id).unwrap(), Some(r));
    assert_eq!(store.run(&RunId::new("none")).unwrap(), None);

    let mut bad = run("r2", None, 1, 1, RunState::Queued);
    let mut n = node("x", 0);
    n.worktree = Some(PathBuf::from("~/wt"));
    bad.nodes = vec![n];
    assert!(store.upsert_run(&bad).is_err());
}

#[test]
fn runs_list_per_issue_latest_and_open() {
    let store = store();
    store.upsert_run(&run("a1", Some("HAR-1"), 1, 100, RunState::Done)).unwrap();
    store.upsert_run(&run("a2", Some("HAR-1"), 2, 200, RunState::Interrupted)).unwrap();
    store.upsert_run(&run("b1", Some("HAR-2"), 1, 150, RunState::Failed)).unwrap();
    store.upsert_run(&run("m1", None, 0, 50, RunState::Running)).unwrap();
    store.upsert_run(&run("c1", Some("HAR-3"), 1, 300, RunState::Cancelled)).unwrap();

    let ids = |runs: Vec<Run>| -> Vec<String> { runs.into_iter().map(|r| r.id.0).collect() };
    assert_eq!(ids(store.runs_for_issue(&IssueKey::new("HAR-1")).unwrap()), ["a2", "a1"]);
    assert_eq!(store.latest_run(&IssueKey::new("HAR-1")).unwrap().unwrap().id.as_str(), "a2");
    assert_eq!(store.latest_run(&IssueKey::new("HAR-9")).unwrap(), None);
    assert_eq!(ids(store.latest_runs().unwrap()), ["a2", "b1", "c1"]);
    assert_eq!(ids(store.open_runs().unwrap()), ["m1", "a2"]);

    // Closing a run takes it off the open list.
    store.upsert_run(&run("a2", Some("HAR-1"), 2, 200, RunState::Done)).unwrap();
    assert_eq!(ids(store.open_runs().unwrap()), ["m1"]);
}

#[test]
fn logs_are_numbered_and_paged() {
    let store = store();
    let id = RunId::new("r1");
    assert!(store.append_run_log(&id, &[line("x")]).is_err());
    store.upsert_run(&run("r1", None, 0, 1, RunState::Running)).unwrap();
    let first = store.append_run_log(&id, &[line("a"), line("b"), line("c")]).unwrap();
    assert_eq!(first.iter().map(|l| l.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(first[0], RunLogLine { seq: 1, ..line("a") });
    let more: Vec<RunLogLine> = (0..7).map(|i| line(&format!("l{i}"))).collect();
    store.append_run_log(&id, &more).unwrap();

    let texts = |lines: Vec<RunLogLine>| -> Vec<String> { lines.into_iter().map(|l| l.text).collect() };
    assert_eq!(texts(store.run_log_after(&id, 0, 2).unwrap()), ["a", "b"]);
    assert_eq!(texts(store.run_log_after(&id, 2, 3).unwrap()), ["c", "l0", "l1"]);
    assert!(store.run_log_after(&id, 10, 5).unwrap().is_empty());
    assert_eq!(texts(store.run_log_tail(&id, 3).unwrap()), ["l4", "l5", "l6"]);
    assert_eq!(store.run_log_tail(&id, 100).unwrap().len(), 10);
    // Re-saving the run keeps its log.
    store.upsert_run(&run("r1", None, 0, 1, RunState::Done)).unwrap();
    assert_eq!(store.run_log_tail(&id, 100).unwrap().len(), 10);
}

#[test]
fn renders_accumulate_on_a_run() {
    let store = store();
    let id = RunId::new("r1");
    let render = |path: &str| Render {
        path: path.into(),
        before: Some(BlobId::new("aa")),
        after: BlobId::new("bb"),
        width: 1440,
        height: 900,
    };
    assert!(store.add_render(&id, &render("a.html")).is_err());
    assert!(store.renders(&id).unwrap().is_empty());
    store.upsert_run(&run("r1", None, 0, 1, RunState::Running)).unwrap();
    store.add_render(&id, &render("a.html")).unwrap();
    store.add_render(&id, &render("b.html")).unwrap();
    store.upsert_run(&run("r1", None, 0, 1, RunState::Running)).unwrap();
    assert_eq!(store.renders(&id).unwrap(), vec![render("a.html"), render("b.html")]);
}

fn review(id: &str, run: &str, state: ReviewState, created: i64) -> Review {
    Review {
        id: ReviewId::new(id),
        run: RunId::new(run),
        node: NodeId::new("review"),
        issue: Some(IssueKey::new("HAR-1")),
        title: "Pick a fix".into(),
        state,
        created,
        provenance: vec!["Issues".into(), "Fixer ×3".into()],
        agent: "Fixer".into(),
        variants: vec![Variant { renders: Vec::new(),
            index: 1,
            name: "A".into(),
            approach: "reuse key".into(),
            branch: Some("fix/a".into()),
            worktree: Some(PathBuf::from("/w/a")),
            session: Some(SessionId::new("s")),
            added: 3,
            removed: 1,
            files: vec![FileChange { path: "a.rs".into(), added: 3, removed: 1 }],
            tests: Some(TestResult { passed: 4, total: 5 }),
            tests_ok: None,
            secs: 30,
            cost_usd: 0.2,
        }],
        renders: Vec::new(),
        feedback: vec![ReviewNote { at: 5, text: "smaller".into() }],
        picked: None,
    }
}

#[test]
fn reviews_round_trip_and_list_open_ones() {
    let store = store();
    let mut a = review("v1", "r1", ReviewState::Open, 20);
    store.upsert_review(&a).unwrap();
    store.upsert_review(&review("v2", "r1", ReviewState::Open, 10)).unwrap();
    store.upsert_review(&review("v3", "r2", ReviewState::Shipped, 5)).unwrap();
    assert_eq!(store.review(&a.id).unwrap(), Some(a.clone()));
    let ids = |reviews: Vec<Review>| -> Vec<String> { reviews.into_iter().map(|r| r.id.0).collect() };
    assert_eq!(ids(store.open_reviews().unwrap()), ["v2", "v1"]);
    assert_eq!(ids(store.reviews_for_run(&RunId::new("r1")).unwrap()), ["v2", "v1"]);
    a.state = ReviewState::SentBack;
    a.picked = Some(1);
    store.upsert_review(&a).unwrap();
    assert_eq!(store.review(&a.id).unwrap(), Some(a));
    assert_eq!(ids(store.open_reviews().unwrap()), ["v2"]);
}

fn item(id: &str, kind: InboxKind, created: i64) -> InboxItem {
    InboxItem {
        id: InboxId::new(id),
        kind,
        title: "Title".into(),
        summary: "**done**".into(),
        outcome: Some(Outcome::NeedsReview),
        source: "Fixer · 3 variants".into(),
        issue: Some(IssueKey::new("HAR-1")),
        session: Some(SessionId::new("s")),
        run: Some(RunId::new("r")),
        links: vec![Link { label: "PR".into(), url: "https://x".into() }],
        created,
        read: false,
        archived: false,
    }
}

#[test]
fn inbox_items_round_trip_list_and_change() {
    let store = store();
    let kinds = [
        InboxKind::Review { review: ReviewId::new("v1") },
        InboxKind::Report,
        InboxKind::Question { request: RequestId::new("q1") },
        InboxKind::Failure,
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        let mut it = item(&format!("i{i}"), kind, i as i64);
        if i == 3 {
            it.outcome = None;
            it.issue = None;
            it.links.clear();
        }
        store.upsert_inbox_item(&it).unwrap();
        assert_eq!(store.inbox_item(&it.id).unwrap(), Some(it));
    }
    let ids = |items: Vec<InboxItem>| -> Vec<String> { items.into_iter().map(|i| i.id.0).collect() };
    assert_eq!(ids(store.inbox(false).unwrap()), ["i3", "i2", "i1", "i0"]);
    assert!(store.set_inbox_read(&InboxId::new("i1"), true).unwrap());
    assert!(store.inbox_item(&InboxId::new("i1")).unwrap().unwrap().read);
    assert!(store.set_inbox_archived(&InboxId::new("i0"), true).unwrap());
    assert_eq!(ids(store.inbox(false).unwrap()), ["i3", "i2", "i1"]);
    assert_eq!(ids(store.inbox(true).unwrap()), ["i0"]);
    assert!(!store.set_inbox_read(&InboxId::new("nope"), true).unwrap());
    assert!(store.delete_inbox_item(&InboxId::new("i3")).unwrap());
    assert_eq!(store.inbox_item(&InboxId::new("i3")).unwrap(), None);
}

#[test]
fn the_waiting_count_matches_the_model() {
    let store = store();
    let kinds = [
        InboxKind::Review { review: ReviewId::new("v") },
        InboxKind::Report,
        InboxKind::Question { request: RequestId::new("q") },
        InboxKind::Failure,
    ];
    let mut expected = 0;
    let mut n = 0;
    for kind in kinds {
        for read in [false, true] {
            for archived in [false, true] {
                let mut it = item(&format!("i{n}"), kind.clone(), n);
                it.read = read;
                it.archived = archived;
                expected += u32::from(it.needs_person());
                store.upsert_inbox_item(&it).unwrap();
                n += 1;
            }
        }
    }
    assert_eq!(expected, 7);
    assert_eq!(store.inbox_needs_person().unwrap(), expected);
    // Reading a report takes it off the count; reading a question does not.
    store.set_inbox_read(&InboxId::new("i4"), true).unwrap();
    assert_eq!(store.inbox_needs_person().unwrap(), expected - 1);
    store.set_inbox_read(&InboxId::new("i8"), true).unwrap();
    assert_eq!(store.inbox_needs_person().unwrap(), expected - 1);
}
