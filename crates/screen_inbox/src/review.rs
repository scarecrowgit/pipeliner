//! The review detail: header, provenance, the comparison table, the diff, renders, earlier
//! notes, and the action bar (Ship, Skip, Send back) or what was decided.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, ElementId, IntoElement, ObjectFit, SharedString, Window, div, img,
    prelude::*, px,
};
use hub::LoadState;
use hub::api::{
    Render as Shot, Review, ReviewDecision, ReviewState, SpanKind, Variant, variant_letter,
};
use ui::dev::Probe;
use ui::prelude::*;

use crate::texts::{Look, block, cell};
use crate::view::InboxView;

/// Height of a rendered mockup in the renders card.
const SHOT_HEIGHT: f32 = 220.;
/// Width of a line of text in the middle of the empty detail area.
const PLACEHOLDER_WIDTH: f32 = 420.;
/// Width of the hint under "Nothing waits for you".
const EMPTY_HINT_WIDTH: f32 = 360.;

/// "212 of 212 passed", "211 of 212 · 1 failed", "Passed", "Failed", "No tests", "Not run",
/// and whether it reads as a pass.
pub(crate) fn tests_label(variant: &Variant) -> (String, Option<bool>) {
    match (variant.tests, variant.tests_ok) {
        (Some(t), _) if t.total == 0 => ("No tests".to_string(), None),
        (Some(t), _) if t.failed() == 0 => (
            format!(
                "{} of {} passed",
                base::fmt::count(t.passed.into()),
                base::fmt::count(t.total.into())
            ),
            Some(true),
        ),
        (Some(t), _) => (
            format!(
                "{} of {} · {} failed",
                base::fmt::count(t.passed.into()),
                base::fmt::count(t.total.into()),
                base::fmt::count(t.failed().into())
            ),
            Some(false),
        ),
        (None, Some(true)) => ("Passed".to_string(), Some(true)),
        (None, Some(false)) => ("Failed".to_string(), Some(false)),
        (None, None) => ("Not run".to_string(), None),
    }
}

/// The colour of a test result.
pub(crate) fn tests_color(ok: Option<bool>) -> gpui::Hsla {
    match ok {
        Some(true) => SUCCESS,
        Some(false) => DANGER,
        None => TEXT_MUTED,
    }
}

/// A variant's letter in a 24px (or `small` 20px) tile, accent when chosen.
pub(crate) fn letter_tile(index: u32, accent: bool, small: bool) -> impl IntoElement {
    let size = if small { 20. } else { 24. };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(RADIUS_MD))
        .bg(if accent { ACCENT_SOFT } else { SURFACE_HOVER })
        .text_color(if accent { ACCENT_TEXT } else { TEXT_SECONDARY })
        .type_style(if small { CAPTION_STRONG } else { LABEL_STRONG })
        .child(variant_letter(index))
}

/// What the closed review's banner says.
pub(crate) fn decided_label(review: &Review) -> String {
    match review.state {
        ReviewState::Open => String::new(),
        ReviewState::Shipped => match review
            .picked
            .and_then(|p| review.variants.iter().find(|v| v.index == p))
        {
            Some(v) => format!("Shipped variant {} · {}", variant_letter(v.index), v.name),
            None => "Shipped".to_string(),
        },
        ReviewState::Skipped => "Skipped: nothing was shipped".to_string(),
        ReviewState::SentBack => {
            if review.agent.is_empty() {
                "Sent back".to_string()
            } else {
                format!("Sent back to {}", review.agent)
            }
        }
    }
}

impl InboxView {
    /// Everything right of the queue.
    pub(crate) fn render_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = self.current_item(cx);
        let scope = match (&self.review, &item) {
            (Some(open), _) => format!("review/{}", open.id),
            (None, Some(item)) => format!("item/{}", item.id),
            (None, None) => String::new(),
        };
        if self.texts.scope(scope, window, cx) {
            // The text that had the keys is gone with the item it belonged to.
            window.focus(&self.focus_handle);
        }
        if let Some(open) = &self.review {
            let model = open.model.read(cx);
            return match (model.review().cloned(), model.state().clone()) {
                (Some(review), _) => self.render_review(review, window, cx),
                (None, LoadState::Failed(error)) => {
                    self.error_placeholder(format!("Could not load the review: {error}"), cx)
                }
                (None, _) => self.placeholder("Loading the review…"),
            };
        }
        match item {
            Some(item) => self.render_item(item, window, cx),
            None => {
                let waiting = self.live_items(crate::list::Tab::Waiting, cx).len();
                match self.inbox.read(cx).state().clone() {
                    LoadState::Loading => self.placeholder("Loading…"),
                    LoadState::Failed(error) => {
                        self.error_placeholder(format!("Could not load the inbox: {error}"), cx)
                    }
                    LoadState::Ready if waiting == 0 => empty_state(self.content_width()),
                    LoadState::Ready => self.placeholder("Pick an item on the left"),
                }
            }
        }
    }

    /// A centred line in the detail area: loading, a hint.
    fn placeholder(&self, text: &'static str) -> AnyElement {
        col()
            .flex_1()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(PLACEHOLDER_WIDTH.min(self.content_width())))
                    .text_center()
                    .text_color(TEXT_MUTED)
                    .child(text),
            )
            .into_any_element()
    }

    /// A centred error in the detail area, which can be selected and copied.
    fn error_placeholder(&mut self, text: String, cx: &mut Context<Self>) -> AnyElement {
        let error = self
            .texts
            .get("load-error", text, Look::plain(BODY, DANGER), cx);
        col()
            .flex_1()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(PLACEHOLDER_WIDTH.min(self.content_width())))
                    .child(block("detail-error", error)),
            )
            .into_any_element()
    }

    /// The scrolling column every detail lives in: a fixed width, 24px padding.
    pub(crate) fn detail_column(&self, id: &'static str, content: Vec<AnyElement>) -> AnyElement {
        div()
            .id(id)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                col()
                    .flex_none()
                    .w(px(self.content_width()))
                    .m(px(SPACE_6))
                    .children(content.into_iter().map(|el| div().flex_none().child(el))),
            )
            .into_any_element()
    }

    fn render_review(
        &mut self,
        review: Arc<Review>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chosen = self.review.as_ref().and_then(|r| r.variant);
        let variant = chosen
            .and_then(|c| review.variants.iter().find(|v| v.index == c))
            .cloned();
        let mut content: Vec<AnyElement> = vec![self.review_head(&review, cx)];
        if review.state != ReviewState::Open {
            let decided = self.texts.get(
                "decided",
                decided_label(&review),
                Look::plain(BODY, TEXT),
                cx,
            );
            content.push(
                col()
                    .mt(px(SPACE_4))
                    .debug_selector(|| "review-closed".into())
                    .child(
                        Banner::new()
                            .tone(if review.state == ReviewState::SentBack {
                                BannerTone::Warning
                            } else {
                                BannerTone::Neutral
                            })
                            .child(icon(
                                if review.state == ReviewState::Shipped {
                                    IconName::Check
                                } else {
                                    IconName::Inbox
                                },
                                14.,
                                TEXT_SECONDARY,
                            ))
                            .child(cell("review-closed-text", decided))
                            .child(
                                div()
                                    .flex_none()
                                    .type_style(LABEL)
                                    .text_color(TEXT_MUTED)
                                    .child("This review is closed"),
                            ),
                    )
                    .probe("review-closed")
                    .into_any_element(),
            );
        }
        content.push(
            div()
                .mt(px(SPACE_5))
                .child(self.render_table(&review, chosen, window, cx))
                .into_any_element(),
        );
        if let Some(variant) = &variant {
            content.push(
                div()
                    .mt(px(SPACE_4))
                    .child(self.render_diff(variant, cx))
                    .into_any_element(),
            );
        }
        let shots: Vec<Shot> = review
            .renders
            .iter()
            .chain(variant.iter().flat_map(|v| v.renders.iter()))
            .cloned()
            .collect();
        if !shots.is_empty() {
            let paths: Vec<_> = shots
                .iter()
                .enumerate()
                .map(|(ix, shot)| {
                    self.texts.get(
                        format!("shot-{ix}-path"),
                        shot.path.clone(),
                        Look::plain(MONO, TEXT),
                        cx,
                    )
                })
                .collect();
            content.push(
                div()
                    .mt(px(SPACE_4))
                    .child(render_shots(&shots, paths, cx))
                    .into_any_element(),
            );
        }
        if !review.feedback.is_empty() {
            let notes = self.render_notes(&review, cx);
            content.push(div().mt(px(SPACE_4)).child(notes).into_any_element());
        }
        let column = self.detail_column("review-scroll", content);
        col()
            .flex_1()
            .min_h_0()
            .child(column)
            .when_some(
                variant.filter(|_| review.state == ReviewState::Open),
                |d, variant| d.child(self.action_bar(&variant, cx)),
            )
            .into_any_element()
    }

    /// Key, type and priority; the title; the provenance breadcrumb and what was spawned.
    fn review_head(&mut self, review: &Review, cx: &mut Context<Self>) -> AnyElement {
        let issue = review
            .issue
            .as_ref()
            .and_then(|key| hub::issues(cx).read(cx).issue(key).cloned());
        let tags = row()
            .gap(px(SPACE_1_5))
            .when_some(review.issue.clone(), |d, key| {
                d.child(
                    div()
                        .type_style(MONO)
                        .text_color(TEXT_MUTED)
                        .child(key.to_string()),
                )
            })
            .when_some(issue, |d, issue| {
                let high = issue.priority == hub::api::Priority::High;
                d.child(Badge::new(issue.kind.label()).square()).child(
                    Badge::new(issue.priority.label()).square().tone(if high {
                        BadgeTone::Warning
                    } else {
                        BadgeTone::Neutral
                    }),
                )
            });
        let trail = provenance_line(&review.provenance);
        let spawned = self.spawned_summary(review, cx);
        let title = self.texts.get(
            "title",
            review.title.clone(),
            Look::plain(TITLE_LG, TEXT),
            cx,
        );
        col()
            .child(tags)
            .child(div().mt(px(SPACE_2)).child(block("review-title", title)))
            .child(
                row()
                    .mt(px(SPACE_2_5))
                    .gap(px(SPACE_2_5))
                    .type_style(MONO)
                    .text_color(TEXT_SECONDARY)
                    .child(trail)
                    .when_some(spawned, |d, text| {
                        d.child(div().flex_none().child(SpawnChip::new(text)))
                    }),
            )
            .into_any_element()
    }

    /// "Test writer ×2 spawned by Fixer", from the run's spawned instances.
    fn spawned_summary(&self, review: &Review, cx: &Context<Self>) -> Option<SharedString> {
        let run = self.review.as_ref()?.run.as_ref()?.read(cx).run()?.clone();
        let mut names: Vec<(String, usize)> = Vec::new();
        for node in run.nodes.iter().filter(|n| n.kind == SpanKind::Spawned) {
            match names.iter_mut().find(|(name, _)| name == &node.name) {
                Some((_, n)) => *n += 1,
                None => names.push((node.name.clone(), 1)),
            }
        }
        if names.is_empty() {
            return None;
        }
        let list: Vec<String> = names
            .into_iter()
            .map(|(name, n)| if n > 1 { format!("{name} ×{n}") } else { name })
            .collect();
        let by = if review.agent.is_empty() {
            String::new()
        } else {
            format!(" spawned by {}", review.agent)
        };
        Some(format!("{}{by}", list.join(", ")).into())
    }

    /// Earlier "Send back" notes, oldest first; their text can be selected.
    fn render_notes(&mut self, review: &Review, cx: &mut Context<Self>) -> AnyElement {
        let now = self.now();
        let offset = base::time::utc_offset(now);
        let rows: Vec<AnyElement> = review
            .feedback
            .iter()
            .enumerate()
            .map(|(ix, note)| {
                let text = self.texts.get(
                    format!("note-{ix}"),
                    note.text.clone(),
                    Look::plain(BODY, TEXT_SECONDARY),
                    cx,
                );
                col()
                    .gap(px(SPACE_1))
                    .py(px(SPACE_3))
                    .px(px(SPACE_4))
                    .when(ix > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
                    .child(
                        row()
                            .gap(px(SPACE_2))
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child(format!("Round {}", ix + 1))
                            .child(grow())
                            .child(
                                div()
                                    .type_style(MONO_SM)
                                    .child(base::time::ago(now, note.at, offset)),
                            ),
                    )
                    .child(block("review-note", text))
                    .into_any_element()
            })
            .collect();
        div()
            .debug_selector(|| "review-notes".into())
            .child(
                Card::new()
                    .flush()
                    .header(
                        CardHeader::new("Earlier notes")
                            .count(base::fmt::count(review.feedback.len() as u64)),
                    )
                    .child(col().children(rows)),
            )
            .into_any_element()
    }

    /// "Variant A · $0.21 · 1:42 · 212 of 212 tests", then Send back…, Skip, Ship A.
    fn action_bar(&self, variant: &Variant, cx: &mut Context<Self>) -> AnyElement {
        let letter = variant_letter(variant.index);
        let tests = match variant.tests {
            Some(t) if t.total > 0 => Some(format!(
                "{} of {} tests",
                base::fmt::count(t.passed.into()),
                base::fmt::count(t.total.into())
            )),
            _ => match variant.tests_ok {
                Some(true) => Some("tests passed".to_string()),
                Some(false) => Some("tests failed".to_string()),
                None => None,
            },
        };
        let index = variant.index;
        let summary = row()
            .min_w_0()
            .flex_shrink()
            .gap(px(SPACE_1))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(format!("Variant {letter} ·"))
            .child(div().type_style(MONO).child(format!(
                "{} · {}",
                base::fmt::cost(variant.cost_usd),
                base::fmt::duration(variant.secs)
            )))
            .when_some(tests, |d, t| d.child(format!("· {t}")));
        row()
            .flex_none()
            .gap(px(SPACE_2))
            .h(px(ACTION_BAR_HEIGHT))
            .pl(px(SPACE_5))
            .pr(px(SPACE_4))
            .border_t_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(summary)
            .child(grow())
            .child(
                Button::new("review-send-back", "Send back…")
                    .ghost()
                    .icon(IconName::Replay)
                    .kbd("R")
                    .on_click(cx.listener(|this, _, window, cx| this.open_send_back(window, cx)))
                    .probe("review-send-back"),
            )
            .child(
                Button::new("review-skip", "Skip")
                    .secondary()
                    .on_click(cx.listener(|this, _, _, cx| this.decide(ReviewDecision::Skip, cx)))
                    .probe("review-skip"),
            )
            .child(
                Button::new("review-ship", format!("Ship {letter}"))
                    .primary()
                    .icon(IconName::Check)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.decide(ReviewDecision::Ship { variant: index }, cx)
                    }))
                    .probe("review-ship"),
            )
            .into_any_element()
    }
}

/// "triage › fixer › review" on one line, the last step brighter, ending in `…` when it does
/// not fit; as wide as its text otherwise.
fn provenance_line(steps: &[String]) -> AnyElement {
    let mut text = String::new();
    let mut highlights = Vec::new();
    let last = steps.len().saturating_sub(1);
    for (ix, step) in steps.iter().enumerate() {
        if ix > 0 {
            let start = text.len();
            text.push_str(" › ");
            highlights.push((start..text.len(), TEXT_DISABLED.into()));
        }
        let start = text.len();
        text.push_str(step);
        if ix == last {
            highlights.push((start..text.len(), TEXT.into()));
        }
    }
    div()
        .flex()
        .flex_shrink()
        .min_w_0()
        .debug_selector(|| "review-provenance".into())
        .child(truncate_styled(text, highlights, MONO.line).flex_initial())
        .into_any_element()
}

/// Before and after pictures of rendered files, each under its path (`paths`, in order).
fn render_shots(
    shots: &[Shot],
    paths: Vec<gpui::Entity<ui_rich::markdown::Markdown>>,
    cx: &Context<InboxView>,
) -> AnyElement {
    let queries = hub::queries(cx);
    let picture = |label: &'static str, blob: &hub::api::BlobId, shot: &Shot| {
        let ratio = if shot.height > 0 {
            shot.width as f32 / shot.height as f32
        } else {
            1.6
        };
        let width = (SHOT_HEIGHT * ratio).clamp(120., 520.);
        col()
            .flex_none()
            .gap(px(SPACE_1_5))
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(label))
            .child(
                div()
                    .w(px(width))
                    .h(px(SHOT_HEIGHT))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(BG_SIDEBAR)
                    .overflow_hidden()
                    .child(
                        img(queries.blob_path(blob))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
    };
    let rows: Vec<AnyElement> = shots
        .iter()
        .zip(paths)
        .enumerate()
        .map(|(ix, (shot, path))| {
            col()
                .gap(px(SPACE_2))
                .p(px(SPACE_4))
                .when(ix > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
                .child(block("review-shot-path", path))
                .child(
                    div()
                        .id(ElementId::NamedInteger("review-shot".into(), ix as u64))
                        .flex()
                        .gap(px(SPACE_4))
                        .overflow_x_scroll()
                        .when_some(shot.before.as_ref(), |d, before| {
                            d.child(picture("Before", before, shot))
                        })
                        .child(picture("After", &shot.after, shot)),
                )
                .into_any_element()
        })
        .collect();
    div()
        .debug_selector(|| "review-renders".into())
        .child(
            Card::new()
                .flush()
                .header(CardHeader::new("Renders").count(base::fmt::count(shots.len() as u64)))
                .child(col().children(rows)),
        )
        .into_any_element()
}

/// Nothing waits for a person; the hint is at most `width` wide.
fn empty_state(width: f32) -> AnyElement {
    col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .debug_selector(|| "inbox-nothing".into())
        .child(icon(IconName::Inbox, 24., TEXT_MUTED))
        .child(div().type_style(HEADING).child("Nothing waits for you"))
        .child(
            div()
                .w(px(EMPTY_HINT_WIDTH.min(width)))
                .text_center()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Reviews, questions and reports from agents land here."),
        )
        .into_any_element()
}
