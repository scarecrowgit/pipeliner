//! What the run behind an open item waits on: an answer to an agent's question, or a raised
//! budget. The answer box and the budget control live here.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, Subscription, Window, div,
    prelude::*, px,
};
use hub::RunModel;
use hub::api::{
    InboxId, InboxItem, InboxKind, NodeId, Outcome, Run, RunId, RunState, RunStats, RunWait,
};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{
    SubmitOn, TextArea, TextAreaEvent, TextInput, TextInputEvent, text_area_field, text_field,
};

use crate::view::InboxView;

/// What the budget field starts at, in dollars.
const DEFAULT_RAISE: &str = "1.00";

/// What a run waits on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Waiting {
    /// An agent asked something; the answer goes to `node`.
    Question { node: NodeId },
    /// It stopped at its budget.
    Budget,
}

/// What `run` waits on from a person, if anything besides a review, given why the engine says
/// it waits (`wait`): a question goes to the node that asked it.
pub(crate) fn waiting_on(run: &Run, wait: Option<RunWait>) -> Option<Waiting> {
    match wait? {
        RunWait::Review => None,
        RunWait::Budget => Some(Waiting::Budget),
        RunWait::Question => run
            .nodes
            .iter()
            .find(|n| n.question.is_some())
            .map(|node| Waiting::Question {
                node: node.node.clone(),
            }),
    }
}

/// The run of an item that waited on a person (a question, a blocked run) has moved on, so
/// the item no longer waits.
pub(crate) fn moved_on(item: &InboxItem, run: Option<&Run>) -> bool {
    let asks =
        matches!(item.kind, InboxKind::Question { .. }) || item.outcome == Some(Outcome::Blocked);
    asks && run.is_some_and(|run| run.state != RunState::WaitingAnswer)
}

/// The run of the open item, with the answer box and the budget field.
pub(crate) struct ItemRun {
    pub(crate) item: InboxId,
    pub(crate) id: RunId,
    pub(crate) model: Entity<RunModel>,
    pub(crate) stats: Option<RunStats>,
    pub(crate) answer: Entity<TextArea>,
    pub(crate) amount: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl InboxView {
    /// Opens the run of `item` (not of a review: the review shows its own run).
    pub(crate) fn sync_item_run(&mut self, item: &InboxItem, cx: &mut Context<Self>) {
        let run = match (&item.kind, &item.run) {
            (InboxKind::Review { .. }, _) | (_, None) => None,
            (_, Some(run)) => Some(run.clone()),
        };
        if self.item_run.as_ref().map(|r| (&r.item, &r.id)) == run.as_ref().map(|id| (&item.id, id))
        {
            return;
        }
        self.item_run = run.map(|id| {
            let model = hub::run(&id, cx);
            let answer = cx.new(|cx| {
                TextArea::new(cx)
                    .placeholder("Your answer…")
                    .submit_on(SubmitOn::CmdEnter)
                    .rows(3, 10)
            });
            let amount = cx.new(|cx| TextInput::new(cx).with_text(DEFAULT_RAISE));
            let subscriptions = vec![
                cx.observe(&model, |_, _, cx| cx.notify()),
                cx.subscribe(&answer, |this, _, event: &TextAreaEvent, cx| match event {
                    TextAreaEvent::Submit => this.send_answer(cx),
                    TextAreaEvent::Changed => cx.notify(),
                    _ => {}
                }),
                cx.subscribe(&amount, |this, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Submit => this.raise_budget(cx),
                    TextInputEvent::Changed => cx.notify(),
                    _ => {}
                }),
            ];
            let task = hub::queries(cx).run_stats(&id, cx);
            let for_run = id.clone();
            cx.spawn(async move |this, cx| {
                let stats = task.await.ok();
                this.update(cx, |this, cx| {
                    if let Some(open) = this.item_run.as_mut().filter(|r| r.id == for_run) {
                        open.stats = stats;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            ItemRun {
                item: item.id.clone(),
                id,
                model,
                stats: None,
                answer,
                amount,
                _subscriptions: subscriptions,
            }
        });
    }

    /// The open item's run, once loaded.
    fn item_run_value(&self, cx: &gpui::App) -> Option<std::sync::Arc<Run>> {
        self.item_run.as_ref()?.model.read(cx).run().cloned()
    }

    /// What the open item's run waits on from a person, as the engine reports it.
    fn item_waiting(&self, cx: &gpui::App) -> Option<Waiting> {
        let model = self.item_run.as_ref()?.model.read(cx);
        waiting_on(model.run()?, model.waiting())
    }

    /// A text field of the open item has keyboard focus (the queue's keys stay out of it).
    pub(crate) fn typing(&self, window: &Window, cx: &gpui::App) -> bool {
        self.item_run.as_ref().is_some_and(|r| {
            r.answer.read(cx).is_focused(window) || r.amount.read(cx).is_focused(window)
        })
    }

    /// Sends the answer to the asking agent; an empty answer is refused.
    pub(crate) fn send_answer(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.item_run.as_ref() else {
            return;
        };
        let Some(Waiting::Question { node }) = self.item_waiting(cx) else {
            return;
        };
        let text = open.answer.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let (id, answer) = (open.id.clone(), open.answer.clone());
        hub::commands(cx).answer_run(&id, Some(&node), &text, cx);
        answer.update(cx, |area, cx| area.set_text("", cx));
        cx.notify();
    }

    /// The amount typed into the budget field, when it is a positive number of dollars.
    fn raise_amount(&self, cx: &gpui::App) -> Option<f64> {
        let text = self
            .item_run
            .as_ref()?
            .amount
            .read(cx)
            .text()
            .trim()
            .trim_start_matches('$')
            .to_string();
        text.parse::<f64>()
            .ok()
            .filter(|usd| usd.is_finite() && *usd > 0.)
    }

    /// Lets the run spend the typed amount more.
    pub(crate) fn raise_budget(&mut self, cx: &mut Context<Self>) {
        let Some(usd) = self.raise_amount(cx) else {
            return;
        };
        let Some(open) = self.item_run.as_ref() else {
            return;
        };
        if self.item_waiting(cx) != Some(Waiting::Budget) {
            return;
        }
        let id = open.id.clone();
        hub::commands(cx).raise_run_budget(&id, usd, cx);
        cx.notify();
    }

    /// Under a question: the answer box while the agent waits, a note once it moved on.
    /// `None` when the item has no run to answer through.
    pub(crate) fn answer_box(
        &self,
        link: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.item_run.as_ref()?;
        self.item_run_value(cx)?;
        let waits = matches!(self.item_waiting(cx), Some(Waiting::Question { .. }));
        if !waits {
            return Some(
                row()
                    .gap(px(SPACE_2))
                    .debug_selector(|| "item-answered".into())
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child("Answered. The agent is working again."),
                    )
                    .child(grow())
                    .children(link)
                    .into_any_element(),
            );
        }
        let empty = open.answer.read(cx).text().trim().is_empty();
        let field = div()
            .debug_selector(|| "item-answer-field".into())
            .child(text_area_field("item-answer-field", &open.answer))
            .probe("item-answer-field");
        let send = Button::new("item-answer-send", "Send answer")
            .primary()
            .icon(IconName::ArrowUp)
            .kbd("⌘↵")
            .disabled(empty)
            .on_click(cx.listener(|this, _, _, cx| this.send_answer(cx)))
            .probe("item-answer-send");
        Some(
            col()
                .gap(px(SPACE_2_5))
                .child(field)
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
                        .children(link)
                        .child(send),
                )
                .into_any_element(),
        )
    }

    /// For a run stopped at its budget: what was spent and a way to raise it.
    pub(crate) fn budget_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.item_run.as_ref()?;
        let run = self.item_run_value(cx)?;
        if self.item_waiting(cx) != Some(Waiting::Budget) {
            return None;
        }
        let spent = base::fmt::cost(run.cost_usd);
        let spent = match open.stats.as_ref().and_then(|s| s.budget_usd) {
            Some(limit) => format!("Spent {spent} of {}", base::fmt::cost(limit)),
            None => format!("Spent {spent}"),
        };
        let valid = self.raise_amount(cx).is_some();
        let field = div()
            .w(px(120.))
            .debug_selector(|| "item-budget-amount".into())
            .child(text_field("item-budget-amount", &open.amount).small())
            .probe("item-budget-amount");
        let card = Card::new()
            .header(
                CardHeader::new("Out of budget")
                    .dot(Status::Waiting)
                    .meta(div().tabular().child(spent)),
            )
            .child(div().text_color(TEXT_SECONDARY).child(
                "The run stopped when it reached its budget. Raise it to let the run go on.",
            ))
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child("Add $"),
                    )
                    .child(field)
                    .child(grow())
                    .child(
                        Button::new("item-raise-budget", "Raise budget")
                            .primary()
                            .disabled(!valid)
                            .on_click(cx.listener(|this, _, _, cx| this.raise_budget(cx)))
                            .probe("item-raise-budget"),
                    ),
            );
        Some(
            div()
                .mt(px(SPACE_5))
                .debug_selector(|| "item-budget".into())
                .child(card)
                .into_any_element(),
        )
    }
}
