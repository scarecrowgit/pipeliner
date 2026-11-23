//! "New issue" dialog: title, description, kind, pipeline and "Run the pipeline now".

use gpui::{
    AnyElement, Context, Focusable, FontWeight, IntoElement, MouseButton, SharedString, Window,
    div, prelude::*, px, relative,
};
use ui::motion::{Pop, presence};
use ui::theme::*;
use ui::{Button, checkbox, field_box, kbd};

use crate::IssuesScreen;
use crate::look::KINDS;
use crate::menu::Menu;
use crate::widgets::small;

/// Rows of the description box.
const BODY_ROWS: f32 = 4.;

fn field_label(text: &'static str) -> gpui::Div {
    div()
        .mb(px(6.))
        .text_size(px(TEXT_LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(TEXT_SECONDARY)
        .child(text)
}

impl IssuesScreen {
    pub(crate) fn open_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new.open = true;
        self.new.error = None;
        self.new.body_editing = false;
        window.focus(&self.new.title.focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn close_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.new.open {
            return;
        }
        self.new.open = false;
        self.new.body_editing = false;
        if self.menu == Some(Menu::NewIssue) {
            self.menu = None;
        }
        self.focus_self(window);
        cx.notify();
    }

    pub(crate) fn edit_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new.body_editing = true;
        window.focus(&self.new.body.focus_handle(cx));
        cx.notify();
    }

    fn set_kind(&mut self, kind: usize, cx: &mut Context<Self>) {
        self.new.kind = kind;
        cx.notify();
    }

    fn toggle_run_now(&mut self, cx: &mut Context<Self>) {
        self.new.run_now = !self.new.run_now;
        cx.notify();
    }

    pub(crate) fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.new.title.read(cx).text().trim().to_string();
        if title.is_empty() {
            self.new.error = Some("Give the issue a title.".into());
            window.focus(&self.new.title.focus_handle(cx));
            cx.notify();
            return;
        }
        let body = self.new.body.read(cx).text().to_string();
        let kind = KINDS[self.new.kind.min(KINDS.len() - 1)];
        let start = self.new.run_now;
        let pipeline = self.new.pipeline.clone();
        let made = self.pipes.update(cx, |p, cx| {
            p.create_issue(&title, &body, kind, pipeline.as_deref(), start, cx)
        });
        match made {
            Ok(key) => {
                self.new.title.update(cx, |i, cx| i.set_text("", cx));
                self.new.body.update(cx, |i, cx| i.set_text("", cx));
                self.close_new(window, cx);
                self.select(key.into(), window, cx);
            }
            Err(e) => {
                self.new.error = Some(format!("Could not save the issue: {e:#}").into());
                cx.notify();
            }
        }
    }

    /// The dialog with its scrim; `None` once closed and its exit has played.
    pub(crate) fn render_new(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let scrim = presence("ni-scrim", self.new.open, Pop::Scrim, window, cx);
        let panel = presence("ni-dialog", self.new.open, Pop::Dialog, window, cx);
        let (Some(scrim), Some(panel)) = (scrim, panel) else {
            return None;
        };
        let title_focused = self.new.title.read(cx).is_focused(window);
        let body_text = self.new.body.read(cx).text().to_string();
        let can_create = !self.new.title.read(cx).text().trim().is_empty();

        let title = div().child(field_label("Title")).child(
            field_box()
                .border_color(if title_focused {
                    ACCENT_STRONG
                } else {
                    LINE_STRONG
                })
                .child(self.new.title.clone()),
        );

        // Multi-line look: wrapped text at rest, the (single-line) input while editing.
        let editing = self.new.body_editing;
        let body_box = div()
            .id("ni-body")
            .min_h(px(BODY_ROWS * LINE_BODY + 22.))
            .px(px(12.))
            .py(px(10.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(if editing { ACCENT_STRONG } else { LINE_STRONG })
            .rounded(px(RADIUS_LG))
            .line_height(px(LINE_BODY))
            .cursor_text()
            .when(editing, |d| {
                d.child(div().flex().h(px(LINE_BODY)).child(self.new.body.clone()))
            })
            .when(!editing, |d| {
                d.child(if body_text.is_empty() {
                    div()
                        .text_color(TEXT_MUTED)
                        .child("What should change, and why")
                } else {
                    div().text_color(TEXT).child(SharedString::from(body_text))
                })
            })
            .on_click(cx.listener(|this, _, window, cx| this.edit_body(window, cx)));
        let body = div()
            .child(field_label("Description"))
            .child(body_box)
            .child(div().mt(px(6.)).child(small(
                "Markdown. The pipeline's agents read it as written.",
                TEXT_MUTED,
            )));

        let kinds = div().child(field_label("Kind")).child(
            div()
                .flex()
                .gap(px(4.))
                .p(px(3.))
                .rounded(px(RADIUS_LG))
                .border_1()
                .border_color(LINE_STRONG)
                .bg(SURFACE_RAISED)
                .children(KINDS.iter().enumerate().map(|(i, k)| {
                    let on = self.new.kind == i;
                    div()
                        .id(("ni-kind", i))
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .h(px(CONTROL_HEIGHT_SM))
                        .rounded(px(RADIUS_MD))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .when(on, |d| {
                            d.bg(SURFACE_HOVER)
                                .text_color(TEXT)
                                .font_weight(FontWeight::MEDIUM)
                        })
                        .when(!on, |d| {
                            d.text_color(TEXT_MUTED)
                                .hover(|s| s.text_color(TEXT_SECONDARY))
                        })
                        .child(*k)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_kind(i, cx)))
                })),
        );

        let (first_step, problems) = {
            let p = self.pipes.read(cx);
            let def = self
                .new
                .pipeline
                .as_deref()
                .and_then(|id| p.pipeline(id))
                .or(p.default_pipeline());
            (
                def.and_then(|d| d.steps.first()).map(|s| s.name.clone()),
                def.map(|d| d.problems().len()).unwrap_or(0),
            )
        };
        let pipeline = div()
            .child(field_label("Pipeline"))
            .child(self.pipeline_select(Menu::NewIssue, self.new.pipeline.as_deref(), cx))
            .when(problems > 0, |d| {
                d.child(div().mt(px(6.)).child(small(
                    format!(
                        "This pipeline has {problems} problem{} to fix in the editor before it can run.",
                        if problems == 1 { "" } else { "s" }
                    ),
                    WARNING,
                )))
            });
        let run_hint = match first_step {
            Some(step) => format!("Its first step, {step}, starts right away."),
            None => "The pipeline has no steps yet.".to_string(),
        };

        let run_now = div()
            .id("ni-run-now")
            .flex()
            .items_start()
            .gap(px(10.))
            .cursor_pointer()
            .child(
                div()
                    .pt(px(2.))
                    .child(checkbox("ni-run-box", self.new.run_now)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().child("Run the pipeline now"))
                    .child(small(run_hint, TEXT_MUTED)),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_run_now(cx)));

        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(SPACE_5))
            .py(px(SPACE_3))
            .border_t_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(kbd("↵"))
            .child(small("next field / create", TEXT_MUTED))
            .child(kbd("esc"))
            .child(small("close", TEXT_MUTED))
            .child(div().flex_1())
            .child(
                Button::new("ni-cancel", "Cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.close_new(window, cx))),
            )
            .child(
                Button::new(
                    "ni-create",
                    if self.new.run_now {
                        "Create and run"
                    } else {
                        "Create issue"
                    },
                )
                .primary()
                .disabled(!can_create)
                .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
            );

        let dialog = div()
            .id("new-issue")
            .flex()
            .flex_col()
            .w(px(560.))
            .max_w(relative(0.94))
            .rounded(px(12.))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE)
            .overflow_hidden()
            .shadow(popover_shadow())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(SPACE_5))
                    .py(px(SPACE_3))
                    .border_b_1()
                    .border_color(LINE)
                    .child(
                        div()
                            .text_size(px(TEXT_HEADING))
                            .font_weight(FontWeight::MEDIUM)
                            .child("New issue"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("ni-close", "")
                            .ghost()
                            .small()
                            .icon("x")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_new(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_4))
                    .p(px(SPACE_5))
                    .child(title)
                    .child(body)
                    .child(kinds)
                    .child(pipeline)
                    .child(run_now)
                    .when_some(self.new.error.clone(), |d, e| d.child(small(e, DANGER))),
            )
            .child(footer);

        Some(
            div()
                .id("new-issue-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(96. + panel.dy))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_new(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(gpui::hsla(210. / 360., 0.11, 0.035, 0.72))
                        .opacity(scrim.opacity),
                )
                .child(dialog.opacity(panel.opacity))
                .into_any_element(),
        )
    }
}
