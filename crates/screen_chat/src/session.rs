//! One chat session: the transcript, the composer with its pickers, the context panel, and
//! the session's overlays (agent picker, model picker, share, more menu).
//!
//! Layout, as `chat-session.html` draws it: the transcript column takes the width that is left,
//! with the composer docked under it (760px wide at most, centred); the context panel sits at
//! the right, 320px wide, and folds away with the header's panel toggle.
//!
//! Areas live in their own modules: [`input`] (composer wiring: send, stop, queueing, drafts,
//! completions, images), [`pickers`] (agent, model and mode), [`panel`] (the context panel),
//! [`header`] (title, header controls, more menu, rename and delete dialogs) and [`share`]
//! (the share popover and the local exports). [`memory`] keeps drafts across navigation.

mod header;
mod input;
mod memory;
mod panel;
mod pickers;
mod share;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use hub::api::{ChatMode, Session, SessionOrigin};
use hub::model::SessionId;
use hub::{CatalogModel, LoadState, OpsModel, SessionEvent, SessionModel};
use ui::prelude::*;
use ui_rich::{Composer, composer_view};

use crate::transcript::TranscriptView;
use crate::workdir::WorkdirChip;

/// Widest the composer gets, px (`max-width:760px` in the mockup).
const COMPOSER_MAX_WIDTH: f32 = 760.;

/// The whole content of `Route::Chat`.
pub struct SessionView {
    session: SessionId,
    model: Entity<SessionModel>,
    catalog: Entity<CatalogModel>,
    transcript: Entity<TranscriptView>,
    workdir: Entity<WorkdirChip>,
    composer: Entity<Composer>,
    ops: Entity<OpsModel>,
    /// The sidebar index's copy of the session, used until the session's own header loads.
    fallback: Option<Arc<Session>>,
    input: input::InputState,
    models: pickers::ModelList,
    share: share::ShareOptions,
    rename: Option<(Entity<ui_rich::TextInput>, Subscription)>,
    /// The selectable errors of failed MCP servers in the context panel, by server name.
    mcp_errors: std::collections::HashMap<String, Entity<ui_rich::markdown::Markdown>>,
    panel_open: bool,
    /// What was last pushed into the composer, so it is told only about changes.
    applied: Option<input::Applied>,
    _subs: Vec<Subscription>,
}

impl SessionView {
    /// The view of `session`. Loading starts at once; the composer takes focus.
    pub fn new(session: SessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = hub::session(&session, cx);
        let catalog = hub::catalog(cx);
        let ops = hub::ops(cx);
        let fallback = hub::chat_index(cx).read(cx).session(&session).cloned();
        let transcript = cx.new(|cx| TranscriptView::new(session.clone(), window, cx));
        let workdir = cx.new(|cx| WorkdirChip::new(Some(session.clone()), window, cx));
        let composer = cx.new(|cx| Composer::chat(cx).name("chat").placeholder("Message…"));
        let area = composer.read(cx).text_area().clone();
        let subs = vec![
            cx.observe(&model, |this, _, cx| this.sync(cx)),
            cx.subscribe_in(&model, window, Self::on_session_event),
            cx.observe(&catalog, |this, _, cx| this.sync(cx)),
            cx.subscribe_in(&composer, window, Self::on_composer_event),
            cx.subscribe_in(&area, window, Self::on_area_event),
            cx.observe(&composer, |_, _, cx| cx.notify()),
        ];
        let panel_open = memory::panel_open(cx);
        let mut this = Self {
            session,
            model,
            catalog,
            transcript,
            workdir,
            composer,
            ops,
            fallback,
            input: input::InputState::default(),
            models: pickers::ModelList::default(),
            share: share::ShareOptions::default(),
            rename: None,
            mcp_errors: Default::default(),
            panel_open,
            applied: None,
            _subs: subs,
        };
        this.restore_draft(cx);
        this.sync(cx);
        this.composer.read(cx).focus(window, cx);
        this
    }

    /// The session it shows.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, cx: &App) -> SharedString {
        self.header_session(cx).map_or_else(|| "Chat".into(), |s| SharedString::from(s.title.clone()))
    }

    /// The controls on the right of the shell's header, left to right: Share, New session,
    /// the panel toggle and the more menu.
    pub fn header_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        header::actions(self, window, cx)
    }

    /// What the shell's header shows right after the title: the workdir chip and the
    /// session's state badge ("waiting for you"), as `chat-session.html` draws them.
    pub fn header_badges(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        header::badges(self, window, cx)
    }

    /// The context panel is shown.
    pub fn is_panel_open(&self) -> bool {
        self.panel_open
    }

    /// Shows or hides the context panel; the choice is kept for every chat.
    pub fn set_panel_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.panel_open = open;
        memory::set_panel_open(open, cx);
        cx.notify();
    }

    /// The message box, for the shell's shortcuts and for tests.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The session's header: the loaded one, else the sidebar's copy.
    fn header_session<'a>(&'a self, cx: &'a App) -> Option<&'a Session> {
        self.model.read(cx).session().or(self.fallback.as_deref())
    }

    fn on_session_event(
        &mut self,
        _: &Entity<SessionModel>,
        event: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let SessionEvent::Removed = event {
            ui::overlays(window, cx).close_all(window, cx);
            let shown = matches!(hub::router(cx).read(cx).current(), hub::Route::Chat { session } if *session == self.session);
            if shown {
                hub::navigate(hub::Route::Chats, cx);
            }
        }
    }
}

/// Why the composer takes no input for this session, if it does not.
pub(crate) fn disabled_reason(model: &SessionModel) -> Option<SharedString> {
    if model.is_removed() {
        return Some("This chat was deleted".into());
    }
    if let LoadState::Failed(why) = model.state() {
        return Some(why.clone());
    }
    let session = model.session()?;
    if session.archived {
        return Some("Archived chat · restore it to reply".into());
    }
    match session.origin {
        SessionOrigin::Agent { .. } => Some("Sub-agent session · read-only".into()),
        SessionOrigin::Run { .. } => Some("Pipeline session · read-only".into()),
        _ if session.parent.is_some() => Some("Sub-agent session · read-only".into()),
        _ => None,
    }
}

/// The word for a chat mode, as the mockups write it.
pub(crate) fn mode_label(mode: ChatMode) -> &'static str {
    match mode {
        ChatMode::Build => "Build",
        ChatMode::Plan => "Plan",
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let composer = composer_view(&self.composer)
            .leading(pickers::agent_trigger(self, window, cx))
            .leading(pickers::model_trigger(self, window, cx));
        let composer = input::forced_skills(self, cx).into_iter().fold(composer, |c, el| c.leading(el));
        let composer = input::trailing(self, cx).into_iter().fold(composer, |c, el| c.trailing(el));
        let dock = div().flex_none().px(px(SPACE_6)).pb(px(SPACE_4)).child(
            row().justify_center().child(
                col()
                    .w_full()
                    .max_w(px(COMPOSER_MAX_WIDTH))
                    .on_action(cx.listener(|this, _: &ui_rich::text::Tab, window, cx| this.toggle_mode(window, cx)))
                    .child(composer)
                    .child(input::hint_row(self, cx)),
            ),
        );
        let conversation = col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(div().flex_1().min_h_0().flex().flex_col().child(self.transcript.clone()))
            .child(dock);
        div()
            .id("chat-session")
            .debug_selector(|| "chat-session".into())
            .flex()
            .flex_row()
            .size_full()
            .min_h_0()
            .child(conversation)
            .when(self.panel_open, |d| d.child(panel::panel(self, cx)))
    }
}
