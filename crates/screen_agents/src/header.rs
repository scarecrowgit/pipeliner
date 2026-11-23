//! Page header: title, tagline and the primary "New" button with its menu (new issue for the
//! pipeline, new chat).

use gpui::{Context, FontWeight, IntoElement, deferred, div, prelude::*, px};
use ui::Button;
use ui::nav::{ChatRequest, Screen, navigate, request_chat};
use ui::theme::*;

use crate::AgentsScreen;
use crate::looks::tile;

/// Items of the "New" menu.
#[derive(Clone, Copy)]
enum NewItem {
    Issue,
    Chat,
}

impl NewItem {
    const ALL: [NewItem; 2] = [NewItem::Issue, NewItem::Chat];

    fn name(self) -> &'static str {
        match self {
            NewItem::Issue => "New issue",
            NewItem::Chat => "New chat",
        }
    }

    fn desc(self) -> &'static str {
        match self {
            NewItem::Issue => "On the board; a pipeline runs it",
            NewItem::Chat => "Talk to Build, Plan or a custom agent",
        }
    }

    fn looks(self) -> (&'static str, gpui::Hsla, gpui::Hsla) {
        match self {
            NewItem::Issue => ("pipelines", INFO_SOFT, INFO_TEXT),
            NewItem::Chat => ("agent", SURFACE_HOVER, TEXT_SECONDARY),
        }
    }
}

impl AgentsScreen {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.new_open;
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .min_h(px(HEADER_HEIGHT))
            .px(px(SPACE_6))
            .py(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .text_size(px(TEXT_HEADING))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Agents & pipelines"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("Your pipelines, their agents and sub-agents, your chat agents and what they created."),
            )
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .child(
                        Button::new("agents-new", "New")
                            .primary()
                            .icon("plus")
                            .on_click(cx.listener(move |this, _, _, cx| this.set_new_open(!open, cx))),
                    )
                    .when(open, |d| {
                        d.child(
                            deferred(ui::motion::pop_in(
                                "agents-new-menu-in",
                                ui::motion::Pop::Below,
                                self.render_new_menu(cx),
                            ))
                            .with_priority(1),
                        )
                    }),
            )
    }

    fn render_new_menu(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("agents-new-menu")
            .absolute()
            .right_0()
            .top(px(CONTROL_HEIGHT + 6.))
            .w(px(300.))
            .p(px(4.))
            .flex()
            .flex_col()
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE_RAISED)
            .shadow(popover_shadow())
            .occlude()
            .children(NewItem::ALL.iter().enumerate().map(|(i, &item)| {
                let (icon, bg, fg) = item.looks();
                div()
                    .id(("agents-new-kind", i))
                    .flex()
                    .gap(px(10.))
                    .p(px(8.))
                    .rounded(px(RADIUS_MD))
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .child(tile(icon, 28., RADIUS_MD, 14., bg, fg))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::MEDIUM).child(item.name()))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .text_color(TEXT_MUTED)
                                    .child(item.desc()),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_new_open(false, cx);
                        match item {
                            NewItem::Issue => navigate(Screen::Issues, cx),
                            NewItem::Chat => request_chat(ChatRequest::New, cx),
                        }
                    }))
            }))
    }
}
