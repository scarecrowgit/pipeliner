//! The panel's header: the key, the type (a picker), then Start or Stop for the attached
//! runner, the more menu (copy the key, delete the issue) and Close.

use std::sync::Arc;

use gpui::{AnyElement, ClipboardItem, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{Issue, RunState};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use super::pickers::TYPE;
use super::{Named, IssuePanel, PAD_X};

/// Anchor key and probe of the more menu.
const MORE: &str = "issue-more";
/// Id of the delete confirmation.
const DELETE_DIALOG: &str = "issue-delete-dialog";

/// The header row; without an issue it shows the key and Close only.
pub(super) fn header(this: &IssuePanel, issue: Option<&Arc<Issue>>, window: &mut Window, cx: &mut Context<IssuePanel>) -> AnyElement {
    let mut head = row()
        .flex_none()
        .w_full()
        .h(px(CARD_HEAD_HEIGHT))
        .gap(px(SPACE_2))
        .pl(px(PAD_X))
        .pr(px(SPACE_3))
        .border_b_1()
        .border_color(LINE)
        .child(div().flex_none().type_style(MONO).text_color(TEXT_MUTED).child(this.issue.to_string()).named("issue-key"));
    if let Some(issue) = issue {
        head = head.child(type_tag(issue, window, cx));
    }
    let mut end = row().flex_none().gap(px(SPACE_1)).ml_auto();
    if let Some(issue) = issue {
        if let Some(action) = run_action(this, issue, cx) {
            end = end.child(div().mr(px(SPACE_1)).child(action));
        }
        let view = cx.entity();
        end = end.child(
            IconButton::new(MORE, IconName::More)
                .small()
                .tooltip("More actions")
                .on_click(move |_, window, cx| view.update(cx, |this, cx| this.open_more(window, cx)))
                .popover_anchor(MORE)
                .probe(MORE),
        );
    }
    end = end.child(
        IconButton::new("issue-close", IconName::X)
            .small()
            .tooltip("Close")
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .probe("issue-close"),
    );
    head.child(end).into_any_element()
}

/// The type as a tag that opens the type picker.
fn type_tag(issue: &Arc<Issue>, window: &mut Window, cx: &mut Context<IssuePanel>) -> AnyElement {
    let kind = issue.kind;
    let id = ElementId::Name(TYPE.into());
    let el = row()
        .id(id.clone())
        .debug_selector(|| TYPE.into())
        .flex_none()
        .gap(px(SPACE_1))
        .h(px(20.))
        .px(px(SPACE_1_5))
        .rounded(px(RADIUS_MD))
        .type_style(LABEL_TIGHT)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(kind.label());
    let rest = Look::bg(SURFACE_HOVER).fg(TEXT_SECONDARY);
    let hover = Look::bg(SURFACE_HOVER).fg(TEXT);
    motion::hover_fade(el, id, rest, hover, window, cx)
        .on_click(cx.listener(move |this, _, window, cx| this.open_type(kind, window, cx)))
        .popover_anchor(TYPE)
        .probe(TYPE)
        .into_any_element()
}

/// Start when a runner is attached and nothing runs; Stop while a run is open.
fn run_action(this: &IssuePanel, issue: &Issue, cx: &mut Context<IssuePanel>) -> Option<AnyElement> {
    let latest = this.latest_run(cx);
    if let Some(run) = latest.filter(|run| run.state.is_open() && run.state != RunState::Interrupted) {
        let id = run.id.clone();
        return Some(
            Button::new("issue-stop", "Stop")
                .small()
                .ghost()
                .icon(IconName::Pause)
                .on_click(move |_, _, cx| hub::commands(cx).cancel_run(&id, cx))
                .probe("issue-stop")
                .into_any_element(),
        );
    }
    issue.runner.as_ref()?;
    Some(
        Button::new("issue-start", "Start")
            .small()
            .secondary()
            .icon(IconName::Play)
            .on_click(cx.listener(|this, _, _, cx| hub::commands(cx).start_issue(&this.issue, cx)))
            .probe("issue-start")
            .into_any_element(),
    )
}

impl IssuePanel {
    fn open_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let key = self.issue.to_string();
        let at = Anchor::of(MORE, window, cx);
        ui::overlays(window, cx).open_menu(
            at,
            Placement::below().end(),
            move |_, _| {
                let (copy, delete) = (key.clone(), view.clone());
                Menu::new("issue-more-menu")
                    .width(220.)
                    .item(MenuItem::new("issue-more-copy", "Copy key").icon(IconName::Copy).on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        ui::overlays(window, cx).toast(Toast::new(format!("Copied {copy}")), cx);
                    }))
                    .separator()
                    .item(
                        MenuItem::new("issue-more-delete", "Delete issue…")
                            .icon(IconName::Trash)
                            .danger()
                            .on_click(move |_, window, cx| delete.update(cx, |this, cx| this.confirm_delete(window, cx))),
                    )
            },
            window,
            cx,
        );
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let key = self.issue.clone();
        let title: SharedString = format!("Delete {key}?").into();
        ui::overlays(window, cx).open_dialog(
            DELETE_DIALOG,
            move |_, _| {
                let (v, key) = (view.clone(), key.clone());
                Dialog::new(DELETE_DIALOG, title.clone())
                    .subtitle("Its activity and comments go with it. Runs and their branches stay.")
                    .cancel_button()
                    .primary_action(Button::new("issue-delete-confirm", "Delete issue").danger().kbd("⌘↵"), move |_, cx| {
                        hub::commands(cx).delete_issue(&key, cx);
                        v.update(cx, |this, cx| this.close(cx));
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
    }
}
