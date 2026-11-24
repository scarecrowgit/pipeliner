//! The pane at the right of the list: the selected chat's title, facts, matching message and
//! the buttons for what the footer's keys do.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};
use hub::api::{Session, SessionHit, SessionState};
use ui::dev::Probe as _;
use ui::prelude::*;

use crate::labels::{agent_name, ago_in_words, status};
use crate::palette::Palette;
use crate::snippet;

/// Lines of the matching message the preview shows.
const MESSAGE_LINES: usize = 10;

impl Palette {
    pub(crate) fn preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(hit) = self.selected_hit() else {
            return col()
                .size_full()
                .items_center()
                .justify_center()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Select a chat to preview it")
                .probe("palette-preview")
                .into_any_element();
        };
        let session = &hit.session;
        let now = self.clock();
        let offset = crate::groups::local_offset(now);

        let title = if hit.title_highlights.is_empty() {
            div().child(session.title.clone())
        } else {
            snippet::block(session.title.clone(), &hit.title_highlights, 3)
        };
        let heading = row()
            .gap(px(SPACE_2))
            .items_start()
            .child(div().flex_none().mt(px(10.)).child(StatusDot::new(status(session.state))))
            .child(grow().type_style(TITLE).text_color(TEXT).child(title));

        let facts = div()
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(format!(
                "{} · updated {} · started {}",
                model_name(session),
                ago_in_words(now, session.updated, offset),
                base::time::long_date(session.created, offset),
            ));

        let mut preview = col()
            .id("palette-preview")
            .size_full()
            .overflow_y_scroll()
            .px(px(SPACE_6))
            .py(px(SPACE_5))
            .child(heading.flex_none())
            .child(badges(hit).flex_none().mt(px(SPACE_2)))
            .child(workdir_line(session).flex_none().mt(px(SPACE_2_5)))
            .child(facts.flex_none().mt(px(SPACE_1)));
        if let Some(found) = &hit.snippet {
            let text = snippet::window(&found.text, &found.highlights);
            preview = preview.child(
                col()
                    .flex_none()
                    .mt(px(SPACE_3_5))
                    .pt(px(SPACE_2_5))
                    .gap(px(SPACE_1))
                    .border_t_1()
                    .border_color(LINE_SUBTLE)
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(format!("Matching message · {}", found.seq)))
                    .child(
                        snippet::block(text.text, &text.marks, MESSAGE_LINES)
                            .type_style(BODY)
                            .text_color(TEXT_SECONDARY),
                    ),
            );
        }
        preview.child(self.actions(session, cx).flex_none().mt(px(SPACE_5))).probe("palette-preview").into_any_element()
    }

    /// Open, Fork, Pin and Archive for the selected chat.
    fn actions(&self, session: &Session, cx: &mut Context<Self>) -> gpui::Div {
        let pinned = session.pinned;
        row()
            .gap(px(SPACE_2))
            .child(
                Button::new("palette-open", "Open")
                    .primary()
                    .small()
                    .kbd("↵")
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected(cx)))
                    .probe("palette-open"),
            )
            .child(
                Button::new("palette-fork", "Fork")
                    .secondary()
                    .small()
                    .icon(IconName::Fork)
                    .on_click(cx.listener(|this, _, _, cx| this.fork_selected(cx)))
                    .probe("palette-fork"),
            )
            .child(
                Button::new("palette-pin", if pinned { "Unpin" } else { "Pin" })
                    .ghost()
                    .small()
                    .icon(IconName::Pin)
                    .on_click(cx.listener(|this, _, _, cx| this.pin_selected(cx)))
                    .probe("palette-pin"),
            )
            .child(
                Button::new("palette-archive", "Archive")
                    .ghost()
                    .small()
                    .icon(IconName::Archive)
                    .on_click(cx.listener(|this, _, _, cx| this.archive_selected(cx)))
                    .probe("palette-archive"),
            )
    }
}

/// The model's name as the chat uses it: `opus-5.5`.
fn model_name(session: &Session) -> String {
    session.model.model.clone()
}

/// State, agent, size, cost and sub-agents of a chat as badges.
fn badges(hit: &SessionHit) -> gpui::Div {
    let session = &hit.session;
    let state = match session.state {
        SessionState::Waiting => Some(Badge::new("waiting for you").tone(BadgeTone::Warning)),
        SessionState::Running => Some(Badge::new("running").dot(Status::Running)),
        SessionState::Failed => Some(Badge::new("failed").tone(BadgeTone::Danger)),
        SessionState::Done => Some(Badge::new("done").tone(BadgeTone::Success)),
        SessionState::Idle => None,
    };
    row()
        .gap(px(SPACE_1_5))
        .overflow_hidden()
        .children(state)
        .child(Badge::new(agent_name(&session.runnable)).icon(IconName::Agent))
        .child(Badge::new(base::fmt::plural(u64::from(session.message_count), "message", "messages")))
        .child(Badge::new(base::fmt::cost(session.cost_usd)))
        .when(hit.children > 0, |d| {
            d.child(
                Badge::new(base::fmt::plural(u64::from(hit.children), "sub-agent", "sub-agents")).tone(BadgeTone::Spawn),
            )
        })
}

/// `mac-mini · payments-api · fix/PAY-1423 · PAY-1423`, in mono.
fn workdir_line(session: &Session) -> gpui::Div {
    let mut parts: Vec<String> = Vec::new();
    if let Some(workdir) = &session.workdir {
        if !workdir.runtime.is_empty() {
            parts.push(workdir.runtime.clone());
        }
        parts.push(workdir.repo.clone());
        parts.extend(workdir.branch.clone());
    }
    parts.extend(session.issue.as_ref().map(ToString::to_string));
    let text = if parts.is_empty() { "No folder".to_string() } else { parts.join(" · ") };
    div().type_style(MONO_SM).text_color(TEXT_MUTED).child(truncate_lh(text, MONO_SM.line))
}
