//! The detail of items that are not reviews: a question, a report, a pull request, a failure.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px,
};
use hub::api::{InboxItem, InboxKind, Outcome, PullRequest, Target};
use ui::dev::Probe;
use ui::prelude::*;

use crate::list::{is_pull_request, kind_label};
use crate::table::counts;
use crate::texts::{Look, block, cell};
use crate::view::{InboxView, PrStatus};

/// Where Enter (and the first button) takes an item: its chat, its run, its issue.
pub(crate) fn main_target(item: &InboxItem) -> Option<Target> {
    match item.kind {
        InboxKind::Failure => item
            .run
            .clone()
            .map(Target::Run)
            .or_else(|| item.session.clone().map(Target::Session)),
        _ => item
            .session
            .clone()
            .map(Target::Session)
            .or_else(|| item.issue.clone().map(Target::Issue))
            .or_else(|| item.run.clone().map(Target::Run)),
    }
}

/// The outcome as a badge.
fn outcome_badge(outcome: Outcome) -> Badge {
    match outcome {
        Outcome::Done => Badge::new("Done").tone(BadgeTone::Success),
        Outcome::NeedsReview => Badge::new("Needs review")
            .tone(BadgeTone::Warning)
            .dot(Status::Waiting),
        Outcome::Blocked => Badge::new("Blocked")
            .tone(BadgeTone::Warning)
            .dot(Status::Waiting),
        Outcome::Failed => Badge::new("Failed")
            .tone(BadgeTone::Danger)
            .dot(Status::Error),
    }
}

impl InboxView {
    /// The detail of a question, report, pull request or failure.
    pub(crate) fn render_item(
        &mut self,
        item: Arc<InboxItem>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut content: Vec<AnyElement> = vec![self.item_head(&item, cx)];
        match item.kind {
            InboxKind::Question { .. } => content.push(self.question_body(&item, cx)),
            InboxKind::Failure => content.push(self.failure_body(&item, cx)),
            InboxKind::Report | InboxKind::Review { .. } => {
                if let Some(md) = self.summary(&item, cx) {
                    content.push(div().mt(px(SPACE_5)).child(md).into_any_element());
                }
            }
        }
        if let Some(budget) = self.budget_card(cx) {
            content.push(budget);
        }
        for (ix, link) in item
            .links
            .iter()
            .enumerate()
            .filter(|(_, link)| is_pull_request(&link.url))
        {
            let card = self.pull_request_card(ix, &link.url, cx);
            content.push(div().mt(px(SPACE_4)).child(card).into_any_element());
        }
        content.push(self.links(&item));
        self.detail_column("item-scroll", content)
    }

    /// Key, kind, outcome; the title; who it is from and when.
    fn item_head(&mut self, item: &InboxItem, cx: &mut Context<Self>) -> AnyElement {
        let now = self.now();
        let pipeline = item
            .run
            .as_ref()
            .and_then(|run| self.run_pipeline_name(run, cx));
        let mut from = vec![item.source.clone()];
        if let Some(name) = pipeline {
            from.push(name.to_string());
        }
        from.push(base::time::ago(
            now,
            item.created,
            base::time::utc_offset(now),
        ));
        from.retain(|part| !part.is_empty());
        let title = self
            .texts
            .get("title", item.title.clone(), Look::plain(TITLE_LG, TEXT), cx);
        col()
            .child(
                row()
                    .gap(px(SPACE_1_5))
                    .when_some(item.issue.clone(), |d, key| {
                        d.child(
                            div()
                                .type_style(MONO)
                                .text_color(TEXT_MUTED)
                                .child(key.to_string()),
                        )
                    })
                    .child(Badge::new(kind_label(item)).square())
                    .when_some(item.outcome, |d, outcome| d.child(outcome_badge(outcome)))
                    .when(item.archived, |d| d.child(Badge::new("Archived"))),
            )
            .child(div().mt(px(SPACE_2)).child(block("item-title", title)))
            .child(
                div()
                    .mt(px(SPACE_1_5))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(truncate_block(from.join(" · "), LABEL.line)),
            )
            .into_any_element()
    }

    /// The summary as Markdown, when there is one.
    fn summary(&mut self, item: &InboxItem, cx: &mut Context<Self>) -> Option<AnyElement> {
        if item.summary.is_empty() {
            return None;
        }
        let md = self.texts.get(
            "summary",
            item.summary.clone(),
            Look::markdown(BODY, TEXT),
            cx,
        );
        Some(block("item-summary", md))
    }

    /// The question, and where to answer it: the agent waits in its chat.
    fn question_body(&mut self, item: &InboxItem, cx: &mut Context<Self>) -> AnyElement {
        let target = main_target(item);
        let label = match &target {
            Some(Target::Session(_)) => "Answer in the chat",
            Some(Target::Issue(_)) => "Answer on the issue",
            _ => "Open the run",
        };
        let link = |primary: bool| {
            target.clone().map(|target| {
                let button = Button::new("item-answer", label).icon(IconName::Chat);
                let button = if primary {
                    button.primary()
                } else {
                    button.secondary()
                };
                button
                    .on_click(move |_, _, cx| hub::open(target.clone(), cx))
                    .probe("item-answer")
                    .into_any_element()
            })
        };
        let foot = self.answer_box(link(false), cx).unwrap_or_else(|| {
            row()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .child("The agent waits until it is answered."),
                )
                .child(grow())
                .children(link(true))
                .into_any_element()
        });
        let card = Card::new()
            .child(
                row()
                    .gap(px(SPACE_2))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(icon(IconName::Agent, 14., TEXT_MUTED))
                    .child(truncate_lh(
                        if item.source.is_empty() {
                            "The agent asks".to_string()
                        } else {
                            format!("{} asks", item.source)
                        },
                        LABEL.line,
                    )),
            )
            .when_some(self.summary(item, cx), |d, md| d.child(md))
            .child(foot);
        div().mt(px(SPACE_5)).child(card).into_any_element()
    }

    /// The error, and a way to the run. The text takes the width between the icon and the
    /// button and wraps there; icon and button stay level with its first line.
    fn failure_body(&mut self, item: &InboxItem, cx: &mut Context<Self>) -> AnyElement {
        let text: SharedString = if item.summary.is_empty() {
            "The run stopped.".into()
        } else {
            item.summary.clone().into()
        };
        let run = item.run.clone();
        // With a button the row is as tall as the button: the first line of text sits in its
        // middle, as the icon does.
        let lead = if run.is_some() {
            (CONTROL_HEIGHT_SM - BODY.line) / 2.
        } else {
            0.
        };
        let error = self.texts.get("error", text, Look::plain(BODY, TEXT), cx);
        col()
            .mt(px(SPACE_5))
            .debug_selector(|| "item-error".into())
            .child(
                Banner::new().tone(BannerTone::Danger).child(
                    row()
                        .flex_1()
                        .items_start()
                        .gap(px(SPACE_3))
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .h(px(BODY.line + 2. * lead))
                                .debug_selector(|| "item-error-icon".into())
                                .child(icon(IconName::Alert, 14., DANGER)),
                        )
                        .child(cell("item-error-text", error).pt(px(lead)))
                        .when_some(run, |d, run| {
                            d.child(
                                div().flex_none().child(
                                    Button::new("item-open-run", "Open the run")
                                        .secondary()
                                        .small()
                                        .icon(IconName::Runs)
                                        .on_click(move |_, _, cx| {
                                            hub::open(Target::Run(run.clone()), cx)
                                        })
                                        .probe("item-open-run"),
                                ),
                            )
                        }),
                ),
            )
            .into_any_element()
    }

    /// The live status of a pull request.
    fn pull_request_card(&mut self, ix: usize, url: &str, cx: &mut Context<Self>) -> AnyElement {
        let body = match self.prs.get(url) {
            None | Some(PrStatus::Loading) => div()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Reading the pull request…")
                .into_any_element(),
            Some(PrStatus::Failed(error)) => {
                let text = format!("Could not read the pull request: {error}");
                let error = self.texts.get(
                    format!("pr-{ix}-error"),
                    text,
                    Look::plain(LABEL, DANGER),
                    cx,
                );
                block("item-pull-request-error", error)
            }
            Some(PrStatus::Ready(pr)) => {
                let pr = pr.clone();
                self.pr_facts(ix, &pr, cx)
            }
        };
        let open_url = url.to_string();
        let header = CardHeader::new("Pull request").meta(
            Link::new(
                ElementId::NamedInteger("item-pr-open".into(), ix as u64),
                "Open",
            )
            .on_click(move |_, _, cx| cx.open_url(&open_url)),
        );
        div()
            .debug_selector(|| "item-pull-request".into())
            .child(Card::new().header(header).child(body))
            .probe(format!("item-pull-request-{ix}"))
            .into_any_element()
    }

    /// Number, title, state, checks, review, size and branches of a pull request; the title,
    /// the branches and the address can be selected.
    fn pr_facts(&mut self, ix: usize, pr: &PullRequest, cx: &mut Context<Self>) -> AnyElement {
        let (state, tone) = match pr.state.as_str() {
            "merged" => ("Merged", BadgeTone::Accent),
            "open" => ("Open", BadgeTone::Success),
            "closed" => ("Closed", BadgeTone::Danger),
            "draft" => ("Draft", BadgeTone::Neutral),
            _ => ("Unknown", BadgeTone::Neutral),
        };
        let checks = match pr.checks.as_str() {
            "passing" => Some(("Checks passing", SUCCESS)),
            "failing" => Some(("Checks failing", DANGER)),
            "pending" => Some(("Checks running", TEXT_MUTED)),
            _ => None,
        };
        let review = match pr.review.as_str() {
            "approved" => Some("Approved"),
            "changes_requested" => Some("Changes requested"),
            "review_required" => Some("Review required"),
            _ => None,
        };
        let title = self.texts.get(
            format!("pr-{ix}-title"),
            pr.title.clone(),
            Look::plain(BODY_STRONG, TEXT),
            cx,
        );
        let branches = self.texts.get(
            format!("pr-{ix}-branches"),
            format!("{} → {}", pr.head, pr.base),
            Look::plain(MONO, TEXT_MUTED),
            cx,
        );
        let address = self.texts.get(
            format!("pr-{ix}-url"),
            pr.url.clone(),
            Look::plain(MONO_SM, TEXT_MUTED),
            cx,
        );
        col()
            .gap(px(SPACE_2))
            .child(
                row()
                    .items_start()
                    .gap(px(SPACE_2))
                    .child(
                        div()
                            .flex_none()
                            .type_style(MONO)
                            .text_color(TEXT_MUTED)
                            .child(format!("#{}", pr.number)),
                    )
                    .child(cell("item-pull-request-title", title))
                    .child(div().flex_none().child(Badge::new(state).tone(tone))),
            )
            .child(
                row()
                    .items_start()
                    .gap(px(SPACE_3))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(counts(pr.added, pr.removed))
                    .child(cell("item-pull-request-branches", branches))
                    .when_some(checks, |d, (text, color)| {
                        d.child(div().flex_none().text_color(color).child(text))
                    })
                    .when_some(review, |d, text| d.child(div().flex_none().child(text))),
            )
            .when(!pr.url.is_empty(), |d| {
                d.child(block("item-pull-request-url", address))
            })
            .into_any_element()
    }

    /// Buttons to the chat, the run, the issue and every other link of the item.
    fn links(&self, item: &InboxItem) -> AnyElement {
        let mut buttons: Vec<AnyElement> = Vec::new();
        if let Some(session) = item.session.clone() {
            buttons.push(
                Button::new("item-link-chat", "Open the chat")
                    .secondary()
                    .small()
                    .icon(IconName::Chat)
                    .on_click(move |_, _, cx| hub::open(Target::Session(session.clone()), cx))
                    .into_any_element(),
            );
        }
        if let Some(run) = item.run.clone().filter(|_| item.kind != InboxKind::Failure) {
            buttons.push(
                Button::new("item-link-run", "Open the run")
                    .secondary()
                    .small()
                    .icon(IconName::Runs)
                    .on_click(move |_, _, cx| hub::open(Target::Run(run.clone()), cx))
                    .into_any_element(),
            );
        }
        if let Some(key) = item.issue.clone() {
            buttons.push(
                Button::new("item-link-issue", format!("Open {key}"))
                    .secondary()
                    .small()
                    .icon(IconName::Issues)
                    .on_click(move |_, _, cx| hub::open(Target::Issue(key.clone()), cx))
                    .probe("item-link-issue")
                    .into_any_element(),
            );
        }
        for (ix, link) in item.links.iter().enumerate() {
            let url = link.url.clone();
            buttons.push(
                Button::new(
                    ElementId::NamedInteger("item-link".into(), ix as u64),
                    link.label.clone(),
                )
                .ghost()
                .small()
                .icon(IconName::External)
                .on_click(move |_, _, cx| cx.open_url(&url))
                .into_any_element(),
            );
        }
        if buttons.is_empty() {
            return div().into_any_element();
        }
        row()
            .mt(px(SPACE_5))
            .flex_wrap()
            .gap(px(SPACE_2))
            .debug_selector(|| "item-links".into())
            .children(buttons)
            .into_any_element()
    }
}
