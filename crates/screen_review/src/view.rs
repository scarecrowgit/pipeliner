//! Render parts of the review screen: header, queue list, issue main column.

use gpui::{
    AnyElement, Context, Focusable, FontWeight, Hsla, IntoElement, MouseButton, SharedString, div,
    prelude::*, px,
};
use ui::nav::{self, Screen};
use ui::theme::*;
use ui::{Badge, Button, Status, Tone, icon, icon_tile, kbd, mono, spawn_chip, status_dot};

use crate::data::{ISSUES, Variant};
use crate::{Done, ReviewScreen};

/// Monospace `+18 −4` pair.
fn add_del(add: &'static str, del: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .gap(px(6.))
        .font_family(FONT_MONO)
        .text_size(px(TEXT_MONO))
        .child(div().text_color(SUCCESS).child(add))
        .child(div().text_color(DANGER).child(del))
}

fn spacer() -> gpui::Div {
    div().flex_1()
}

impl ReviewScreen {
    // ── Header ──────────────────────────────────────────────────────────

    pub(crate) fn render_header(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let chevron = || icon("chevron-right", 12., TEXT_DISABLED);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(SPACE_3))
            .min_h(px(HEADER_HEIGHT))
            .px(px(SPACE_6))
            .py(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(ACCENT_SOFT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("pipelines", 16., ACCENT_TEXT)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(TEXT_HEADING))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(div().text_color(TEXT_MUTED).child("Inbox"))
                    .child(chevron())
                    .child(
                        div()
                            .id("review-crumb-pipeline")
                            .text_color(TEXT_MUTED)
                            .cursor_pointer()
                            .hover(|s| s.text_color(TEXT))
                            .on_click(|_, _, cx| nav::navigate(Screen::PipelineEditor, cx))
                            .child("PAY triage"),
                    )
                    .child(chevron())
                    .child(div().text_color(TEXT).child("Review")),
            )
            .child(
                Badge::new(format!("{} waiting", self.waiting_variants()))
                    .tone(Tone::Warning)
                    .dot(Status::Waiting),
            )
            .child(spacer())
            .child(
                Button::new("review-back-to-editor", "Back to editor")
                    .ghost()
                    .icon("pipelines")
                    .on_click(|_, _, cx| nav::navigate(Screen::PipelineEditor, cx)),
            )
    }

    // ── Queue (left list) ───────────────────────────────────────────────

    pub(crate) fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = ISSUES.iter().enumerate().map(|(i, issue)| {
            let current = i == self.cur;
            let (status, state, state_color) = match self.done[i] {
                Some(Done::Shipped) => (Status::Live, "shipped", SUCCESS),
                Some(Done::Back) => (Status::Running, "re-running", INFO),
                None => (Status::Waiting, "waiting", WARNING),
            };
            let n = issue.variants.len();
            let meta = format!(
                "{} · {} {}",
                issue.agent,
                n,
                if n > 1 { "variants" } else { "variant" }
            );
            div()
                .id(("review-item", i))
                .flex()
                .flex_none()
                .items_start()
                .gap(px(10.))
                .w_full()
                .p(px(10.))
                .border_1()
                .rounded(px(RADIUS_LG))
                .cursor_pointer()
                .when(current, |d| d.border_color(LINE_STRONG).bg(SURFACE_HOVER))
                .when(!current, |d| {
                    d.border_color(gpui::transparent_black())
                        .hover(|s| s.bg(SURFACE_RAISED))
                })
                .on_click(cx.listener(move |this, _, window, cx| this.open(i, window, cx)))
                .child(div().mt(px(7.)).flex_none().child(status_dot(status, true)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(SPACE_2))
                                .child(mono(issue.key).text_color(TEXT_MUTED))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(state_color)
                                        .child(state),
                                ),
                        )
                        .child(div().font_weight(FontWeight::MEDIUM).child(issue.title))
                        .child(div().text_size(px(12.)).text_color(TEXT_MUTED).child(meta)),
                )
        });

        let hint = |keys: AnyElement, text: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(keys)
                .child(text)
        };

        div()
            .id("review-queue")
            .w(px(280.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(SPACE_3))
            .py(px(SPACE_4))
            .bg(BG_SIDEBAR)
            .border_r_1()
            .border_color(LINE)
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(SPACE_2))
                    .pb(px(SPACE_2))
                    .child(
                        div()
                            .text_size(px(TEXT_HEADING))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Waiting for you"),
                    )
                    .child(Badge::new(self.waiting_issues().to_string()).tone(Tone::Count)),
            )
            .children(items)
            .child(
                div()
                    .mt_auto()
                    .pt(px(SPACE_3))
                    .px(px(SPACE_2))
                    .flex()
                    .flex_none()
                    .flex_wrap()
                    .gap_x(px(SPACE_3))
                    .gap_y(px(6.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(hint(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(kbd("J"))
                            .child(kbd("K"))
                            .into_any_element(),
                        "move",
                    ))
                    .child(hint(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(kbd("1"))
                            .child("–")
                            .child(kbd("3"))
                            .into_any_element(),
                        "pick",
                    ))
                    .child(hint(kbd("R").into_any_element(), "send back")),
            )
    }

    // ── Main column ─────────────────────────────────────────────────────

    pub(crate) fn render_main(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("review-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_5))
                    .p(px(SPACE_6))
                    .w_full()
                    .max_w(px(1080.))
                    .child(self.render_issue_head())
                    .when(self.is_shipped(), |d| {
                        d.child(self.render_shipped_banner(cx))
                    })
                    .child(self.render_variants(cx))
                    .child(self.render_diff())
                    .when(self.sending_back, |d| d.child(self.render_send_back(cx)))
                    .child(self.render_footer(cx)),
            )
    }

    fn render_issue_head(&self) -> impl IntoElement {
        let issue = self.issue();
        let prio_tone = if issue.prio == "High" {
            Tone::Warning
        } else {
            Tone::Neutral
        };
        let last = issue.path.len() - 1;
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_2))
                    .child(mono(issue.key).text_color(TEXT_MUTED))
                    .child(Badge::new(issue.kind))
                    .child(Badge::new(issue.prio).tone(prio_tone)),
            )
            .child(
                div()
                    .text_size(px(TEXT_TITLE_LG))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(issue.title),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO))
                    .text_color(TEXT_MUTED)
                    .children(issue.path.iter().enumerate().map(|(i, step)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_color(if i == last { WARNING } else { TEXT_MUTED })
                                    .child(*step),
                            )
                            .when(i < last, |d| {
                                d.child(div().text_color(TEXT_DISABLED).child("›"))
                            })
                    })),
            )
            .child(div().flex().child(spawn_chip(issue.spawn)))
    }

    fn render_shipped_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_3))
            .px(px(14.))
            .py(px(SPACE_3))
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .child(icon("check", 16., SUCCESS))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(4.))
                    .child(
                        div()
                            .text_color(SUCCESS)
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Shipped {}.", self.picked_variant().id)),
                    )
                    .child(div().text_color(TEXT_SECONDARY).child(self.issue().pr)),
            )
            .child(
                Button::new("review-next-issue", "Next issue")
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.next_issue(window, cx))),
            )
    }

    fn render_variants(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let picked = self.picked_index();
        div().flex().flex_wrap().gap(px(SPACE_3)).children(
            self.issue()
                .variants
                .iter()
                .enumerate()
                .map(|(i, v)| self.render_variant_card(i, v, i == picked, cx)),
        )
    }

    fn render_variant_card(
        &self,
        index: usize,
        v: &'static Variant,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let stat = |name: &'static str, value: gpui::Div| {
            div()
                .flex()
                .justify_between()
                .gap(px(SPACE_2))
                .child(div().text_color(TEXT_MUTED).child(name))
                .child(value.font_family(FONT_MONO))
        };
        let text = |s: &'static str, color: Hsla| div().text_color(color).child(s);
        let tests_color = if v.failed { DANGER } else { SUCCESS };

        ui::card()
            .flex_1()
            .flex_basis(px(240.))
            .min_w(px(240.))
            .when(selected, |d| {
                d.border_color(ACCENT_RING).shadow(selected_glow())
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .child(icon_tile("agent", Tone::Accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(format!("{} · {}", v.id, v.name))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child(v.cost),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .child(stat("Lines", add_del(v.add, v.del).gap(px(7.))))
                    .child(stat("Tests", text(v.tests, tests_color)))
                    .child(stat("New tests", text(v.new_tests, SPAWN_TEXT)))
                    .child(stat("Time", text(v.time, TEXT))),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_SECONDARY)
                    .child(v.note),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .when(selected, |d| {
                        d.child(Badge::new(format!("Selected · {}", index + 1)).tone(Tone::Accent))
                    })
                    .when(!selected, |d| {
                        d.child(
                            Button::new(("review-select", index), format!("Select {}", v.id))
                                .small()
                                .disabled(self.is_done())
                                .on_click(cx.listener(move |this, _, _, cx| this.pick(index, cx))),
                        )
                    }),
            )
    }

    fn render_diff(&self) -> impl IntoElement {
        let v = self.picked_variant();
        let lines = v.lines.iter().map(|&(sign, code)| {
            let (glyph, bg, color, sign_color) = match sign {
                '+' => ("+", DIFF_ADD_BG, DIFF_ADD_TEXT, DIFF_ADD_TEXT),
                '-' => ("−", DIFF_DEL_BG, DIFF_DEL_TEXT, DIFF_DEL_TEXT),
                _ => (" ", gpui::transparent_black(), TEXT_SECONDARY, TEXT_MUTED),
            };
            div()
                .flex()
                .flex_none()
                .gap(px(SPACE_3))
                .min_w_full()
                .min_h(px(20.))
                .px(px(14.))
                .bg(bg)
                .text_color(color)
                .whitespace_nowrap()
                .child(
                    div()
                        .w(px(12.))
                        .flex_none()
                        .text_color(sign_color)
                        .child(glyph),
                )
                .child(div().flex_none().child(code))
        });

        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_2))
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(LINE)
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Variant {}", v.id)),
                    )
                    .child(mono(v.file).text_color(TEXT_MUTED))
                    .child(spacer())
                    .child(add_del(v.add, v.del)),
            )
            .child(
                div()
                    .id("review-diff-lines")
                    .flex()
                    .flex_col()
                    .overflow_x_scroll()
                    .bg(BG_SIDEBAR)
                    .py(px(SPACE_2))
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO))
                    .line_height(px(20.))
                    .children(lines),
            )
    }

    fn render_send_back(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .p(px(14.))
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .child(ui::label(format!(
                "Notes for {} · it re-runs with these",
                self.issue().agent
            )))
            .child(
                div()
                    .id("review-notes")
                    .flex()
                    .items_start()
                    .w_full()
                    .min_h(px(82.))
                    .px(px(SPACE_3))
                    .py(px(10.))
                    .bg(SURFACE_RAISED)
                    .border_1()
                    .border_color(LINE_STRONG)
                    .rounded(px(RADIUS_LG))
                    .text_color(TEXT)
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.focus(&this.notes_input.focus_handle(cx));
                        }),
                    )
                    .child(self.notes_input.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(SPACE_2))
                    .child(
                        Button::new("review-send-back-confirm", "Send back with notes")
                            .icon("replay")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_send_back(window, cx)
                            })),
                    )
                    .child(
                        Button::new("review-send-back-cancel", "Cancel")
                            .ghost()
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.cancel_send_back(window, cx)
                                }),
                            ),
                    ),
            )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.picked_variant();
        let shipped = self.is_shipped();
        let hint = if v.failed {
            format!("Variant {} has a failing test", v.id)
        } else {
            format!("{} · {}", v.cost, v.time)
        };
        let ship_label = if shipped {
            format!("Shipped {}", v.id)
        } else {
            format!("Ship {}", v.id)
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_2))
            .pt(px(SPACE_4))
            .border_t_1()
            .border_color(LINE)
            .child(
                Button::new("review-send-back", "Send back…")
                    .disabled(self.is_done())
                    .icon("replay")
                    .on_click(cx.listener(|this, _, window, cx| this.start_send_back(window, cx))),
            )
            .child(
                Button::new("review-skip", "Skip")
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.next_issue(window, cx))),
            )
            .child(spacer())
            .child(div().text_size(px(12.)).text_color(TEXT_MUTED).child(hint))
            .child(
                Button::new("review-ship", ship_label)
                    .primary()
                    .icon("check")
                    .disabled(self.is_done())
                    .on_click(cx.listener(|this, _, window, cx| this.ship(window, cx))),
            )
    }
}
