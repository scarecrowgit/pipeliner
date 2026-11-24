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
            InboxKind::Question { .. } => content.push(self.question_body(&item)),
            InboxKind::Failure => content.push(self.failure_body(&item)),
            InboxKind::Report | InboxKind::Review { .. } => {
                if let Some(md) = self.summary(&item) {
                    content.push(div().mt(px(SPACE_5)).child(md).into_any_element());
                }
            }
        }
        for (ix, link) in item
            .links
            .iter()
            .enumerate()
            .filter(|(_, link)| is_pull_request(&link.url))
        {
            content.push(
                div()
                    .mt(px(SPACE_4))
                    .child(self.pull_request_card(ix, &link.url))
                    .into_any_element(),
            );
        }
        content.push(self.links(&item));
        self.detail_column("item-scroll", content)
    }

    /// Key, kind, outcome; the title; who it is from and when.
    fn item_head(&self, item: &InboxItem, cx: &Context<Self>) -> AnyElement {
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
            .child(
                div()
                    .mt(px(SPACE_2))
                    .type_style(TITLE_LG)
                    .child(item.title.clone()),
            )
            .child(
                div()
                    .mt(px(SPACE_1_5))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(from.join(" · ")),
            )
            .into_any_element()
    }

    /// The summary as Markdown, when there is one.
    fn summary(&self, item: &InboxItem) -> Option<AnyElement> {
        let (id, md) = self.markdown.as_ref()?;
        (id == &item.id).then(|| {
            div()
                .debug_selector(|| "item-summary".into())
                .child(md.clone())
                .into_any_element()
        })
    }

    /// The question, and where to answer it: the agent waits in its chat.
    fn question_body(&self, item: &InboxItem) -> AnyElement {
        let target = main_target(item);
        let label = match &target {
            Some(Target::Session(_)) => "Answer in the chat",
            Some(Target::Issue(_)) => "Answer on the issue",
            _ => "Open the run",
        };
        let card = Card::new()
            .child(
                row()
                    .gap(px(SPACE_2))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(icon(IconName::Agent, 14., TEXT_MUTED))
                    .child(if item.source.is_empty() {
                        "The agent asks".to_string()
                    } else {
                        format!("{} asks", item.source)
                    }),
            )
            .when_some(self.summary(item), |d, md| d.child(md))
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child("The agent waits until it is answered."),
                    )
                    .child(grow())
                    .when_some(target, |d, target| {
                        d.child(
                            Button::new("item-answer", label)
                                .primary()
                                .icon(IconName::Chat)
                                .on_click(move |_, _, cx| hub::open(target.clone(), cx))
                                .probe("item-answer"),
                        )
                    }),
            );
        div().mt(px(SPACE_5)).child(card).into_any_element()
    }

    /// The error, and a way to the run.
    fn failure_body(&self, item: &InboxItem) -> AnyElement {
        let text: SharedString = if item.summary.is_empty() {
            "The run stopped.".into()
        } else {
            item.summary.clone().into()
        };
        let run = item.run.clone();
        div()
            .mt(px(SPACE_5))
            .debug_selector(|| "item-error".into())
            .child(
                Banner::new()
                    .tone(BannerTone::Danger)
                    .child(icon(IconName::Alert, 14., DANGER))
                    .child(div().flex_1().min_w_0().child(text))
                    .when_some(run, |d, run| {
                        d.child(
                            Button::new("item-open-run", "Open the run")
                                .secondary()
                                .small()
                                .icon(IconName::Runs)
                                .on_click(move |_, _, cx| hub::open(Target::Run(run.clone()), cx))
                                .probe("item-open-run"),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The live status of a pull request.
    fn pull_request_card(&self, ix: usize, url: &str) -> AnyElement {
        let body = match self.prs.get(url) {
            None | Some(PrStatus::Loading) => div()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Reading the pull request…")
                .into_any_element(),
            Some(PrStatus::Failed(error)) => div()
                .type_style(LABEL)
                .text_color(DANGER)
                .child(format!("Could not read the pull request: {error}"))
                .into_any_element(),
            Some(PrStatus::Ready(pr)) => pr_facts(pr).into_any_element(),
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
            .gap(px(SPACE_2))
            .children(buttons)
            .into_any_element()
    }
}

/// Number, title, state, checks, review and size of a pull request.
fn pr_facts(pr: &PullRequest) -> impl IntoElement {
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
    col()
        .gap(px(SPACE_2))
        .child(
            row()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .type_style(MONO)
                        .text_color(TEXT_MUTED)
                        .child(format!("#{}", pr.number)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .type_style(BODY_STRONG)
                        .child(pr.title.clone()),
                )
                .child(Badge::new(state).tone(tone)),
        )
        .child(
            row()
                .gap(px(SPACE_3))
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(counts(pr.added, pr.removed))
                .child(
                    div()
                        .type_style(MONO)
                        .child(format!("{} → {}", pr.head, pr.base)),
                )
                .when_some(checks, |d, (text, color)| {
                    d.child(div().text_color(color).child(text))
                })
                .when_some(review, |d, text| d.child(text)),
        )
}
