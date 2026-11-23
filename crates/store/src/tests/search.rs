use model::{IssueKey, RunId, RunnableRef, SessionId, SessionOrigin, SessionState};

use super::{session, store, stored, workdir};
use crate::search::{fts_query, split_marks};
use crate::{SessionQuery, StartedBy, Store};

/// Six sessions:
/// - `pay`: mine, payments-api, waiting, pinned, PR, issue; title and messages about retry.
/// - `budget`: sub-agent of `pay`, payments-api, running, from an issue; "retry" in a message.
/// - `ru`: mine, Cyrillic text, failed.
/// - `node`: a pipeline node's session, web repo, done, "retry" in the title.
/// - `old`: archived, mentions retry.
/// - `other`: mine, nothing about retry.
fn corpus() -> Store {
    let store = store();
    let mut pay = session("pay", "Fix double charge on retry", 6_000);
    pay.workdir = Some(workdir("payments-api"));
    pay.state = SessionState::Waiting;
    pay.pinned = true;
    pay.pr_url = Some("https://github.com/acme/pay/pull/1".into());
    pay.issue = Some(IssueKey::new("PAY-1423"));
    store.upsert_session(&pay).unwrap();

    let mut budget = session("budget", "Split plan", 5_000);
    budget.workdir = Some(workdir("payments-api"));
    budget.parent = Some(pay.id.clone());
    budget.depth = 1;
    budget.origin = SessionOrigin::Agent { parent: pay.id.clone() };
    budget.state = SessionState::Running;
    budget.issue = Some(IssueKey::new("PAY-1436"));
    budget.runnable = RunnableRef::agent("lead");
    store.upsert_session(&budget).unwrap();

    let mut ru = session("ru", "Исправить двойное списание", 4_000);
    ru.state = SessionState::Failed;
    store.upsert_session(&ru).unwrap();

    let mut node = session("node", "Retry banner copy", 3_000);
    node.workdir = Some(workdir("web"));
    node.origin = SessionOrigin::Run { run: RunId::new("r1"), node: "fixer".into(), variant: 1 };
    node.state = SessionState::Done;
    store.upsert_session(&node).unwrap();

    let mut old = session("old", "Old thread", 2_000);
    old.archived = true;
    store.upsert_session(&old).unwrap();

    store.upsert_session(&session("other", "Bump gateway sdk", 1_000)).unwrap();

    let say = |id: &str, text: &str, at: i64| {
        store.append_message(&SessionId::new(id), stored(text, at)).unwrap();
    };
    say("pay", "Fix the double charge on retry in src/charges/retry.ts. Add a test.", 100);
    say("pay", "Retries now reuse one idempotency key, so the gateway drops the duplicate charge.", 200);
    say("pay", "Wants to run bun test src/charges.", 300);
    say("budget", "Retry budget per merchant: split the plan into two steps.", 100);
    say("ru", "Повторное СПИСАНИЕ при ретрае: нужен ключ идемпотентности.", 100);
    say("old", "We tried retry with backoff and gave up.", 100);
    say("other", "Bump the gateway SDK to 4.2 (\"breaking\" changes: none).", 100);
    store
}

fn ids(store: &Store, query: SessionQuery) -> Vec<String> {
    store.search_sessions(&query).unwrap().hits.into_iter().map(|hit| hit.session.id.0).collect()
}

fn text(text: &str) -> SessionQuery {
    SessionQuery { text: text.into(), ..Default::default() }
}

#[test]
fn fts_queries_quote_every_word_and_prefix_the_last() {
    assert_eq!(fts_query("retry"), Some("\"retry\"*".into()));
    assert_eq!(fts_query("fix retry "), Some("\"fix\" \"retry\"".into()));
    assert_eq!(fts_query("say \"hi"), Some("\"say\" \"\"\"hi\"*".into()));
    assert_eq!(fts_query("  -- () * "), None);
    assert_eq!(fts_query(""), None);
}

#[test]
fn marks_split_into_text_and_byte_ranges() {
    let (plain, ranges) = split_marks("a \u{E000}ретрай\u{E001} b \u{E000}x\u{E001}");
    assert_eq!(plain, "a ретрай b x");
    assert_eq!(&plain[ranges[0].clone()], "ретрай");
    assert_eq!(&plain[ranges[1].clone()], "x");
}

#[test]
fn text_matches_titles_and_messages_newest_first() {
    let store = corpus();
    assert_eq!(ids(&store, text("retry")), ["pay", "budget", "node"]);
    assert_eq!(ids(&store, text("retr")), ["pay", "budget", "node"], "the last word is a prefix");
    assert_eq!(ids(&store, text("retr ")), Vec::<String>::new(), "a finished word is not a prefix");
    assert_eq!(ids(&store, text("idempotency key")), ["pay"]);
    assert_eq!(ids(&store, text("gateway")), ["pay", "other"]);
    assert_eq!(ids(&store, text("banner")), ["node"], "titles are searched");
    assert_eq!(ids(&store, text("RETRY")), ["pay", "budget", "node"]);
    assert_eq!(ids(&store, text("")).len(), 5, "no text lists every unarchived session");
}

#[test]
fn hits_carry_the_best_snippet_and_title_highlights() {
    let store = corpus();
    let result = store.search_sessions(&text("idempotency")).unwrap();
    let hit = &result.hits[0];
    let snippet = hit.snippet.as_ref().unwrap();
    assert_eq!(snippet.seq, 2);
    assert!(!snippet.text.contains('\u{E000}'));
    assert_eq!(snippet.highlights.len(), 1);
    assert_eq!(&snippet.text[snippet.highlights[0].clone()], "idempotency");

    let result = store.search_sessions(&text("retry")).unwrap();
    let pay = &result.hits[0];
    assert_eq!(pay.children, 1);
    assert_eq!(pay.title_highlights.len(), 1);
    assert_eq!(&pay.session.title[pay.title_highlights[0].clone()], "retry");
    let snippet = pay.snippet.as_ref().unwrap();
    assert!(snippet.highlights.iter().all(|r| snippet.text[r.clone()].to_lowercase().starts_with("retr")));
    // A title-only match has no message snippet.
    let node = result.hits.iter().find(|hit| hit.session.id.0 == "node").unwrap();
    assert!(node.snippet.is_none());
    assert_eq!(&node.session.title[node.title_highlights[0].clone()], "Retry");
}

#[test]
fn hostile_queries_are_taken_literally() {
    let store = corpus();
    for query in [
        "\"", "\"retry", "retry\"", "retry AND", "AND", "OR retry", "NOT", "NEAR(retry gateway)", "retry*", "*",
        "^retry", "text:retry", "-retry", "(retry", "retry)", "'", "retry.ts", "src/charges/retry.ts", "{a b}",
        "a + b", "\\", "%", "_", "4.2", "\"breaking\"", "😀", "retry ;DROP TABLE sessions;--",
    ] {
        let result = store.search_sessions(&text(query));
        assert!(result.is_ok(), "{query:?}: {:?}", result.err());
    }
    assert_eq!(ids(&store, text("src/charges/retry.ts")), ["pay"]);
    assert_eq!(ids(&store, text("OR")), Vec::<String>::new(), "operators are words");
    assert_eq!(ids(&store, text("\"breaking\"")), ["other"]);
    assert_eq!(ids(&store, text("4.2")), ["other"]);
    // Only punctuation: nothing to search for, so nothing is filtered out.
    assert_eq!(ids(&store, text("\" * ()")).len(), 5);
    assert_eq!(store.index_stats().unwrap().sessions, 6, "the tables survived");
}

#[test]
fn cyrillic_matches_with_case_folding_and_prefixes() {
    let store = corpus();
    assert_eq!(ids(&store, text("списание")), ["ru"]);
    assert_eq!(ids(&store, text("СПИС")), ["ru"]);
    assert_eq!(ids(&store, text("двойное")), ["ru"], "titles too");
    assert_eq!(ids(&store, text("идемпотентности")), ["ru"]);
    let result = store.search_sessions(&text("ретра")).unwrap();
    let snippet = result.hits[0].snippet.as_ref().unwrap();
    assert_eq!(&snippet.text[snippet.highlights[0].clone()], "ретрае");
}

#[test]
fn filters_narrow_the_scope_and_the_chips() {
    let store = corpus();
    let q = |f: fn(&mut SessionQuery)| {
        let mut query = text("retry");
        f(&mut query);
        ids(&store, query)
    };
    assert_eq!(q(|q| q.repo = Some("payments-api".into())), ["pay", "budget"]);
    assert_eq!(q(|q| q.runnable = Some(RunnableRef::agent("lead"))), ["budget"]);
    assert_eq!(q(|q| q.pinned = Some(true)), ["pay"]);
    assert_eq!(q(|q| q.archived = Some(true)), ["old"]);
    assert_eq!(q(|q| q.archived = None), ["pay", "budget", "node", "old"]);
    assert_eq!(q(|q| q.state = Some(SessionState::Running)), ["budget"]);
    assert_eq!(q(|q| q.started_by = Some(StartedBy::Me)), ["pay"]);
    assert_eq!(q(|q| q.started_by = Some(StartedBy::Agents)), ["budget", "node"]);
    assert_eq!(q(|q| q.has_pr = true), ["pay"]);
    assert_eq!(q(|q| q.from_issue = true), ["pay", "budget"]);
    assert_eq!(q(|q| q.limit = 1), ["pay"]);
    assert_eq!(q(|q| { q.limit = 1; q.offset = 1 }), ["budget"]);
}

#[test]
fn totals_and_facets_follow_the_text_not_the_chips() {
    let store = corpus();
    let result = store
        .search_sessions(&SessionQuery { text: "retry".into(), started_by: Some(StartedBy::Agents), limit: 1, ..Default::default() })
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.total, 2, "matches of the whole query, past the limit");
    let f = result.facets;
    assert_eq!((f.all, f.mine, f.started_by_agents), (3, 1, 2));
    assert_eq!((f.waiting, f.running, f.failed), (1, 1, 0));
    assert_eq!((f.has_pr, f.from_issue), (1, 2));

    let all = store.search_sessions(&SessionQuery::default()).unwrap();
    assert_eq!(all.total, 5);
    let f = all.facets;
    assert_eq!((f.all, f.mine, f.started_by_agents), (5, 3, 2));
    assert_eq!((f.waiting, f.running, f.failed, f.has_pr, f.from_issue), (1, 1, 1, 1, 2));
}

#[test]
fn index_statistics_count_everything() {
    let store = corpus();
    let stats = store.search_sessions(&text("nothing-matches-this")).unwrap().index;
    assert_eq!(stats.sessions, 6);
    assert_eq!(stats.messages, 7);
    assert_eq!(stats.updated, Some(6_000));
    assert_eq!(store.index_stats().unwrap(), stats);
    assert_eq!(store_empty_stats(), crate::IndexStats { sessions: 0, messages: 0, updated: None });
}

fn store_empty_stats() -> crate::IndexStats {
    store().index_stats().unwrap()
}

#[test]
fn renaming_a_session_reindexes_its_title() {
    let store = corpus();
    let mut other = store.session(&SessionId::new("other")).unwrap().unwrap();
    other.title = "Zeppelin launch".into();
    store.upsert_session(&other).unwrap();
    assert_eq!(ids(&store, text("zeppelin")), ["other"]);
    assert_eq!(ids(&store, text("bump")), ["other"], "still found by its message");
    assert_eq!(ids(&store, text("sdk")), ["other"]);
    other.title = "Plain".into();
    store.upsert_session(&other).unwrap();
    assert!(ids(&store, text("zeppelin")).is_empty());
}
