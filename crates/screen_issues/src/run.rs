//! The review section of the detail panel (before/after renders grouped by the step that
//! made them; approve / request changes while an approval step waits) and the full-size
//! render viewer.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, MouseButton, ObjectFit, SharedString,
    Window, div, img, prelude::*, px, relative,
};
use pipeline::{Render, Run, RunStatus, Stage, StageStatus, StepKind};
use ui::motion::{Pop, presence};
use ui::theme::*;
use ui::{Badge, Button, Status, Tone, field_box, icon};

use crate::IssuesScreen;
use crate::detail::section;
use crate::widgets::{heading, mono, small};

/// Height of a render thumbnail in the panel.
const THUMB_HEIGHT: f32 = 168.;

/// A PNG box: the image fit inside, or a note when there is none.
fn shot(path: Option<&PathBuf>, height: Option<f32>) -> Div {
    let frame = div()
        .flex()
        .items_center()
        .justify_center()
        .w_full()
        .overflow_hidden()
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_color(LINE)
        .bg(BG);
    let frame = match height {
        Some(h) => frame.h(px(h)),
        None => frame.flex_1().min_h_0(),
    };
    match path {
        Some(p) if p.exists() => frame.child(
            img(p.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .with_fallback(|| small("Could not load the image", TEXT_MUTED).into_any_element()),
        ),
        Some(_) => frame.child(small("Render file is missing", TEXT_MUTED)),
        None => frame.child(small("New screen", TEXT_MUTED)),
    }
}

fn caption(text: &'static str, color: gpui::Hsla) -> Div {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(color)
        .child(text)
}

impl IssuesScreen {
    /// What is up for review: the changes you asked for, before/after per file grouped by
    /// the step that rendered them and, while an approval step waits, "Approve" / "Request
    /// changes…".
    pub(crate) fn render_review(
        &self,
        run: &Run,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let waiting = run.status == RunStatus::WaitingApproval && !active;
        let approval = approval_stage(run);
        let badge = if waiting {
            Some(
                Badge::new("Waiting for you")
                    .tone(Tone::Warning)
                    .dot(Status::Waiting),
            )
        } else if active && run.status == RunStatus::WaitingApproval {
            Some(Badge::new("Revising").dot(Status::Running))
        } else if approval.is_some_and(|s| s.status == StageStatus::Done) {
            Some(Badge::new("Approved").tone(Tone::Success).dot(Status::Live))
        } else {
            None
        };
        let back_to = approval.and_then(|a| back_to(run, a));
        let title = approval.map_or_else(|| "Review".to_string(), |a| a.name.clone());

        let n = run.renders.len();
        let mut s = section().when(waiting, |d| d.bg(SURFACE)).child(
            heading(title)
                .children(badge)
                .child(div().flex_1())
                .child(mono(
                    format!("{n} file{}", if n == 1 { "" } else { "s" }),
                    TEXT_MUTED,
                )),
        );
        if !run.feedback.is_empty() {
            s = s.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(small("Changes you asked for", TEXT_MUTED))
                    .children(run.feedback.iter().enumerate().map(|(i, f)| {
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .child(mono(format!("#{}", i + 1), TEXT_MUTED).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(TEXT_SECONDARY)
                                    .child(SharedString::from(f.clone())),
                            )
                    })),
            );
        }
        for (step, ixs) in render_groups(run) {
            let name = run
                .stage(&step)
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Renders".into());
            s = s.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(small(name, TEXT_SECONDARY).font_weight(FontWeight::MEDIUM))
                    .child(small(
                        format!("{} file{}", ixs.len(), if ixs.len() == 1 { "" } else { "s" }),
                        TEXT_MUTED,
                    )),
            );
            for ix in ixs {
                s = s.child(self.render_pair(ix, &run.renders[ix], cx));
            }
        }
        if n == 0 && waiting {
            s = s.child(small("Nothing was rendered for this step.", TEXT_MUTED));
        }
        if waiting {
            s = s.child(self.render_approval_actions(back_to, window, cx));
        }
        s
    }

    fn render_pair(&self, ix: usize, r: &Render, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(("render", ix))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(8.))
            .rounded(px(RADIUS_LG))
            .border_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .cursor_pointer()
            .hover(|s| s.border_color(LINE_STRONG))
            .on_click(cx.listener(move |this, _, _, cx| this.open_viewer(ix, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(mono(r.screen.clone(), TEXT_SECONDARY))
                    .child(div().flex_1())
                    .child(small("Click to enlarge", TEXT_MUTED)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(caption("Before", TEXT_MUTED))
                            .child(shot(r.before.as_ref(), Some(THUMB_HEIGHT))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(caption("After", TEXT_SECONDARY))
                            .child(shot(Some(&r.after), Some(THUMB_HEIGHT))),
                    ),
            )
    }

    /// `back_to`: the step that revises when you request changes.
    fn render_approval_actions(
        &self,
        back_to: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hint = match back_to {
            Some(step) => format!("Tell {step} what to change; the run goes back to it."),
            None => "Say what to change; the run goes back to revise it.".to_string(),
        };
        let notes_empty = self.changes_input.read(cx).text().trim().is_empty();
        let focused = self.changes_input.read(cx).is_focused(window);
        let form = self.changes_open.then(|| {
            ui::motion::reveal(
                "changes-form",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(small(hint, TEXT_MUTED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                field_box()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(CONTROL_HEIGHT))
                                    .border_color(if focused { ACCENT_STRONG } else { LINE_STRONG })
                                    .child(self.changes_input.clone()),
                            )
                            .child(
                                Button::new("changes-send", "Send")
                                    .disabled(notes_empty)
                                    .on_click(cx.listener(|this, _, _, cx| this.send_changes(cx))),
                            ),
                    ),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::new("approve", "Approve")
                            .primary()
                            .icon("check")
                            .on_click(cx.listener(|this, _, _, cx| this.approve(cx))),
                    )
                    .child(
                        Button::new("request-changes", "Request changes…")
                            .selected(self.changes_open)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_changes(window, cx)),
                            ),
                    ),
            )
            .children(form)
    }

    /// Full-size before/after of one render over the screen.
    pub(crate) fn render_viewer(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.viewer.is_some();
        let scrim = presence("rv-scrim", open, Pop::Scrim, window, cx);
        let panel = presence("rv-dialog", open, Pop::Dialog, window, cx);
        let (Some(scrim), Some(panel)) = (scrim, panel) else {
            return None;
        };
        // While leaving there is no selection any more: draw nothing but the scrim.
        let render = self.viewer.as_ref().and_then(|(key, ix)| {
            self.pipes
                .read(cx)
                .with_run(key, |r| (r.renders.get(*ix).cloned(), r.renders.len()))
        });
        let body = render.and_then(|(r, n)| r.map(|r| (r, n))).map(|(r, n)| {
            let ix = self.viewer.as_ref().map(|v| v.1).unwrap_or(0);
            div()
                .id("render-viewer")
                .flex()
                .flex_col()
                .w(relative(0.94))
                .h(relative(0.86))
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
                        .child(icon("file", 14., TEXT_MUTED))
                        .child(mono(r.screen.clone(), TEXT))
                        .child(small(format!("screen {} of {n}", ix + 1), TEXT_MUTED))
                        .child(div().flex_1())
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("rv-close", "")
                                .ghost()
                                .small()
                                .icon("x")
                                .on_click(cx.listener(|this, _, _, cx| this.close_viewer(cx))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .gap(px(SPACE_4))
                        .p(px(SPACE_4))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(6.))
                                .child(
                                    caption("Before", TEXT_MUTED).font_weight(FontWeight::MEDIUM),
                                )
                                .child(shot(r.before.as_ref(), None)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(6.))
                                .child(caption("After", TEXT).font_weight(FontWeight::MEDIUM))
                                .child(shot(Some(&r.after), None)),
                        ),
                )
        });

        Some(
            div()
                .id("render-viewer-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_viewer(cx)),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(gpui::hsla(210. / 360., 0.11, 0.035, 0.72))
                        .opacity(scrim.opacity),
                )
                .children(body.map(|b| b.mt(px(panel.dy)).opacity(panel.opacity)))
                .into_any_element(),
        )
    }
}

/// The approval step waiting now, else the last one that ran.
fn approval_stage(run: &Run) -> Option<&Stage> {
    let approvals = || {
        run.stages
            .iter()
            .filter(|s| s.step_kind() == Some(StepKind::Approval))
    };
    approvals()
        .find(|s| s.status == StageStatus::Waiting)
        .or_else(|| approvals().rfind(|s| s.status != StageStatus::Pending))
}

/// Name of the step an approval sends changes back to.
fn back_to(run: &Run, approval: &Stage) -> Option<String> {
    let id = &run.def.as_ref()?.step(&approval.id)?.back_to;
    run.stage(id).map(|s| s.name.clone())
}

/// Render indices grouped by the step that made them, in step order (unknown steps last).
fn render_groups(run: &Run) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, r) in run.renders.iter().enumerate() {
        match groups.iter_mut().find(|(s, _)| *s == r.step) {
            Some((_, v)) => v.push(i),
            None => groups.push((r.step.clone(), vec![i])),
        }
    }
    let order = |step: &str| {
        run.stages
            .iter()
            .position(|s| s.id == step)
            .unwrap_or(usize::MAX)
    };
    groups.sort_by_key(|(s, _)| order(s));
    groups
}

/// Agent Markdown as plain text for the panel: emphasis and code marks dropped, list
/// markers as bullets, headings as plain lines. (gpui's word-level Markdown layout
/// mis-measures inside this scrolling column.)
pub(crate) fn plain_markdown(md: &str) -> String {
    md.lines()
        .map(|l| {
            let t = l.trim_start();
            let indent = &l[..l.len() - t.len()];
            let t = t.trim_start_matches('#').trim_start();
            let t = match t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                Some(rest) => format!("{indent}• {rest}"),
                None => format!("{indent}{t}"),
            };
            t.replace("**", "").replace("__", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod md_tests {
    #[test]
    fn plain() {
        assert_eq!(
            super::plain_markdown("## Done\n- **a:** `x`\n  - b"),
            "Done\n• a: x\n  • b"
        );
    }
}
