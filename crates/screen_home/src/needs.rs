//! "Needs you": what is waiting on the person, oldest first as hub gives it.

use gpui::{AnyElement, ElementId, InteractiveElement, IntoElement, Context, Window, div, prelude::*, px};
use hub::api::{Need, NeedKind};
use hub::Route;
use ui::motion;
use ui::prelude::*;

use crate::blocks::{Block, empty_view, error_view, footer_link, probed, skeleton};
use crate::view::HomeView;

/// Height of a row (`height: 56px`).
const ROW_HEIGHT_PX: f32 = 56.;

/// The mark in front of a row.
fn mark(kind: NeedKind) -> IconName {
    match kind {
        NeedKind::Permission => IconName::Policy,
        NeedKind::Review => IconName::Inbox,
        NeedKind::Question => IconName::Chat,
        NeedKind::Failure => IconName::Alert,
    }
}

/// The verb on a row's button.
fn verb(kind: NeedKind) -> &'static str {
    match kind {
        NeedKind::Permission => "Answer",
        NeedKind::Review => "Review",
        NeedKind::Question => "Reply",
        NeedKind::Failure => "Open",
    }
}

impl HomeView {
    pub(crate) fn render_needs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = self.now();
        let offset = base::time::utc_offset(now);
        let state = self.state(Some(self.ops.read(cx).needs()));
        let mut header = CardHeader::new("Needs you").dot(Status::Waiting);
        let body = match state {
            Block::Ready(needs) if needs.is_empty() => empty_view("home-needs-empty", "Nothing waits for you", "Agents ask here when they need a decision."),
            Block::Ready(needs) => {
                header = header.count(base::fmt::count(needs.len() as u64));
                if let Some(oldest) = needs.iter().map(|need| need.since).min() {
                    header = header.meta(format!("oldest {} · J/K to move", base::time::ago(now, oldest, offset)));
                }
                let selected = self.selected.filter(|ix| *ix < needs.len());
                col()
                    .children(needs.iter().enumerate().map(|(ix, need)| {
                        self.need_row(ix, need, now, offset, selected == Some(ix))
                    }))
                    .into_any_element()
            }
            Block::Loading => col()
                .children((0..3).map(|ix| {
                    row()
                        .flex_none()
                        .h(px(ROW_HEIGHT_PX))
                        .px(px(SPACE_4))
                        .gap(px(SPACE_3))
                        .border_b_1()
                        .border_color(LINE_SUBTLE)
                        .child(skeleton(24., 24.).rounded(px(RADIUS_MD)))
                        .child(
                            col().gap(px(SPACE_1)).child(skeleton(180. - 20. * ix as f32, 12.)).child(skeleton(120., 10.)),
                        )
                }))
                .into_any_element(),
            Block::Failed(message) => {
                let view = cx.entity().downgrade();
                error_view("what needs you", message, "needs", move |_, cx| {
                    view.update(cx, |view, cx| view.retry(cx)).ok();
                })
            }
        };
        let footer = CardFooter::new().bare().child(
            probed(
                footer_link("home-open-inbox", "Open inbox", window, cx)
                    .on_click(|_, _, cx| hub::navigate(Route::Inbox { item: None }, cx)),
                "home-open-inbox",
            ),
        );
        Card::new().flush().header(header).footer(footer).child(body).into_any_element()
    }

    fn need_row(
        &self,
        ix: usize,
        need: &Need,
        now: i64,
        offset: i32,
        selected: bool,
    ) -> AnyElement {
        let id = ElementId::from(("home-need", ix));
        let target = need.target.clone();
        let el = motion::fade_row(div().id(id.clone()), id, selected, SURFACE_RAISED)
            .flex()
            .flex_none()
            .items_center()
            .h(px(ROW_HEIGHT_PX))
            .px(px(SPACE_4))
            .gap(px(SPACE_3))
            .border_b_1()
            .border_color(LINE_SUBTLE)
            .cursor_pointer()
            .child(Tile::new(mark(need.kind)).tone(TileTone::Warn))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().type_style(BODY_STRONG).child(title(&need.title)))
                    .child(
                        div()
                            .flex()
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child(truncate_lh(need.source.clone(), LABEL.line)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .type_style(MONO)
                    .text_color(TEXT_MUTED)
                    .tabular()
                    .child(base::time::ago(now, need.since, offset)),
            )
            // The row takes the click; the button is its label.
            .child(Button::new(ElementId::from(("home-need-button", ix)), verb(need.kind)).small())
            .on_click(move |_, _, cx| hub::open(target.clone(), cx));
        probed(el, format!("home-need-{ix}")).into_any_element()
    }
}

/// A title in one line; a `code` span in backticks is drawn as code.
fn title(text: &str) -> AnyElement {
    if !text.contains('`') {
        return div().flex().child(truncate(text.to_string())).into_any_element();
    }
    row()
        .h(px(BODY_STRONG.line))
        .gap(px(SPACE_1_5))
        .overflow_hidden()
        .children(text.split('`').enumerate().filter(|(_, part)| !part.trim().is_empty()).map(|(ix, part)| {
            if ix % 2 == 1 {
                Code::new(part.to_string()).into_any_element()
            } else {
                div().flex_none().whitespace_nowrap().child(part.trim().to_string()).into_any_element()
            }
        }))
        .into_any_element()
}
