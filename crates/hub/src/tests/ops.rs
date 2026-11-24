//! The operations contract: why runs wait, a pipeline's runs, the sidebar's own chats, saves
//! that report their result, settings and projects.

use gpui::TestAppContext;

use super::setup;
use crate::api::{
    Event, NewProject, NodeId, PipelineId, Place, RunId, RunState, RunWait, SessionId, SessionOrigin,
};

#[gpui::test]
fn runs_know_why_they_wait_and_follow_the_engine(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let runs = cx.update(crate::runs);
    let run = cx.update(|cx| crate::run(&RunId::new("run-1423"), cx));
    cx.update(|cx| run.update(cx, |model, cx| model.ensure_loaded(cx)));
    cx.run_until_parked();
    let id = RunId::new("run-1423");
    cx.update(|cx| {
        assert_eq!(runs.read(cx).waiting(&id), Some(RunWait::Review));
        assert_eq!(run.read(cx).waiting(), Some(RunWait::Review));
        assert_eq!(runs.read(cx).waiting(&RunId::new("run-1427")), None);
    });
    api.emit(Event::RunWaiting { run: id.clone(), waiting: Some(RunWait::Budget) });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(runs.read(cx).waiting(&id), Some(RunWait::Budget));
        assert_eq!(run.read(cx).waiting(), Some(RunWait::Budget));
    });
    let mut done = api.with_world(|world| world.runs.iter().find(|r| r.id == id).cloned()).expect("run");
    done.state = RunState::Done;
    api.emit(Event::RunChanged { run: done });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(runs.read(cx).waiting(&id), None, "a closed run no longer waits");
        assert_eq!(run.read(cx).waiting(), None);
    });
}

#[gpui::test]
fn a_pipelines_runs_load_on_demand_and_follow_new_runs(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let runs = cx.update(crate::runs);
    let triage = PipelineId::new("pay-triage");
    cx.update(|cx| runs.update(cx, |model, cx| model.ensure_pipeline_runs(&triage, cx)));
    cx.run_until_parked();
    let before = cx.update(|cx| runs.read(cx).pipeline_runs(&triage).and_then(|l| l.value().map(|v| v.len())));
    assert!(before.is_some_and(|n| n > 0));
    let commands = cx.update(|cx| crate::commands(cx));
    let started = cx.update(|cx| commands.test_run(&triage, Default::default(), cx));
    cx.run_until_parked();
    let started = cx.executor().block_test(started).expect("test run");
    cx.update(|cx| {
        let model = runs.read(cx);
        let list = model.pipeline_runs(&triage).and_then(|l| l.value()).expect("loaded");
        assert_eq!(list.first().map(|r| r.id.clone()), Some(started.id.clone()), "newest first");
        assert_eq!(model.last_test_run(&triage).map(|r| r.id.clone()), Some(started.id.clone()));
    });
    let queried = cx.update(|cx| crate::queries(cx).runs_for_pipeline(&triage, 1, cx));
    cx.run_until_parked();
    assert_eq!(cx.executor().block_test(queried).expect("runs").len(), 1);
}

#[gpui::test]
fn the_sidebar_lists_only_the_persons_own_chats(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let index = cx.update(crate::chat_index);
    cx.run_until_parked();
    let mut node = api.with_world(|world| world.sessions.iter().find(|s| s.parent.is_none()).cloned()).expect("chat");
    node.id = SessionId::new("pipeline-node");
    node.origin = SessionOrigin::Run { run: RunId::new("run-1427"), node: NodeId::new("repro"), variant: 0 };
    api.with_world(|world| world.sessions.push(node.clone()));
    api.emit(Event::SessionChanged { session: node.clone() });
    cx.run_until_parked();
    cx.update(|cx| {
        let index = index.read(cx);
        assert!(index.session(&node.id).is_none(), "a pipeline session is not a chat");
        assert!(index.pinned().iter().chain(index.recent()).all(|s| s.started_by_person()));
    });
    cx.update(|cx| index.update(cx, |model, cx| model.reload(cx)));
    cx.run_until_parked();
    cx.update(|cx| assert!(index.read(cx).session(&node.id).is_none(), "not after a reload either"));
}

#[gpui::test]
fn saves_report_their_result_and_settings_change_one_by_one(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let commands = cx.update(|cx| crate::commands(cx));
    let graph = api.with_world(|world| world.pipelines.first().cloned()).expect("a pipeline");
    let saved = cx.update(|cx| commands.save_pipeline_with_result(Place::Library, graph.clone(), cx));
    cx.run_until_parked();
    assert_eq!(cx.executor().block_test(saved), Some(()));
    api.fail_next("save_pipeline");
    let failed = cx.update(|cx| commands.save_pipeline_with_result(Place::Library, graph, cx));
    cx.run_until_parked();
    assert_eq!(cx.executor().block_test(failed), None);

    cx.update(|cx| {
        commands.set_daily_limit(Some(9.0), cx);
        commands.set_slots(3, cx);
        commands.set_worktree_retention(14, cx);
    });
    cx.run_until_parked();
    let settings = api.with_world(|world| world.settings.clone());
    assert_eq!((settings.daily_limit_usd, settings.slots, settings.worktree_retention_days), (Some(9.0), 3, 14));
    let created = cx.update(|cx| commands.create_project(NewProject { name: "Ops".into(), key_prefix: "ops".into(), repos: Vec::new() }, cx));
    cx.run_until_parked();
    let project = cx.executor().block_test(created).expect("created");
    assert_eq!(project.key_prefix, "OPS");
    cx.update(|cx| commands.set_current_project(&project.id, cx));
    cx.run_until_parked();
    assert_eq!(api.with_world(|world| world.settings.current_project.clone()), Some(project.id.clone()));
}

#[gpui::test]
fn the_project_switcher_counts_and_scheduled_pipelines_follow_events(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let ops = cx.update(crate::ops);
    cx.update(|cx| {
        ops.update(cx, |ops, cx| {
            ops.ensure_project_counts(cx);
            ops.ensure_schedules(cx);
        })
    });
    cx.run_until_parked();
    let (project, issue) = api.with_world(|world| {
        let issue = world.issues.iter().find(|i| i.status != crate::api::IssueStatus::Done).cloned().expect("an open issue");
        (issue.project.clone(), issue)
    });
    let before = cx.update(|cx| ops.read(cx).counts_for(&project).cloned()).expect("counts");
    assert!(before.open_issues > 0);
    assert!(before.working_runs >= 1, "run-1427 works on a PAY issue: {before:?}");
    let mut done = issue.clone();
    done.status = crate::api::IssueStatus::Done;
    api.with_world(|world| {
        if let Some(known) = world.issues.iter_mut().find(|i| i.key == done.key) {
            *known = done.clone();
        }
    });
    api.emit(Event::IssueChanged { issue: done });
    cx.run_until_parked();
    let after = cx.update(|cx| ops.read(cx).counts_for(&project).cloned()).expect("counts");
    assert_eq!(after.open_issues, before.open_issues - 1);

    let triage = crate::api::RunnableRef::pipeline("pay-triage");
    let at = cx.update(|cx| ops.read(cx).next_scheduled_run(&triage));
    assert_eq!(at, Some(super::NOW + 5 * 3_600_000 + 40 * 60_000));
    assert_eq!(cx.update(|cx| ops.read(cx).next_scheduled_run(&crate::api::RunnableRef::agent("codex"))), None, "switched off");
    api.with_world(|world| world.autopilots.iter_mut().for_each(|a| a.enabled = false));
    api.emit(Event::AutopilotsChanged);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| ops.read(cx).next_scheduled_run(&triage)), None);
}

#[gpui::test]
fn spend_of_runnables_agents_created_is_marked(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let ops = cx.update(crate::ops);
    cx.update(|cx| ops.update(cx, |ops, cx| ops.ensure_loaded(cx)));
    cx.run_until_parked();
    let rows = cx.update(|cx| ops.read(cx).spend_by_runnable(crate::api::Period::Today).and_then(|l| l.value().cloned()));
    let rows = rows.expect("spend by agent");
    assert!(rows.iter().any(|row| row.spawned && row.runnable.as_str().starts_with("spawned:")), "{rows:#?}");
    assert!(rows.iter().filter(|row| !row.runnable.as_str().starts_with("spawned:")).all(|row| !row.spawned));
}
