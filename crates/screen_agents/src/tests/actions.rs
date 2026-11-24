//! The New menu, the more menu, the delete dialog and the archive toast, each down to the
//! command it sends.

use gpui::{TestAppContext, VisualTestContext};
use hub::model::RunnableRef;

use super::{Open, calls, click, lead, open, selected, shown};
use crate::dialogs::Form;

/// The text input of the open dialog, if it has one.
fn form_input(t: &Open, cx: &mut VisualTestContext) -> gpui::Entity<ui_rich::TextInput> {
    t.view.read_with(cx, |v, _| match v.form.as_ref() {
        Some(Form::Delete { input, .. } | Form::Rename { input, .. } | Form::Import { input } | Form::Export { input, .. }) => {
            input.clone()
        }
        _ => panic!("no dialog with an input is open"),
    })
}

#[gpui::test]
fn each_new_item_calls_its_command(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));

    click(cx, "agents-new");
    click(cx, "agents-new-agent");
    assert!(shown(cx, "agents-template-blank-agent"), "a blank agent is offered first");
    click(cx, "agents-template-create");
    assert_eq!(calls(&t.api, "save_runnable"), ["agent:new-agent"]);
    assert_eq!(selected(&t, cx), Some(RunnableRef::agent("new-agent")));

    click(cx, "agents-new");
    click(cx, "agents-new-orchestrator");
    click(cx, "agents-template-create");
    assert_eq!(calls(&t.api, "save_runnable")[1], "orchestrator:new-orchestrator");

    click(cx, "agents-new");
    click(cx, "agents-new-pipeline");
    let kinds = t.view.read_with(cx, |v, _| match v.form.as_ref() {
        Some(Form::Template { kind, .. }) => v.choices(*kind).iter().map(|c| format!("{c:?}")).collect::<Vec<_>>(),
        _ => panic!("the template dialog is open"),
    });
    assert!(kinds.len() == 1 && kinds[0].contains("triage"), "only pipeline templates for a pipeline: {kinds:?}");
    click(cx, "agents-template-triage");
    click(cx, "agents-template-create");
    assert_eq!(calls(&t.api, "create_from_template"), ["triage"]);

    click(cx, "agents-new");
    click(cx, "agents-new-template");
    click(cx, "agents-template-reviewer");
    click(cx, "agents-template-create");
    assert_eq!(calls(&t.api, "create_from_template")[1], "reviewer");

    click(cx, "agents-new");
    click(cx, "agents-new-import");
    let input = form_input(&t, cx);
    input.update(cx, |i, cx| i.set_text("~/agents/reviewer.yaml", cx));
    click(cx, "agents-import-confirm");
    assert_eq!(calls(&t.api, "import_runnable"), ["/home/me/agents/reviewer.yaml"]);

    click(cx, "agents-new");
    click(cx, "agents-new-autopilot");
    assert_eq!(super::route(cx), hub::Route::Issues { issue: None }, "autopilots live with issues");
}

#[gpui::test]
fn each_more_item_calls_its_command(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    let more = "agents-more-orchestrator:payments-lead";

    click(cx, more);
    click(cx, "agents-more-duplicate");
    assert_eq!(calls(&t.api, "duplicate_runnable"), ["orchestrator:payments-lead"]);
    assert_eq!(selected(&t, cx), Some(RunnableRef("orchestrator:payments-lead-copy".into())), "the copy is selected");

    click(cx, "agents-row-orchestrator:payments-lead");
    click(cx, more);
    click(cx, "agents-more-rename");
    form_input(&t, cx).update(cx, |i, cx| i.set_text("Payments captain", cx));
    click(cx, "agents-rename-save");
    assert_eq!(calls(&t.api, "rename_runnable"), ["orchestrator:payments-lead Payments captain"]);

    click(cx, more);
    click(cx, "agents-more-export");
    form_input(&t, cx).update(cx, |i, cx| i.set_text("~/Downloads", cx));
    click(cx, "agents-export-confirm");
    assert_eq!(calls(&t.api, "export_runnable"), ["orchestrator:payments-lead /home/me/Downloads"]);
    assert!(cx.update(|window, cx| ui::overlays(window, cx).has_toast(cx)), "the export says where it went");

    click(cx, more);
    click(cx, "agents-more-pause");
    assert_eq!(calls(&t.api, "set_runnable_paused"), ["orchestrator:payments-lead true"]);

    click(cx, more);
    click(cx, "agents-more-edit");
    assert!(shown(cx, "agents-edit-dialog"), "the definition opens for editing");
    let role = t.view.read_with(cx, |v, _| match v.form.as_ref() {
        Some(Form::Edit(form)) => form.role.clone(),
        _ => panic!("the edit form is open"),
    });
    role.update(cx, |area, cx| area.set_text("Lead payments work.\n\nAsk before merging.", cx));
    click(cx, "agents-edit-save");
    let saved = calls(&t.api, "save_runnable");
    assert_eq!(saved, ["orchestrator:payments-lead"]);
    let role = t.api.with_world(|w| w.runnables.iter().find(|r| r.reference == lead()).and_then(|r| r.agent.clone()).map(|a| a.role));
    assert_eq!(role.as_deref(), Some("Lead payments work.\n\nAsk before merging."));
}

#[gpui::test]
fn built_ins_cannot_be_edited_or_removed(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(RunnableRef::agent("build")));
    let more = "agents-more-agent:build";
    for item in ["agents-more-edit", "agents-more-rename", "agents-more-archive", "agents-more-delete"] {
        click(cx, more);
        assert!(shown(cx, item), "{item} is listed, disabled");
        click(cx, item);
        assert!(!shown(cx, "agents-delete-dialog") && !shown(cx, "agents-edit-dialog"));
        cx.simulate_keystrokes("escape");
    }
    assert!(calls(&t.api, "set_runnable_archived").is_empty() && calls(&t.api, "rename_runnable").is_empty());
    click(cx, more);
    click(cx, "agents-more-duplicate");
    assert_eq!(calls(&t.api, "duplicate_runnable"), ["agent:build"], "a built-in can still be copied");
}

#[gpui::test]
fn delete_names_what_uses_it_and_asks_for_the_name(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(RunnableRef::agent("fixer")));
    click(cx, "agents-more-agent:fixer");
    click(cx, "agents-more-delete");
    assert!(shown(cx, "agents-delete-dialog"));
    assert!(shown(cx, "agents-delete-hit-0"), "the running issue and what uses it are listed");
    assert!(shown(cx, "agents-delete-hit-1"), "the agent it created is listed");

    click(cx, "agents-delete-confirm");
    assert!(calls(&t.api, "delete_runnable").is_empty(), "nothing happens before the name is typed");

    cx.simulate_input("Fixer");
    click(cx, "agents-delete-confirm");
    assert_eq!(calls(&t.api, "delete_runnable"), ["agent:fixer"]);
    assert!(!cx.update(|window, cx| ui::overlays(window, cx).is_open("agents-delete-dialog", cx)));
    assert_eq!(selected(&t, cx), None);
}

#[gpui::test]
fn archive_shows_a_toast_whose_undo_restores(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(RunnableRef::orchestrator("release-captain")));
    click(cx, "agents-more-orchestrator:release-captain");
    click(cx, "agents-more-archive");
    assert_eq!(calls(&t.api, "set_runnable_archived"), ["orchestrator:release-captain true"]);
    assert!(shown(cx, "toast"), "the toast is up");
    assert!(shown(cx, "agents-tab-archived"), "the archived tab appears");
    click(cx, "toast-undo");
    assert_eq!(calls(&t.api, "set_runnable_archived")[1], "orchestrator:release-captain false");
    assert_eq!(selected(&t, cx), Some(RunnableRef::orchestrator("release-captain")));
}
