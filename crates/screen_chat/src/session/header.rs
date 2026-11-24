//! The session's part of the shell header: the workdir chip and state badge after the title,
//! and at the right Share, New session, the panel toggle and the more menu (rename, pin,
//! fork, export, archive, delete) with its rename and delete dialogs.

use gpui::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, Window, div, prelude::*, px};
use hub::Route;
use hub::api::SessionState;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::SessionView;

/// Anchor key and probe of the Share button.
pub(super) const SHARE: &str = "chat-share";
const MORE: &str = "chat-more";
const TOGGLE: &str = "chat-panel-toggle";
const NEW: &str = "chat-new-session";
const RENAME_DIALOG: &str = "chat-rename-dialog";
const DELETE_DIALOG: &str = "chat-delete-dialog";

pub(super) fn actions(this: &mut SessionView, window: &mut Window, cx: &mut Context<SessionView>) -> Vec<AnyElement> {
    let view = cx.entity();
    let share_open = ui::overlays(window, cx).is_open(SHARE, cx);
    let open = this.panel_open;
    let (share, toggle, more) = (view.clone(), view.clone(), view);
    vec![
        Button::new(SHARE, "Share")
            .ghost()
            .icon(IconName::Share)
            .selected(share_open)
            .on_click(move |_, window, cx| share.update(cx, |this, cx| this.open_share(window, cx)))
            .popover_anchor(SHARE)
            .probe(SHARE)
            .into_any_element(),
        Button::new(NEW, "New session")
            .secondary()
            .icon(IconName::Plus)
            .on_click(|_, _, cx| hub::navigate(Route::NewChat, cx))
            .probe(NEW)
            .into_any_element(),
        IconButton::new(TOGGLE, IconName::Panel)
            .current(open)
            .tooltip(if open { "Hide context panel" } else { "Show context panel" })
            .on_click(move |_, _, cx| toggle.update(cx, |this, cx| this.set_panel_open(!open, cx)))
            .probe(TOGGLE)
            .into_any_element(),
        IconButton::new(MORE, IconName::More)
            .tooltip("More")
            .on_click(move |_, window, cx| more.update(cx, |this, cx| this.open_more(window, cx)))
            .popover_anchor(MORE)
            .probe(MORE)
            .into_any_element(),
    ]
}

pub(super) fn badges(this: &mut SessionView, _: &mut Window, cx: &mut Context<SessionView>) -> Vec<AnyElement> {
    let mut out = vec![div().flex_none().ml(px(SPACE_1)).child(this.workdir.clone()).into_any_element()];
    let state = this.header_session(cx).map(|s| s.state);
    let badge = match state {
        Some(SessionState::Waiting) => Some(Badge::new("waiting for you").tone(BadgeTone::Warning).dot(Status::Waiting)),
        Some(SessionState::Running) => Some(Badge::new("running").dot(Status::Running)),
        Some(SessionState::Failed) => Some(Badge::new("failed").tone(BadgeTone::Danger).dot(Status::Error)),
        _ => None,
    };
    if let Some(badge) = badge {
        out.push(div().flex_none().ml(px(SPACE_1)).child(badge).probe("chat-state").into_any_element());
    }
    out
}

impl SessionView {
    fn open_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of(MORE, window, cx);
        let view = cx.entity();
        ui::overlays(window, cx).open_menu(anchor, Placement::below().end(), move |_, cx| more_menu(&view, cx), window, cx);
    }

    fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx).to_string();
        let input = cx.new(|cx| TextInput::new(cx).with_text(&title).placeholder("Chat title"));
        input.update(cx, |input, cx| input.select_all(cx));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Submit => {
                this.commit_rename(cx);
                ui::overlays(window, cx).close(RENAME_DIALOG, window, cx);
            }
            TextInputEvent::Cancel => ui::overlays(window, cx).close(RENAME_DIALOG, window, cx),
            _ => {}
        });
        self.rename = Some((input.clone(), sub));
        let view = cx.entity();
        let field = input.clone();
        ui::overlays(window, cx).open_dialog(
            RENAME_DIALOG,
            move |_, _| {
                let v = view.clone();
                Dialog::new(RENAME_DIALOG, "Rename chat")
                    .width(440.)
                    .child(text_field("chat-rename-input", &field))
                    .cancel_button()
                    .primary_action(Button::new("chat-rename-save", "Rename").primary().kbd("⌘↵"), move |_, cx| {
                        v.update(cx, |this, cx| this.commit_rename(cx))
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        input.read(cx).focus(window);
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some((input, _)) = self.rename.take() else { return };
        let title = input.read(cx).text().trim().to_string();
        if !title.is_empty() && title != self.title(cx).as_ref() {
            hub::commands(cx).rename_session(&self.session, &title, cx);
        }
    }

    fn open_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let id = self.session.clone();
        ui::overlays(window, cx).open_dialog(
            DELETE_DIALOG,
            move |_, _| {
                let id = id.clone();
                Dialog::new(DELETE_DIALOG, "Delete this chat?")
                    .width(440.)
                    .child(
                        div()
                            .text_color(TEXT_SECONDARY)
                            .child(format!("“{title}” and its sub-agent sessions are deleted. This can’t be undone.")),
                    )
                    .cancel_button()
                    .primary_action(Button::new("chat-delete-confirm", "Delete chat").danger(), move |_, cx| {
                        hub::commands(cx).delete_session(&id, cx)
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
    }

    fn fork(&mut self, cx: &mut Context<Self>) {
        let forked = hub::commands(cx).fork_session(&self.session, None, cx);
        cx.spawn(async move |_, cx| {
            if let Some(session) = forked.await {
                cx.update(|cx| hub::navigate(Route::Chat { session: session.id }, cx)).ok();
            }
        })
        .detach();
    }
}

fn more_menu(view: &Entity<SessionView>, cx: &App) -> Menu {
    let this = view.read(cx);
    let session = this.header_session(cx);
    let pinned = session.is_some_and(|s| s.pinned);
    let archived = session.is_some_and(|s| s.archived);
    let id = this.session.clone();
    let act = |f: fn(&mut SessionView, &mut Window, &mut Context<SessionView>)| {
        let v = view.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| v.update(cx, |this, cx| f(this, window, cx))
    };
    let (pin_id, archive_id) = (id.clone(), id);
    Menu::new("chat-more-menu")
        .width(220.)
        .item(MenuItem::new("chat-more-rename", "Rename…").icon(IconName::Edit).on_click(act(SessionView::open_rename)))
        .item(
            MenuItem::new("chat-more-pin", if pinned { "Unpin" } else { "Pin" })
                .icon(IconName::Pin)
                .on_click(move |_, _, cx| hub::commands(cx).pin_session(&pin_id, !pinned, cx)),
        )
        .item(MenuItem::new("chat-more-fork", "Fork chat").icon(IconName::Fork).on_click(act(|this, _, cx| this.fork(cx))))
        .item(MenuItem::new("chat-more-export", "Export…").icon(IconName::Download).on_click(act(SessionView::open_share)))
        .item(
            MenuItem::new("chat-more-archive", if archived { "Restore" } else { "Archive" })
                .icon(IconName::Archive)
                .on_click(move |_, _, cx| hub::commands(cx).archive_session(&archive_id, !archived, cx)),
        )
        .separator()
        .item(MenuItem::new("chat-more-delete", "Delete…").icon(IconName::Trash).danger().on_click(act(SessionView::open_delete)))
}
