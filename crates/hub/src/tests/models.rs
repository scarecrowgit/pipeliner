//! Runs, inbox, catalog, ops, permissions and issues models: the first load, then the events
//! (and commands) that change them.

use std::time::Duration;

use gpui::TestAppContext;

use super::{notifications, setup};
use crate::api::{
    Event, InboxId, IssueKey, PermissionDecision, Period, PipelineId, ProjectId, RequestId, RunInput, RunState,
    RunnableRef, Runtime, RuntimeKind, SessionId, SessionState,
};
use crate::{LoadState, fixtures::SCENARIOS};

#[gpui::test]
fn runs_load_the_latest_per_issue_and_follow_changes(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let runs = cx.update(crate::runs);
    assert_eq!(cx.update(|cx| runs.read(cx).state().clone()), LoadState::Loading);
    cx.run_until_parked();
    cx.update(|cx| {
        let runs = runs.read(cx);
        assert_eq!(runs.state(), &LoadState::Ready);
        assert_eq!(runs.latest_for(&IssueKey::new("PAY-1427")).map(|run| run.id.as_str()), Some("run-1427"));
        assert!(runs.open_without_issue().is_empty(), "the test run is finished");
        assert!(runs.latest_for(&IssueKey::new("PAY-1431")).is_some());
    });

    let mut run = api.with_world(|world| world.runs.iter().find(|r| r.id.as_str() == "run-1427").cloned()).expect("run-1427");
    run.state = RunState::Done;
    api.emit(Event::RunChanged { run });
    cx.run_until_parked();
    cx.update(|cx| {
        let latest = runs.read(cx).latest_for(&IssueKey::new("PAY-1427")).map(|run| run.state);
        assert_eq!(latest, Some(RunState::Done));
    });

    let commands = cx.update(|cx| crate::commands(cx));
    let started = cx.update(|cx| commands.start_run(&PipelineId::new("pay-triage"), RunInput::default(), cx));
    cx.run_until_parked();
    let Some(started) = cx.executor().block_test(started) else { panic!("start_run failed") };
    assert_eq!(cx.update(|cx| runs.read(cx).open_without_issue().len()), 1);
    cx.update(|cx| assert!(runs.read(cx).find(&started.id).is_some()));
    cx.update(|cx| commands.cancel_run(&started.id, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let runs = runs.read(cx);
        assert!(runs.open_without_issue().is_empty(), "a closed manual run leaves the open list");
        assert!(runs.open_runs().iter().all(|run| run.state.is_open()));
    });
}

#[gpui::test]
fn the_inbox_loads_items_and_reviews_and_follows_events(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let inbox = cx.update(crate::inbox);
    cx.run_until_parked();
    let (items, waiting) = cx.update(|cx| (inbox.read(cx).items().len(), inbox.read(cx).waiting()));
    assert!(items >= waiting && waiting == 4);
    let id = InboxId::new("inbox-1419");
    cx.update(|cx| assert!(inbox.read(cx).item(&id).is_some()));

    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.set_inbox_read(&id, true, cx));
    cx.run_until_parked();
    cx.update(|cx| assert!(inbox.read(cx).item(&id).is_some_and(|item| item.read)));
    cx.update(|cx| commands.archive_inbox(&id, true, cx));
    cx.run_until_parked();
    cx.update(|cx| assert!(inbox.read(cx).item(&id).is_none(), "archived items leave the list"));

    api.emit(Event::InboxRemoved { id: InboxId::new("inbox-1427") });
    cx.run_until_parked();
    cx.update(|cx| {
        let inbox = inbox.read(cx);
        assert_eq!(inbox.items().len(), items - 2);
        assert!(inbox.items().windows(2).all(|pair| pair[0].created >= pair[1].created), "newest first");
        assert!(inbox.waiting_items().windows(2).all(|pair| pair[0].created >= pair[1].created), "queue newest first");
    });
}

#[gpui::test]
fn the_catalog_reloads_what_was_read_when_the_library_changes(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let catalog = cx.update(crate::catalog);
    let triage = PipelineId::new("pay-triage");
    catalog.update(cx, |model, cx| {
        model.ensure_pipeline(&triage, cx);
        model.ensure_versions(&triage, cx);
    });
    cx.run_until_parked();
    let lead = RunnableRef::orchestrator("payments-lead");
    let versions = cx.update(|cx| {
        let model = catalog.read(cx);
        assert_eq!(model.state(), &LoadState::Ready);
        assert!(model.runnable(&lead).is_some_and(|r| !r.paused));
        assert!(model.activity(&lead).is_some());
        assert!(model.skills().value().is_some_and(|skills| !skills.is_empty()));
        assert!(model.problems().is_ready());
        assert!(model.of_kind(crate::api::RunnableKind::Pipeline).count() >= 3);
        assert!(model.pipeline(&triage).and_then(|p| p.value()).is_some_and(|graph| graph.is_some()));
        model.versions(&triage).and_then(|v| v.value().map(|v| v.len())).unwrap_or_default()
    });
    assert!(versions > 0);

    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.set_runnable_paused(&lead, true, cx));
    let deployed = cx.update(|cx| commands.deploy_pipeline(&triage, "variants 3", cx));
    cx.run_until_parked();
    assert!(cx.executor().block_test(deployed).is_some());
    cx.update(|cx| {
        let model = catalog.read(cx);
        assert!(model.runnable(&lead).is_some_and(|r| r.paused));
        assert!(model.activity(&lead).is_some_and(|a| a.state == crate::api::RunnableState::Paused));
        assert_eq!(model.versions(&triage).and_then(|v| v.value().map(|v| v.len())), Some(versions + 1));
    });
}

#[gpui::test]
fn ops_loads_home_and_follows_live_changes(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let ops = cx.update(crate::ops);
    cx.run_until_parked();
    cx.update(|cx| {
        let ops = ops.read(cx);
        assert!(ops.settings().is_ready() && ops.projects().is_ready());
        assert_eq!(ops.current_project().map(|p| p.id.clone()), Some(ProjectId::new("payments")));
        assert!(ops.needs().value().is_some_and(|needs| !needs.is_empty()));
        assert!(ops.working().value().is_some_and(|working| !working.is_empty()));
        assert!(ops.spend_today().is_ready());
        assert_eq!(ops.runtimes().value().map(|r| r.len()), Some(3));
        assert!(ops.stats(Period::Today).is_some_and(|s| s.is_ready()));
        assert!(ops.spend_buckets(Period::Today).is_some_and(|s| s.is_ready()));
        assert_eq!(ops.spend_by_runnable(Period::Today).and_then(|s| s.value().map(|rows| rows.len())), Some(7));
        assert!(ops.stats(Period::Days30).is_none(), "other periods load when asked");
        assert!(!ops.autopilots().started());
    });
    ops.update(cx, |ops, cx| {
        ops.ensure_period(Period::Days30, cx);
        ops.ensure_autopilots(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let ops = ops.read(cx);
        assert!(ops.stats(Period::Days30).is_some_and(|s| s.value().is_some_and(|s| s.runs == 1_520)));
        assert!(ops.autopilots().value().is_some_and(|a| !a.is_empty()));
    });

    // A session change re-reads needs, working and every loaded period's stats: one notify.
    let ops_notes = notifications(cx, &ops);
    api.with_world(|world| world.ops.needs.clear());
    let mut session = api.with_world(|world| world.session(&SessionId::new("chat-1423")).cloned()).expect("chat-1423");
    session.state = SessionState::Idle;
    api.emit(Event::SessionChanged { session });
    cx.run_until_parked();
    assert_eq!(ops_notes.get(), 1);
    cx.update(|cx| assert!(ops.read(cx).needs().value().is_some_and(|needs| needs.is_empty())));

    // A runtime update replaces the one with its name.
    let laptop = Runtime {
        name: "laptop".into(),
        kind: RuntimeKind::Remote,
        available: true,
        online: true,
        slots_total: 2,
        slots_busy: 1,
        note: "back online".into(),
    };
    api.emit(Event::RuntimeChanged { runtime: laptop });
    cx.run_until_parked();
    cx.update(|cx| {
        let runtimes = ops.read(cx).runtimes().value().cloned().unwrap_or_default();
        assert_eq!(runtimes.len(), 3);
        assert!(runtimes.iter().any(|r| r.name == "laptop" && r.online));
    });

    // Settings saved through a command come back through the event.
    let commands = cx.update(|cx| crate::commands(cx));
    let mut settings = api.with_world(|world| world.settings.clone());
    settings.slots = 5;
    cx.update(|cx| commands.set_settings(settings, cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(ops.read(cx).settings().value().map(|s| s.slots), Some(5)));

    // The clock re-reads time-relative numbers without any event.
    api.with_world(|world| world.ops.spend_today.cost_usd = 9.99);
    cx.executor().advance_clock(Duration::from_secs(61));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(ops.read(cx).spend_today().value().map(|s| s.cost_usd), Some(9.99)));
}

#[gpui::test]
fn permissions_load_from_waiting_sessions_and_resolve(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let permissions = cx.update(crate::permissions);
    cx.run_until_parked();
    let id = RequestId::new("perm-1423-bun-test");
    cx.update(|cx| {
        let model = permissions.read(cx);
        assert_eq!(model.count(), 1);
        assert_eq!(model.session_of(&id), Some(&SessionId::new("chat-1423")));
        assert_eq!(model.for_session(&SessionId::new("chat-1423")).count(), 1);
    });
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.answer_permission(&id, PermissionDecision::Reject { note: "not now".into() }, cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(permissions.read(cx).count(), 0));
}

#[gpui::test]
fn issues_follow_the_current_project(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let issues = cx.update(crate::issues);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = issues.read(cx);
        assert_eq!(model.project(), Some(&ProjectId::new("payments")));
        assert_eq!(model.len(), 10);
        assert_eq!(model.open_count(), 8);
    });

    // An issue of another project is not shown.
    let mut other = api.with_world(|world| world.issues[0].clone());
    other.key = IssueKey::new("CHK-7");
    other.project = ProjectId::new("checkout");
    api.emit(Event::IssueChanged { issue: other });
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(issues.read(cx).len(), 10));

    let commands = cx.update(|cx| crate::commands(cx));
    let mut settings = api.with_world(|world| world.settings.clone());
    settings.current_project = Some(ProjectId::new("checkout"));
    cx.update(|cx| commands.set_settings(settings, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = issues.read(cx);
        assert_eq!(model.project(), Some(&ProjectId::new("checkout")));
        assert!(model.is_empty());
    });

    // A view that picks a project stops following the settings.
    issues.update(cx, |model, cx| model.set_project(ProjectId::new("payments"), cx));
    cx.run_until_parked();
    let mut settings = api.with_world(|world| world.settings.clone());
    settings.current_project = Some(ProjectId::new("data"));
    cx.update(|cx| commands.set_settings(settings, cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(issues.read(cx).project(), Some(&ProjectId::new("payments"))));
}

#[gpui::test]
fn every_scenario_feeds_every_model(cx: &mut TestAppContext) {
    for name in SCENARIOS {
        let Some(api) = crate::fixtures::FixtureApi::scenario(name) else { panic!("{name}") };
        let api = std::sync::Arc::new(api);
        cx.update(|cx| crate::Hub::new(api.clone(), cx));
        let models = cx.update(|cx| {
            (
                crate::chat_index(cx),
                crate::issues(cx),
                crate::runs(cx),
                crate::inbox(cx),
                crate::catalog(cx),
                crate::ops(cx),
                crate::permissions(cx),
            )
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(models.0.read(cx).state(), &LoadState::Ready, "{name}: chats");
            assert_eq!(models.1.read(cx).state(), &LoadState::Ready, "{name}: issues");
            assert_eq!(models.2.read(cx).state(), &LoadState::Ready, "{name}: runs");
            assert_eq!(models.3.read(cx).state(), &LoadState::Ready, "{name}: inbox");
            assert_eq!(models.4.read(cx).state(), &LoadState::Ready, "{name}: catalog");
            assert!(models.5.read(cx).settings().is_ready(), "{name}: ops");
            assert_eq!(models.6.read(cx).state(), &LoadState::Ready, "{name}: permissions");
        });
    }
}
