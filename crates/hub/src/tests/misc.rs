//! Search, command failures, the router, and models that unload.

use std::time::Duration;

use gpui::TestAppContext;

use super::{collect, setup};
use crate::api::{
    InboxId, IssueKey, NoticeLevel, PipelineId, ReviewDecision, ReviewId, Run, RunId, SearchResult, SessionId, Target,
};
use crate::models::DEBOUNCE;
use crate::{HubNotice, InboxFocus, LoadState, Overlay, Route};

#[gpui::test]
fn a_newer_search_wins_over_an_older_one(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let search = cx.update(|cx| crate::hub(cx).read(cx).search.clone());
    search.update(cx, |model, cx| model.set_text("retry", cx));
    cx.executor().advance_clock(DEBOUNCE / 2);
    search.update(cx, |model, cx| model.set_text("ledger", cx));
    cx.executor().advance_clock(DEBOUNCE * 2);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = search.read(cx);
        assert_eq!(model.state(), &LoadState::Ready);
        let titles: Vec<&str> = model.result().map_or_else(Vec::new, |r| r.hits.iter().map(|h| h.session.title.as_str()).collect());
        assert_eq!(titles, ["Explain ledger reconciliation"]);
    });

    // A result that arrives for an older query is dropped.
    let stale = search.update(cx, |model, cx| {
        let old = model.generation() - 1;
        let empty = SearchResult { hits: Vec::new(), total: 0, facets: Default::default(), index: Default::default() };
        model.finish(old, Ok(empty), cx)
    });
    assert!(!stale);
    cx.update(|cx| assert_eq!(search.read(cx).result().map(|r| r.hits.len()), Some(1)));
}

#[gpui::test]
fn search_counts_and_highlights(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let search = cx.update(crate::search);
    search.update(cx, |model, cx| model.set_text("retry", cx));
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = search.read(cx);
        let Some(result) = model.result() else { panic!("no result") };
        let fix = result.hits.iter().find(|hit| hit.session.id.as_str() == "chat-1423");
        let Some(fix) = fix else { panic!("chat-1423 not found") };
        assert_eq!(fix.children, 2);
        assert_eq!(fix.title_highlights.len(), 1);
        assert_eq!(fix.title_highlights[0], 21..26);
        assert!(result.facets.waiting >= 1);
    });
}

#[gpui::test]
fn a_failed_command_becomes_a_notice(cx: &mut TestAppContext) {
    let (api, hub) = setup(cx);
    let notices = collect::<_, HubNotice>(cx, &hub);
    api.fail_next("pin_session");
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.pin_session(&SessionId::new("chat-ledger"), true, cx));
    cx.run_until_parked();
    {
        let notices = notices.borrow();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].level, NoticeLevel::Error);
        assert_eq!(notices[0].text.as_ref(), "Could not pin the chat");
    }
    cx.update(|cx| assert_eq!(hub.read(cx).notices().count(), 1));

    // A command with a result yields `None` after a failure and `Some` after a success.
    let failed = cx.update(|cx| commands.fork_session(&SessionId::new("no-such-chat"), None, cx));
    let forked = cx.update(|cx| commands.fork_session(&SessionId::new("chat-ledger"), None, cx));
    cx.run_until_parked();
    let failed = cx.executor().block_test(failed);
    let forked = cx.executor().block_test(forked);
    assert!(failed.is_none());
    assert_eq!(forked.map(|s| s.title), Some("Explain ledger reconciliation (fork)".to_string()));
    assert_eq!(notices.borrow().len(), 2);

    // The engine's own notices arrive the same way.
    api.emit(crate::api::Event::Notice { level: NoticeLevel::Success, text: "Cloned acme/ledger".into() });
    cx.run_until_parked();
    assert_eq!(notices.borrow()[2].text.as_ref(), "Cloned acme/ledger");
}

#[gpui::test]
fn the_router_navigates_goes_back_and_maps_targets(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let router = cx.update(|cx| crate::router(cx));
    let chat = Route::Chat { session: SessionId::new("chat-1423") };
    cx.update(|cx| crate::navigate(chat.clone(), cx));
    cx.update(|cx| crate::navigate(Route::Issues { issue: Some(IssueKey::new("PAY-1427")) }, cx));
    router.update(cx, |router, cx| router.open_overlay(Overlay::Palette, cx));
    cx.update(|cx| {
        assert!(router.read(cx).is_open(Overlay::Palette));
        assert_eq!(crate::hub(cx).read(cx).chat_index.read(cx).open(), None);
    });
    assert!(router.update(cx, |router, cx| router.back(cx)));
    cx.update(|cx| {
        let router = router.read(cx);
        assert_eq!(router.current(), &chat);
        assert!(!router.is_open(Overlay::Palette), "overlays close on navigation");
    });
    assert!(router.update(cx, |router, cx| router.back(cx)));
    assert!(!router.update(cx, |router, cx| router.back(cx)));
    cx.update(|cx| assert_eq!(router.read(cx).current(), &Route::Home));

    let run = |issue: Option<&str>| Run {
        id: RunId::new("r"),
        pipeline: PipelineId::new("pay-triage"),
        version: 14,
        issue: issue.map(IssueKey::new),
        number: 1,
        title: String::new(),
        input: String::new(),
        state: Default::default(),
        test: false,
        started: 0,
        finished: None,
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: None,
        nodes: Vec::new(),
    };
    assert_eq!(Route::for_target(&Target::Session(SessionId::new("s")), None), Some(Route::Chat { session: SessionId::new("s") }));
    assert_eq!(
        Route::for_target(&Target::Review(ReviewId::new("v")), None),
        Some(Route::Inbox { item: Some(InboxFocus::Review(ReviewId::new("v"))) })
    );
    assert_eq!(
        Route::for_target(&Target::Inbox(InboxId::new("i")), None),
        Some(Route::Inbox { item: Some(InboxFocus::Item(InboxId::new("i"))) })
    );
    assert_eq!(Route::for_target(&Target::Run(RunId::new("r")), None), None);
    assert_eq!(
        Route::for_target(&Target::Run(RunId::new("r")), Some(&run(Some("PAY-1")))),
        Some(Route::Issues { issue: Some(IssueKey::new("PAY-1")) })
    );
    assert_eq!(
        Route::for_target(&Target::Run(RunId::new("r")), Some(&run(None))),
        Some(Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None })
    );

    // A Home row with a run the hub has not loaded is looked up first.
    cx.update(|cx| crate::open(Target::Run(RunId::new("run-test-42")), cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(router.read(cx).current(), &Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None });
    });
    cx.update(|cx| crate::open(Target::Run(RunId::new("run-1427")), cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(router.read(cx).current(), &Route::Issues { issue: Some(IssueKey::new("PAY-1427")) }));
}

#[gpui::test]
fn opening_a_chat_lists_its_sub_agents(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let index = cx.update(crate::chat_index);
    cx.update(|cx| crate::navigate(Route::Chat { session: SessionId::new("chat-1423") }, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let index = index.read(cx);
        assert_eq!(index.pinned().len(), 2);
        assert_eq!(index.recent().len(), 6);
        assert_eq!(index.waiting(), 1);
        assert_eq!(index.children().len(), 2);
        assert_eq!(index.open(), Some(&SessionId::new("chat-1423")));
    });
}

#[gpui::test]
fn models_unload_when_no_view_holds_them(cx: &mut TestAppContext) {
    let (api, hub) = setup(cx);
    let id = SessionId::new("chat-1423");
    let first = cx.update(|cx| crate::session(&id, cx));
    let again = cx.update(|cx| crate::session(&id, cx));
    assert_eq!(first.entity_id(), again.entity_id(), "views share one model");
    let review = cx.update(|cx| crate::review(&ReviewId::new("review-1423"), cx));
    let run = cx.update(|cx| crate::run(&RunId::new("run-1427"), cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(hub.read(cx).open_sessions(), 1));

    let weak = first.downgrade();
    drop((first, again, review, run));
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    cx.update(|cx| assert_eq!(hub.read(cx).open_sessions(), 0));

    // Events for it are dropped and the stale entries are cleaned up.
    api.emit(super::agent("chat-1423", crate::api::AgentEvent::TextDelta { text: "late".into() }));
    api.emit(crate::api::Event::RunLog {
        run: RunId::new("run-1427"),
        line: crate::api::RunLogLine { seq: 99, at: 0, node: None, variant: 0, level: Default::default(), text: String::new() },
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let hub = hub.read(cx);
        assert!(hub.sessions.is_empty());
        assert!(hub.run_models.is_empty());
    });
}

#[gpui::test]
fn reviews_and_the_inbox_follow_a_decision(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let inbox = cx.update(crate::inbox);
    let id = ReviewId::new("review-1423");
    let review = cx.update(|cx| crate::review(&id, cx));
    review.update(cx, |model, cx| model.ensure_diff(1, "src/charges/retry.ts", cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(inbox.read(cx).waiting(), 4);
        assert_eq!(inbox.read(cx).reviews().len(), 3);
        let diff = review.read(cx).diff(1, "src/charges/retry.ts").and_then(|d| d.value().cloned());
        assert_eq!(diff.map(|d| (d.added, d.removed)), Some((3, 2)));
    });
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.decide_review(&id, ReviewDecision::Ship { variant: 1 }, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(inbox.read(cx).waiting(), 3);
        assert_eq!(inbox.read(cx).reviews().len(), 2);
        assert_eq!(review.read(cx).review().and_then(|r| r.picked), Some(1));
    });
    assert!(api.recorded_names().contains(&"decide_review"));
}

#[gpui::test]
fn the_router_goes_forward_and_keeps_route_parameters(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let router = cx.update(|cx| crate::router(cx));
    let issue = Route::Issues { issue: Some(IssueKey::new("PAY-1427")) };
    let editor = Route::Editor { pipeline: PipelineId::new("pay-triage"), node: Some("fixer".into()) };
    let notes = super::notifications(cx, &router);
    cx.update(|cx| crate::navigate(issue.clone(), cx));
    cx.update(|cx| crate::navigate(issue.clone(), cx));
    assert_eq!(notes.get(), 1, "navigating to the current route does nothing");
    cx.update(|cx| crate::navigate(editor.clone(), cx));
    cx.update(|cx| assert_eq!(router.read(cx).current().section(), crate::Section::Agents));

    assert!(router.update(cx, |router, cx| router.back(cx)));
    assert!(router.update(cx, |router, cx| router.back(cx)));
    cx.update(|cx| {
        let router = router.read(cx);
        assert_eq!(router.current(), &Route::Home);
        assert!(router.can_go_forward() && !router.can_go_back());
    });
    assert!(router.update(cx, |router, cx| router.forward(cx)));
    cx.update(|cx| assert_eq!(router.read(cx).current(), &issue, "the issue parameter survives"));
    assert!(router.update(cx, |router, cx| router.forward(cx)));
    cx.update(|cx| assert_eq!(router.read(cx).current(), &editor));
    assert!(!router.update(cx, |router, cx| router.forward(cx)));

    // A new navigation drops the forward history; a replacement adds no history.
    assert!(router.update(cx, |router, cx| router.back(cx)));
    cx.update(|cx| crate::navigate(Route::Chats, cx));
    cx.update(|cx| assert!(!router.read(cx).can_go_forward()));
    let other = Route::Issues { issue: Some(IssueKey::new("PAY-1440")) };
    router.update(cx, |router, cx| router.replace(other.clone(), cx));
    assert!(router.update(cx, |router, cx| router.back(cx)));
    cx.update(|cx| assert_eq!(router.read(cx).current(), &issue));

    router.update(cx, |router, cx| router.toggle_overlay(Overlay::Palette, cx));
    cx.update(|cx| assert_eq!(router.read(cx).top_overlay(), Some(Overlay::Palette)));
    router.update(cx, |router, cx| router.toggle_overlay(Overlay::Palette, cx));
    cx.update(|cx| assert_eq!(router.read(cx).top_overlay(), None));
}

#[gpui::test]
fn commands_and_queries_never_run_on_the_calling_thread(cx: &mut TestAppContext) {
    let (api, hub) = setup(cx);
    let notices = collect::<_, HubNotice>(cx, &hub);
    let commands = cx.update(|cx| crate::commands(cx));
    let ledger = SessionId::new("chat-ledger");
    cx.update(|cx| {
        commands.rename_session(&ledger, "Ledger", cx);
        commands.archive_session(&ledger, true, cx);
        // Inside the UI update nothing reached the engine yet: the calls wait for the
        // background executor.
        assert!(api.recorded().is_empty());
    });
    cx.run_until_parked();
    assert_eq!(api.recorded_names(), ["rename_session", "archive_session"]);

    let queries = cx.update(|cx| crate::queries(cx));
    let reads = cx.update(|cx| {
        let rules = queries.permission_rules(cx);
        let log = queries.run_log(&RunId::new("run-1427"), 0, 4, cx);
        let archived = queries.archived_inbox(cx);
        (rules, log, archived)
    });
    let rules = cx.executor().block_test(reads.0);
    let log = cx.executor().block_test(reads.1);
    let archived = cx.executor().block_test(reads.2);
    assert_eq!(rules.map(|rules| rules.len()).ok(), Some(3));
    assert_eq!(log.map(|lines| lines.len()).ok(), Some(4));
    assert!(archived.is_ok_and(|items| items.iter().all(|item| item.archived)));

    // Errors of every kind come back as notices, never as panics.
    for command in ["retry_wake", "start_issue"] {
        api.fail_next(command);
    }
    let key = IssueKey::new("PAY-1440");
    cx.update(|cx| {
        commands.retry_wake(&key, &crate::api::RunnableRef::agent("codex"), cx);
        commands.start_issue(&key, cx);
        commands.delete_issue(&IssueKey::new("PAY-0"), cx);
    });
    cx.run_until_parked();
    let mut texts: Vec<String> = notices.borrow().iter().map(|notice| notice.text.to_string()).collect();
    texts.sort();
    assert_eq!(texts, ["Could not delete the issue", "Could not start the issue", "Could not wake it"]);
    assert!(notices.borrow().iter().all(|notice| notice.detail.is_some()));
}
