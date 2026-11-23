use std::path::PathBuf;
use std::sync::Arc;

use base::paths::Paths;
use model::{
    AgentDef, Autopilot, AutopilotId, AutopilotTrigger, ModelRef, Origin, PermissionRule, PipelineId, ProjectId,
    RuleDecision, RuleScope, RunId, Runnable, RunnableKind, RunnableRef, Schedule, SessionId, Uses, VersionState,
};

use super::store;
use crate::Store;

#[test]
fn deploying_retires_the_live_version() {
    let store = store();
    let p = PipelineId::new("fix");
    assert_eq!(store.live_pipeline_version(&p).unwrap(), None);
    let v1 = store.deploy_pipeline(&p, "a = 1", "h1", "first", 10).unwrap();
    let v2 = store.deploy_pipeline(&p, "a = 2", "h2", "second", 20).unwrap();
    store.deploy_pipeline(&PipelineId::new("other"), "x", "h", "", 30).unwrap();
    assert_eq!((v1.version, v2.version), (1, 2));
    assert_eq!(store.live_pipeline_version(&p).unwrap(), Some(v2.clone()));
    let old = store.pipeline_version(&p, 1).unwrap().unwrap();
    assert_eq!(old.state, VersionState::Retired);
    assert_eq!(old.def_text, "a = 1");
    let versions: Vec<u32> = store.pipeline_versions(&p).unwrap().iter().map(|v| v.version).collect();
    assert_eq!(versions, [2, 1]);
    assert_eq!(store.pipeline_version(&p, 9).unwrap(), None);
}

#[test]
fn concurrent_deploys_leave_one_live_version_and_no_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&Paths::under(dir.path())).unwrap());
    let p = PipelineId::new("fix");
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let store = Arc::clone(&store);
            let p = p.clone();
            std::thread::spawn(move || {
                for i in 0..10 {
                    store.deploy_pipeline(&p, &format!("{t}/{i}"), "h", "", i).unwrap();
                    let live = store.live_pipeline_version(&p).unwrap();
                    assert!(live.is_some(), "a reader never sees a pipeline without a live version");
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let versions = store.pipeline_versions(&p).unwrap();
    assert_eq!(versions.iter().map(|v| v.version).collect::<Vec<_>>(), (1..=80).rev().collect::<Vec<_>>());
    assert_eq!(versions.iter().filter(|v| v.state == VersionState::Live).count(), 1);
    assert_eq!(versions[0].state, VersionState::Live);
}

#[test]
fn the_one_live_rule_is_enforced_by_the_schema() {
    let store = store();
    store.deploy_pipeline(&PipelineId::new("p"), "x", "h", "", 1).unwrap();
    let second_live = store.db.write().execute(
        "INSERT INTO pipeline_versions (pipeline, version, state, def_text, hash, note, deployed_at)
         VALUES ('p', 9, 'live', '', '', '', 0)",
        [],
    );
    assert!(second_live.is_err());
}

fn spawned(id: &str, run: Option<&str>) -> Runnable {
    Runnable {
        reference: RunnableRef::spawned(id),
        kind: RunnableKind::Agent,
        name: format!("Helper {id}"),
        description: "checks tests".into(),
        origin: Origin::Spawned {
            by: RunnableRef::agent("lead"),
            run: run.map(RunId::new),
            session: Some(SessionId::new("s1")),
        },
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: Some(AgentDef { role: "You check.".into(), model: ModelRef::parse("a/b"), ..Default::default() }),
        path: None,
    }
}

#[test]
fn spawned_agents_are_kept_and_ended() {
    let store = store();
    let a = spawned("a", Some("r1"));
    let mut b = spawned("b", None);
    store.upsert_spawned(&a).unwrap();
    store.upsert_spawned(&b).unwrap();
    b.description = "changed".into();
    store.upsert_spawned(&b).unwrap();
    assert!(store.end_spawned(&a.reference, 99).unwrap());
    assert!(!store.end_spawned(&RunnableRef::spawned("zz"), 1).unwrap());
    store.upsert_spawned(&a).unwrap();

    let all = store.spawned_runnables(None).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!((&all[0].runnable, all[0].ended), (&a, Some(99)), "re-saving keeps the end");
    assert_eq!((&all[1].runnable, all[1].ended), (&b, None));
    let of_run = store.spawned_runnables(Some(&RunId::new("r1"))).unwrap();
    assert_eq!(of_run.len(), 1);

    let mut library = spawned("c", None);
    library.origin = Origin::Library;
    assert!(store.upsert_spawned(&library).is_err());
}

#[test]
fn permission_rules_are_unique_per_tool_pattern_and_scope() {
    let store = store();
    let rule = |tool: &str, pattern: &str, decision, scope| PermissionRule {
        tool: tool.into(),
        pattern: pattern.into(),
        decision,
        scope,
    };
    let repo = RuleScope::Repo { path: PathBuf::from("/home/u/code/pay") };
    let session = RuleScope::Session { session: SessionId::new("s1") };
    store.add_permission_rule(&rule("bash", "bun test", RuleDecision::Allow, repo.clone())).unwrap();
    store.add_permission_rule(&rule("bash", "rm", RuleDecision::Deny, RuleScope::Global)).unwrap();
    store.add_permission_rule(&rule("write_file", "", RuleDecision::Ask, session.clone())).unwrap();
    // Same tool, pattern and scope: the decision changes, no second row.
    store.add_permission_rule(&rule("bash", "bun test", RuleDecision::Ask, repo.clone())).unwrap();

    assert_eq!(store.permission_rules(&repo).unwrap(), vec![rule("bash", "bun test", RuleDecision::Ask, repo.clone())]);
    assert_eq!(store.permission_rules(&RuleScope::Global).unwrap().len(), 1);
    assert_eq!(store.permission_rules(&session).unwrap()[0].scope, session);
    assert_eq!(store.all_permission_rules().unwrap().len(), 3);

    assert!(store.remove_permission_rule(&rule("bash", "bun test", RuleDecision::Allow, repo.clone())).unwrap());
    assert!(!store.remove_permission_rule(&rule("bash", "bun test", RuleDecision::Allow, repo.clone())).unwrap());
    assert!(store.permission_rules(&repo).unwrap().is_empty());
    let tilde = RuleScope::Repo { path: PathBuf::from("~/code") };
    assert!(store.add_permission_rule(&rule("bash", "", RuleDecision::Allow, tilde)).is_err());
}

fn autopilot(id: &str, project: &str, name: &str) -> Autopilot {
    Autopilot {
        id: AutopilotId::new(id),
        project: ProjectId::new(project),
        name: name.into(),
        trigger: AutopilotTrigger::Schedule { schedule: Schedule::Week { weekday: 0, hour: 9, minute: 30 } },
        target: RunnableRef::pipeline("triage"),
        prompt: "look at new errors".into(),
        enabled: true,
        paused_reason: None,
        next_run: Some(1_000),
        last_run: None,
        last_result: String::new(),
        budget_usd: Some(2.5),
    }
}

#[test]
fn autopilots_round_trip_tick_and_delete() {
    let store = store();
    let a = autopilot("a", "p1", "Nightly");
    let mut b = autopilot("b", "p1", "Errors");
    b.trigger = AutopilotTrigger::Event { source: "sentry".into() };
    b.paused_reason = Some("token expired".into());
    let c = autopilot("c", "p2", "Weekly");
    for x in [&a, &b, &c] {
        store.upsert_autopilot(x).unwrap();
    }
    assert_eq!(store.autopilot(&b.id).unwrap(), Some(b.clone()));
    let names = |list: Vec<Autopilot>| -> Vec<String> { list.into_iter().map(|a| a.name).collect() };
    assert_eq!(names(store.autopilots(Some(&ProjectId::new("p1"))).unwrap()), ["Errors", "Nightly"]);
    assert_eq!(store.autopilots(None).unwrap().len(), 3);

    assert!(store.record_autopilot_tick(&a.id, 500, "filed 2 issues", Some(2_000)).unwrap());
    assert!(!store.record_autopilot_tick(&AutopilotId::new("zz"), 1, "", None).unwrap());
    let ticked = store.autopilot(&a.id).unwrap().unwrap();
    assert_eq!((ticked.last_run, ticked.last_result.as_str(), ticked.next_run), (Some(500), "filed 2 issues", Some(2_000)));

    assert!(store.delete_autopilot(&a.id).unwrap());
    assert!(!store.delete_autopilot(&a.id).unwrap());
    assert_eq!(store.autopilot(&a.id).unwrap(), None);
}
