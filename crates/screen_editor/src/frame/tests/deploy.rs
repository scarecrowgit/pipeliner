//! The deploy dialog.

use gpui::{TestAppContext, VisualTestContext};
use hub::api::{NodeKind, SourceKind, SourceNode};

use super::{PAY, calls, click, edit, open, shown, wait_check};
use crate::frame::deploy::DIALOG_ID;

fn dialog_open(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| ui::overlays(window, cx).is_open(DIALOG_ID, cx))
}

#[gpui::test]
async fn the_dialog_lists_the_changes_and_an_error_blocks_the_deploy(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    wait_check(cx);
    click(cx, "editor-deploy");
    assert!(dialog_open(cx));
    assert!(shown(cx, "deploy-change-0"));
    assert!(shown(cx, "deploy-change-1"));
    assert!(!shown(cx, "deploy-change-2"), "the fixture has two changes");
    assert!(shown(cx, "deploy-blocked"));
    click(cx, "deploy-confirm");
    assert!(calls(&t.api, "deploy_pipeline").is_empty(), "the Jira error blocks it");
    assert!(dialog_open(cx));
}

#[gpui::test]
async fn deploy_calls_the_command_with_the_note(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    edit(&t, cx, "Source", |g| {
        if let Some(node) = g.nodes.iter_mut().find(|n| n.id.as_str() == "jira") {
            node.kind = NodeKind::Source(SourceNode { source: SourceKind::Manual });
        }
    });
    wait_check(cx);
    click(cx, "editor-deploy");
    assert!(!t.view.read_with(cx, |v, cx| v.deploy_blocked(cx)), "nothing blocks it");
    let note = t
        .view
        .read_with(cx, |v, cx| v.deploy_dialog().map(|d| d.read(cx).note.clone()))
        .expect("the dialog is open");
    note.update(cx, |input, cx| input.set_text("Severity split", cx));
    cx.run_until_parked();
    click(cx, "deploy-confirm");
    assert_eq!(calls(&t.api, "save_pipeline"), [PAY], "saved before the deploy");
    assert_eq!(calls(&t.api, "deploy_pipeline"), [format!("{PAY} Severity split")]);
    assert!(!dialog_open(cx));
    t.view.read_with(cx, |v, cx| assert_eq!(v.versions(cx).live, Some(15)));
}
