//! One result of the list: state dot, title with the match marked, the line of agent, repo,
//! branch and issue, the matching snippet, and the state, age and cost at the right.

use gpui::{AnyElement, Context, IntoElement, MouseMoveEvent, div, prelude::*, px};
use hub::api::{SessionHit, SessionState};
use ui::dev::Probe as _;
use ui::prelude::*;

use crate::groups::local_offset;
use crate::labels::{age_and_cost, state_word, status, sub_line};
use crate::palette::Palette;
use crate::snippet;

/// The colour of a state's word.
pub(crate) fn state_color(state: SessionState) -> gpui::Hsla {
    match state {
        SessionState::Waiting => WARNING,
        SessionState::Running => INFO_TEXT,
        SessionState::Failed => DANGER,
        SessionState::Done => SUCCESS,
        SessionState::Idle => TEXT_MUTED,
    }
}

impl Palette {
    /// Row `row` of the list for `hit`. The selected row sits on `surface-hover` with an accent
    /// bar at its left edge; the pointer moves the selection, a click opens the chat.
    pub(crate) fn result_row(&self, row: usize, hit: &SessionHit, cx: &mut Context<Self>) -> AnyElement {
        let selected = row == self.selected;
        let session = &hit.session;
        let now = self.clock();

        let title = if hit.title_highlights.is_empty() {
            truncate_lh(session.title.clone(), BODY.line)
        } else {
            snippet::line(session.title.clone(), &hit.title_highlights, BODY.line)
        };
        let title_row = row_title(title, hit.children);
        let sub = truncate_lh(sub_line(session), MONO_SM.line).type_style(MONO_SM).text_color(TEXT_MUTED);
        let matched = hit.snippet.as_ref().map(|found| {
            let found = snippet::window(&found.text, &found.highlights);
            snippet::line(found.text, &found.marks, LABEL_TIGHT.line).type_style(LABEL_TIGHT).text_color(TEXT_SECONDARY)
        });
        let state = state_word(session.state).map(|word| {
            div().type_style(LABEL).text_color(state_color(session.state)).whitespace_nowrap().child(word)
        });

        let el = div()
            .id(("palette-row", row))
            .relative()
            .flex()
            .flex_row()
            .items_start()
            .flex_none()
            .w_full()
            .gap(px(SPACE_2_5))
            .px(px(SPACE_3))
            .py(px(SPACE_2))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .debug_selector(move || format!("palette-row-{row}"));
        motion::fade_row(el, ("palette-row", row), selected, SURFACE_RAISED)
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| this.select(row, false, cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(row, false, cx);
                this.open_selected(cx);
            }))
            .when(selected, |d| {
                d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(ACCENT))
            })
            .child(div().flex_none().mt(px(6.)).child(StatusDot::new(status(session.state))))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(SPACE_1))
                    .child(title_row)
                    .child(sub)
                    .children(matched),
            )
            .child(
                col()
                    .flex_none()
                    .items_end()
                    .gap(px(SPACE_0_5))
                    .children(state)
                    .child(
                        div()
                            .type_style(MONO_SM)
                            .text_color(TEXT_MUTED)
                            .tabular()
                            .whitespace_nowrap()
                            .child(age_and_cost(session, now, local_offset(now))),
                    ),
            )
            .probe(format!("palette-row-{row}"))
            .into_any_element()
    }
}

/// The title line: the title, then the violet "+2 sub" tag when sub-agents worked for it.
fn row_title(title: gpui::Div, children: u32) -> gpui::Div {
    row()
        .gap(px(SPACE_1_5))
        .type_style(BODY_STRONG)
        .text_color(TEXT)
        .child(title)
        .when(children > 0, |d| d.child(Tag::new(format!("+{children} sub")).tone(TagTone::Spawn)))
}
