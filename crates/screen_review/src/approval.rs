//! Mockup approvals in the Inbox: the pipeline stopped after the designer's round and waits
//! for the user — renders before / after per changed screen, the designer's summary and
//! earlier change requests, "Approve mockup" or "Request changes…".

use std::path::PathBuf;

use gpui::{
    Focusable as _,
    AnyElement, Context, FontWeight, IntoElement, ObjectFit, SharedString, div, img, prelude::*,
    px,
};
use ui::nav;
use ui::theme::*;
use ui::{Badge, Button, Status, Tone, icon, mono, status_dot};

use crate::ReviewScreen;

/// What the approval view needs from the run.
struct Snapshot {
    key: String,
    title: String,
    body: String,
    /// Decisions made so far (`Triage: yes · …`).
    reason: String,
    /// What the step the approval sends back to reported (the designer's summary).
    summary: String,
    /// Name of the waiting step and of the step it sends back to.
    step: String,
    back_to: String,
    /// Names of the steps done before it.
    path: Vec<String>,
    feedback: Vec<String>,
    renders: Vec<(String, Option<PathBuf>, PathBuf)>,
    round: usize,
    created: u64,
    cost: f64,
}

fn ago(t: u64) -> String {
    let now = ui::time::now_secs();
    ui::time::when_short(t, now, ui::time::utc_offset(now))
}

/// Agent Markdown as plain text (emphasis and code marks dropped, bullets kept).
fn plain(md: &str) -> String {
    md.lines()
        .map(|l| {
            let t = l.trim_start();
            let indent = &l[..l.len() - t.len()];
            let t = t.trim_start_matches('#').trim_start();
            let t = match t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                Some(rest) => format!("{indent}• {rest}"),
                None => format!("{indent}{t}"),
            };
            t.replace("**", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `(width, height)` of a PNG from its header.
fn png_size(path: &std::path::Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    (&head[1..4] == b"PNG").then(|| {
        let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
        let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
        (w, h)
    })
}

/// Frame height for a render: tall screens (the sidebar) get room to be readable.
fn frame_height(path: Option<&PathBuf>) -> f32 {
    match path.and_then(|p| png_size(p)) {
        Some((w, h)) if h > w => (h as f32).min(720.),
        _ => 300.,
    }
}

fn picture(path: Option<&PathBuf>, label: &'static str, height: f32) -> gpui::Div {
    let frame = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(TEXT_LABEL))
                .text_color(TEXT_MUTED)
                .child(label),
        );
    let image = div()
        .h(px(height))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_color(LINE)
        .bg(BG);
    let image = match path {
        Some(p) if p.exists() => image.child(
            img(p.clone())
                .size_full()
                .object_fit(ObjectFit::Contain),
        ),
        Some(_) => image.child(div().text_color(TEXT_MUTED).child("Render file is missing")),
        None => image.child(div().text_color(TEXT_MUTED).child("New screen")),
    };
    frame.child(image)
}

impl ReviewScreen {
    fn snapshot(&self, key: &str, cx: &gpui::App) -> Option<Snapshot> {
        let p = self.pipes.as_ref()?.read(cx);
        let issue = p.issue(key)?.clone();
        p.with_run(key, |r| {
            let waiting = r.stages.get(r.cursor);
            let step_def = r
                .def
                .as_ref()
                .and_then(|d| waiting.and_then(|w| d.step(&w.id)).cloned());
            let back = step_def.as_ref().map(|s| s.back_to.clone()).unwrap_or_default();
            let back_name = r.stage(&back).map(|s| s.name.clone()).unwrap_or_default();
            Snapshot {
            key: issue.key.clone(),
            title: issue.title.clone(),
            body: issue.body.clone(),
            reason: r
                .decisions
                .iter()
                .map(|d| {
                    let name = r.stage(&d.step).map(|s| s.name.clone()).unwrap_or_default();
                    format!("{name}: {} · {}", if d.yes { "yes" } else { "no" }, d.reason)
                })
                .collect::<Vec<_>>()
                .join("\n"),
            summary: r.notes.get(&back).cloned().unwrap_or_default(),
            step: waiting.map(|w| w.name.clone()).unwrap_or_default(),
            back_to: back_name,
            path: r
                .stages
                .iter()
                .take(r.cursor)
                .filter(|s| s.status == pipeline::StageStatus::Done)
                .map(|s| s.name.clone())
                .collect(),
            feedback: r.feedback.clone(),
            renders: r
                .renders
                .iter()
                .map(|x| (x.screen.clone(), x.before.clone(), x.after.clone()))
                .collect(),
            round: r.feedback.len() + 1,
            created: waiting.map(|w| w.started).filter(|t| *t > 0).unwrap_or(r.created),
            cost: r.cost_usd,
            }
        })
    }

    pub(crate) fn approve(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(p)) = (self.approval.clone(), self.pipes.clone()) else { return };
        p.update(cx, |p, cx| p.approve(&key, cx));
        self.approval = None;
        self.changes_open = false;
        self.publish_count(cx);
        cx.notify();
    }

    pub(crate) fn send_changes(&mut self, cx: &mut Context<Self>) {
        let notes = self.changes_input.read(cx).text().trim().to_string();
        let (Some(key), Some(p)) = (self.approval.clone(), self.pipes.clone()) else { return };
        if notes.is_empty() {
            return;
        }
        p.update(cx, |p, cx| p.request_changes(&key, &notes, cx));
        self.changes_input.update(cx, |i, cx| i.set_text("", cx));
        self.changes_open = false;
        self.approval = None;
        self.publish_count(cx);
        cx.notify();
    }

    // ── Queue rows ──────────────────────────────────────────────────────

    pub(crate) fn render_approval_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.approvals(cx)
            .into_iter()
            .enumerate()
            .filter_map(|(i, key)| {
                let s = self.snapshot(&key, cx)?;
                let current = self.approval.as_deref() == Some(key.as_str());
                let (rest, hover) = if current {
                    let l = ui::motion::Look::bg(SURFACE_HOVER).border(LINE_STRONG);
                    (l, l)
                } else {
                    let clear = gpui::transparent_black();
                    (
                        ui::motion::Look::bg(clear).border(clear),
                        ui::motion::Look::bg(SURFACE_RAISED).border(clear),
                    )
                };
                let fade_key = gpui::ElementId::Name(format!("inbox-approval-{key}").into());
                let n = s.renders.len();
                Some(
                    ui::motion::fade(div().id(("inbox-approval", i)), fade_key, rest, hover)
                        .flex()
                        .flex_none()
                        .items_start()
                        .gap(px(10.))
                        .w_full()
                        .p(px(10.))
                        .border_1()
                        .rounded(px(RADIUS_LG))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_approval(key.clone(), cx)
                        }))
                        .child(div().mt(px(7.)).flex_none().child(status_dot(Status::Waiting, true)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .gap(px(2.))
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(SPACE_2))
                                        .child(mono(s.key.clone()).text_color(TEXT_MUTED))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(WARNING)
                                                .child("needs approval"),
                                        ),
                                )
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .line_clamp(2)
                                        .child(s.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(TEXT_MUTED)
                                        .child(format!(
                                            "{} · {n} file{} · round {} · {}",
                                            s.step,
                                            if n == 1 { "" } else { "s" },
                                            s.round,
                                            ago(s.created)
                                        )),
                                ),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    // ── Approval view ───────────────────────────────────────────────────

    pub(crate) fn render_approval(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.snapshot(key, cx) else {
            return div()
                .text_color(TEXT_MUTED)
                .child("This approval is no longer waiting.")
                .into_any_element();
        };
        let open_key = s.key.clone();
        let head = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_2))
                    .child(mono(s.key.clone()).text_color(TEXT_MUTED))
                    .child(Badge::new("needs your approval").tone(Tone::Warning).dot(Status::Waiting))
                    .when(!s.back_to.is_empty(), |d| {
                        d.child(Badge::new(format!("changes go back to {}", s.back_to)))
                    })
                    .child(Badge::new(format!("round {}", s.round))),
            )
            .child(
                div()
                    .text_size(px(TEXT_TITLE_LG))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s.title.clone()),
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
                    .children(s.path.iter().map(|n| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(n.clone())
                            .child(div().text_color(TEXT_DISABLED).child("›"))
                    }))
                    .child(div().text_color(WARNING).child(s.step.clone()))
                    .child(div().text_color(TEXT_DISABLED).child("·"))
                    .child(ago(s.created))
                    .when(s.cost > 0., |d| d.child(format!("· ${:.2} so far", s.cost))),
            );

        let issue = (!s.body.trim().is_empty() || !s.reason.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(14.))
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .bg(SURFACE)
                .child(div().text_size(px(12.)).text_color(TEXT_MUTED).child("Issue"))
                .when(!s.body.trim().is_empty(), |d| {
                    d.child(div().text_color(TEXT_SECONDARY).child(s.body.clone()))
                })
                .when(!s.reason.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child(s.reason.clone()),
                    )
                })
        });

        let summary = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(14.))
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(if s.back_to.is_empty() {
                        "What changed".to_string()
                    } else {
                        format!("What {} reported", s.back_to)
                    }),
            )
            .child(
                div()
                    .text_color(TEXT_SECONDARY)
                    .child(SharedString::from(plain(&s.summary))),
            )
            .when(!s.feedback.is_empty(), |d| {
                d.child(
                    div()
                        .pt(px(6.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child("Your earlier requests"),
                )
                .children(s.feedback.iter().enumerate().map(|(i, f)| {
                    div()
                        .text_size(px(12.))
                        .text_color(TEXT_SECONDARY)
                        .child(format!("{}. {f}", i + 1))
                }))
            });

        let renders = s.renders.iter().map(|(screen, before, after)| {
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(14.))
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .bg(SURFACE)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon("file", 14., TEXT_MUTED))
                        .child(mono(screen.clone()).text_color(TEXT)),
                )
                .child({
                    let h = frame_height(Some(after));
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(picture(before.as_ref(), "Before", h))
                        .child(picture(Some(after), "After", h))
                })
        });

        let actions = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .pt(px(SPACE_4))
            .border_t_1()
            .border_color(LINE)
            .when(self.changes_open, |d| {
                d.child(
                    ui::motion::reveal(
                        "inbox-changes",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(SPACE_2))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(40.))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(RADIUS_LG))
                                    .border_1()
                                    .border_color(ACCENT_STRONG)
                                    .bg(SURFACE_RAISED)
                                    .child(div().flex_1().h(px(20.)).child(self.changes_input.clone())),
                            )
                            .child(
                                Button::new("inbox-send-changes", "Send back")
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| this.send_changes(cx))),
                            ),
                    ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_2))
                    .child(
                        Button::new("inbox-open-issue", "Open issue")
                            .icon("check")
                            .on_click(move |_, _, cx| nav::open_issue(open_key.clone(), cx)),
                    )
                    .child(
                        Button::new("inbox-request-changes", "Request changes…")
                            .ghost()
                            .selected(self.changes_open)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.changes_open = !this.changes_open;
                                if this.changes_open {
                                    window.focus(&this.changes_input.focus_handle(cx));
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child("Approving continues the pipeline"),
                    )
                    .child(
                        Button::new("inbox-approve", "Approve")
                            .primary()
                            .icon("check")
                            .on_click(cx.listener(|this, _, _, cx| this.approve(cx))),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_5))
            .child(head)
            .children(issue)
            .child(summary)
            .children(renders)
            .child(actions)
            .into_any_element()
    }
}
