//! The list: rows and groups, tabs, the search typed through the keyboard, selection by
//! pointer and keys, and the load problems.

use std::path::PathBuf;

use gpui::TestAppContext;
use hub::Route;
use hub::api::{LoadProblem, PipelineId};
use hub::model::RunnableRef;

use super::{click, lead, open, order, route, selected, shown};

#[gpui::test]
fn rows_and_groups_come_from_the_fixture(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    let rows = order(&t, cx);
    assert_eq!(rows.len(), 14, "two orchestrators, three pipelines, six agents, three created");
    assert_eq!(rows[0], lead(), "orchestrators come first");
    assert_eq!(rows[2], RunnableRef::pipeline("pay-triage"));
    assert_eq!(rows[13], RunnableRef::spawned("test-writer"), "created agents come last");
    for probe in ["agents-row-agent:fixer", "agents-tab-all", "agents-tab-created", "agents-search", "agents-new"] {
        assert!(shown(cx, probe), "{probe} is drawn");
    }
    assert!(!shown(cx, "agents-tab-archived"), "no archived tab while nothing is archived");
    assert!(shown(cx, "agents-more-orchestrator:payments-lead"), "the selection's panel is open");
}

#[gpui::test]
fn tabs_and_the_typed_search_narrow_the_list(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    click(cx, "agents-tab-pipelines");
    assert_eq!(order(&t, cx).len(), 3);
    click(cx, "agents-tab-created");
    assert_eq!(order(&t, cx).len(), 3);
    click(cx, "agents-tab-all");

    click(cx, "agents-search");
    cx.simulate_input("sonnet");
    let rows = order(&t, cx);
    assert!(rows.contains(&RunnableRef::agent("reproducer")) && !rows.contains(&RunnableRef::agent("fixer")), "{rows:?}");
    assert!(!rows.contains(&lead()), "the model is searched");
    assert_eq!(selected(&t, cx), rows.first().cloned(), "the selection follows the first match");

    // Down in the search field moves the selection.
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&t, cx), rows.get(1).cloned());

    cx.simulate_keystrokes("escape");
    assert_eq!(order(&t, cx).len(), 14, "Escape clears the search");

    // "F" from the list jumps to the search field.
    click(cx, "agents-row-agent:build");
    cx.simulate_keystrokes("f");
    cx.simulate_input("codex");
    assert_eq!(order(&t, cx), [RunnableRef::agent("codex")]);
    cx.simulate_input("zzz");
    assert!(shown(cx, "agents-empty"), "nothing matches: the empty state shows");
}

#[gpui::test]
fn selecting_navigates_and_shows_the_detail(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    click(cx, "agents-row-agent:fixer");
    assert_eq!(selected(&t, cx), Some(RunnableRef::agent("fixer")));
    assert_eq!(route(cx), Route::Agents { runnable: Some(RunnableRef::agent("fixer")) });
    assert!(shown(cx, "agents-more-agent:fixer") && shown(cx, "agents-chat"));

    // The route changing from outside (the shell, back) moves the selection.
    cx.update(|_, cx| hub::navigate(Route::Agents { runnable: Some(RunnableRef::agent("analyst")) }, cx));
    cx.run_until_parked();
    assert_eq!(selected(&t, cx), Some(RunnableRef::agent("analyst")));
}

#[gpui::test]
fn arrows_move_and_enter_opens(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&t, cx), Some(RunnableRef::orchestrator("release-captain")));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&t, cx), Some(RunnableRef::pipeline("pay-triage")));
    assert_eq!(route(cx), Route::Agents { runnable: Some(RunnableRef::pipeline("pay-triage")) });
    cx.simulate_keystrokes("up down enter");
    assert_eq!(route(cx), Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None });
}

#[gpui::test]
fn a_pipeline_opens_in_the_editor(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx, Some(RunnableRef::pipeline("pay-triage")));
    assert!(shown(cx, "agents-open-editor"));
    click(cx, "agents-open-editor");
    assert_eq!(route(cx), Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None });
}

#[gpui::test]
fn a_load_problem_is_shown(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    assert!(!shown(cx, "agents-problems"));
    t.view.update(cx, |v, cx| {
        v.problems_override = Some(vec![LoadProblem {
            path: PathBuf::from("/home/me/.config/harness/library/agents/broken.yaml"),
            message: "expected a mapping at line 3".into(),
        }]);
        cx.notify();
    });
    assert!(shown(cx, "agents-problems"), "the notice is over the list");
    assert!(shown(cx, "agents-problem-0"), "the file has a row with the load problem state");
}
