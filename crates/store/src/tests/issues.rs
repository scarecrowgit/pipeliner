use std::collections::BTreeSet;
use std::sync::Arc;

use base::paths::Paths;
use model::{
    Actor, Attachment, BlobId, Issue, IssueEvent, IssueEventKind, IssueKey, IssueStatus, IssueType, Priority,
    ProjectId, RunId, RunnableRef, StartTriggers,
};

use super::store;
use crate::Store;

pub(crate) fn issue(key: &str, project: &str, updated: i64) -> Issue {
    Issue {
        key: IssueKey::new(key),
        project: ProjectId::new(project),
        title: format!("Issue {key}"),
        body: String::new(),
        status: IssueStatus::Backlog,
        priority: Priority::Medium,
        kind: IssueType::Story,
        owner: String::new(),
        repo: None,
        created: updated - 10,
        updated,
        filed_by: None,
        runner: None,
        triggers: StartTriggers::default(),
        run: None,
        run_count: 0,
        pr_url: None,
        images: Vec::new(),
    }
}

fn event(kind: IssueEventKind, at: i64) -> IssueEvent {
    IssueEvent { seq: 0, at, actor: Actor::Me, kind }
}

#[test]
fn keys_count_up_per_prefix() {
    let store = store();
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-1");
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-2");
    assert_eq!(store.next_issue_key("PAY").unwrap().as_str(), "PAY-1");
    assert!(store.next_issue_key("").is_err());
    // A stored issue with a higher number (an import) pushes the counter past it.
    store.upsert_issue(&issue("HAR-40", "p", 1)).unwrap();
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-41");
    // A lower one does not pull it back.
    store.upsert_issue(&issue("HAR-3", "p", 1)).unwrap();
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-42");
    // Prefixes that contain a dash split at the last one.
    store.upsert_issue(&issue("WEB-APP-7", "p", 1)).unwrap();
    assert_eq!(store.next_issue_key("WEB-APP").unwrap().as_str(), "WEB-APP-8");
    // Deleting an issue does not free its number.
    assert!(store.delete_issue(&IssueKey::new("HAR-40")).unwrap());
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-43");
}

#[test]
fn eight_threads_never_share_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&Paths::under(dir.path())).unwrap());
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || (0..25).map(|_| store.next_issue_key("HAR").unwrap().0).collect::<Vec<_>>())
        })
        .collect();
    let keys: Vec<String> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    let unique: BTreeSet<&String> = keys.iter().collect();
    assert_eq!(keys.len(), 200);
    assert_eq!(unique.len(), 200);
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-201");
}

#[test]
fn an_issue_round_trips_and_lists_by_project() {
    let store = store();
    let mut full = issue("HAR-1", "harness", 100);
    full.body = "Steps:\n1. open".into();
    full.status = IssueStatus::InReview;
    full.priority = Priority::High;
    full.kind = IssueType::Bug;
    full.owner = "Ivan".into();
    full.repo = Some("pipeliner".into());
    full.filed_by = Some(RunnableRef::agent("triage"));
    full.runner = Some(RunnableRef::pipeline("fix"));
    full.triggers = StartTriggers { on_assign: false, on_comment: true, on_mention: false, on_todo: true };
    full.run = Some(RunId::new("r9"));
    full.run_count = 3;
    full.pr_url = Some("https://github.com/x/y/pull/2".into());
    full.images = vec![Attachment { blob: BlobId::new("ab"), name: "s.png".into(), media_type: "image/png".into(), bytes: 9 }];
    store.upsert_issue(&full).unwrap();
    assert_eq!(store.issue(&full.key).unwrap(), Some(full.clone()));

    store.upsert_issue(&issue("HAR-2", "harness", 300)).unwrap();
    store.upsert_issue(&issue("PAY-1", "payments", 200)).unwrap();
    let keys: Vec<String> = store.issues(&ProjectId::new("harness")).unwrap().into_iter().map(|i| i.key.0).collect();
    assert_eq!(keys, ["HAR-2", "HAR-1"]);
    assert!(store.issues(&ProjectId::new("none")).unwrap().is_empty());
    assert_eq!(store.issue(&IssueKey::new("HAR-99")).unwrap(), None);
}

#[test]
fn the_activity_feed_numbers_entries_and_goes_with_the_issue() {
    let store = store();
    let key = IssueKey::new("HAR-1");
    assert!(store.append_issue_event(&key, event(IssueEventKind::Created, 1)).is_err());
    store.upsert_issue(&issue("HAR-1", "p", 1)).unwrap();
    let comment = IssueEventKind::Comment {
        text: "@fixer please".into(),
        images: Vec::new(),
        mentions: vec![RunnableRef::agent("fixer")],
    };
    let first = store.append_issue_event(&key, event(IssueEventKind::Created, 10)).unwrap();
    let mut by_agent = event(comment, 20);
    by_agent.actor = Actor::Runnable { reference: RunnableRef::agent("lead"), name: "Lead".into(), spawned: true, session: None };
    let second = store.append_issue_event(&key, by_agent.clone()).unwrap();
    let third = store
        .append_issue_event(&key, event(IssueEventKind::StatusChanged { from: IssueStatus::Todo, to: IssueStatus::Done }, 30))
        .unwrap();
    assert_eq!((first.seq, second.seq, third.seq), (1, 2, 3));
    assert_eq!(second, IssueEvent { seq: 2, ..by_agent });
    assert_eq!(store.issue_events(&key).unwrap(), vec![first, second, third]);

    // Re-saving the issue keeps its feed; deleting it removes the feed.
    store.upsert_issue(&issue("HAR-1", "p", 2)).unwrap();
    assert_eq!(store.issue_events(&key).unwrap().len(), 3);
    assert!(store.delete_issue(&key).unwrap());
    assert!(store.issue_events(&key).unwrap().is_empty());
    let kinds: Vec<String> = store
        .db
        .read()
        .prepare("SELECT event FROM issue_events")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(kinds.is_empty());
}

#[test]
fn feed_rows_record_their_kind() {
    let store = store();
    store.upsert_issue(&issue("HAR-1", "p", 1)).unwrap();
    store
        .append_issue_event(&IssueKey::new("HAR-1"), event(IssueEventKind::PrOpened { url: "u".into() }, 1))
        .unwrap();
    let kind: String = store.db.read().query_row("SELECT event FROM issue_events", [], |r| r.get(0)).unwrap();
    assert_eq!(kind, "pr_opened");
}

#[test]
fn raising_the_counter_only_ever_moves_it_up() {
    let store = store();
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-1");
    store.raise_issue_counter("HAR", 1_000_000_000_000).unwrap();
    store.raise_issue_counter("HAR", 5).unwrap();
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-1000000000001");
    assert!(store.raise_issue_counter("", 3).is_err());
}
