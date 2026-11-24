//! The send-back dialog: a note, where the next round starts from, how many variants, and
//! what it would take, estimated as the options change.

use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use hub::api::{Estimate, Review, ReviewDecision, StartFrom, variant_letter};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{SubmitOn, TextArea, TextAreaEvent, text_area_field};

/// The dialog's overlay id.
pub(crate) const DIALOG_ID: &str = "send-back";
/// Variants a round may build.
const MAX_VARIANTS: u32 = 9;
/// What the stepper starts at (the mockup's 2).
const DEFAULT_VARIANTS: u32 = 2;

/// Where the next round starts from, as the dialog offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The variant picked in the review keeps its branch.
    Pick,
    /// Every variant gets the note.
    All,
    /// New branches; the variants are archived.
    Over,
}

impl Mode {
    /// The modes in the order the dialog lists them.
    pub(crate) const ALL: [Mode; 3] = [Mode::Pick, Mode::All, Mode::Over];

    /// The mode's name in probes: `send-back-from-variant`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Mode::Pick => "variant",
            Mode::All => "all",
            Mode::Over => "over",
        }
    }
}

/// What the estimate line shows.
pub(crate) enum EstimateState {
    Loading,
    Ready(Estimate),
    Failed,
}

/// The state behind the dialog; the overlay host builds the dialog from it each frame.
pub(crate) struct SendBack {
    review: Arc<Review>,
    pick: u32,
    pub(crate) mode: Mode,
    pub(crate) variants: u32,
    pub(crate) note: Entity<TextArea>,
    pub(crate) estimate: EstimateState,
    generation: u64,
    estimating: Option<Task<()>>,
    /// The note got focus after the dialog's own first focus.
    focused: bool,
    _subscriptions: Vec<Subscription>,
}

impl SendBack {
    /// The dialog for `review`, with `pick` the variant chosen in the table.
    pub(crate) fn new(
        review: Arc<Review>,
        pick: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let note = cx.new(|cx| TextArea::new(cx).submit_on(SubmitOn::Never).rows(4, 10));
        let subscriptions =
            vec![
                cx.subscribe_in(
                    &note,
                    window,
                    |_, _, event: &TextAreaEvent, window, cx| match event {
                        TextAreaEvent::Changed => cx.notify(),
                        // The note takes Escape for itself; it still closes the dialog.
                        TextAreaEvent::Cancel => overlays(window, cx).close(DIALOG_ID, window, cx),
                        _ => {}
                    },
                ),
            ];
        let mut dialog = Self {
            review,
            pick,
            mode: Mode::Pick,
            variants: DEFAULT_VARIANTS,
            note,
            estimate: EstimateState::Loading,
            generation: 0,
            estimating: None,
            focused: false,
            _subscriptions: subscriptions,
        };
        dialog.refresh_estimate(cx);
        dialog
    }

    /// Where the round starts, for the engine.
    pub(crate) fn start(&self) -> StartFrom {
        match self.mode {
            Mode::Pick => StartFrom::Variant { index: self.pick },
            Mode::All => StartFrom::All,
            Mode::Over => StartFrom::Over,
        }
    }

    /// How many variants the round builds: one per variant when all of them go back.
    pub(crate) fn variant_count(&self) -> u32 {
        match self.mode {
            Mode::All => (self.review.variants.len() as u32).max(1),
            Mode::Pick | Mode::Over => self.variants,
        }
    }

    /// The decision the dialog sends, or `None` while there is no note.
    pub(crate) fn decision(&self, cx: &gpui::App) -> Option<ReviewDecision> {
        let note = self.note.read(cx).text().trim().to_string();
        (!note.is_empty()).then(|| ReviewDecision::SendBack {
            note,
            start: self.start(),
            variants: self.variant_count(),
        })
    }

    /// Chooses where the round starts.
    pub(crate) fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.refresh_estimate(cx);
            cx.notify();
        }
    }

    /// Sets how many variants to build.
    pub(crate) fn set_variants(&mut self, n: u32, cx: &mut Context<Self>) {
        let n = n.clamp(1, MAX_VARIANTS);
        if self.variants != n {
            self.variants = n;
            self.refresh_estimate(cx);
            cx.notify();
        }
    }

    /// Asks for a new estimate; an answer to an older question is dropped.
    fn refresh_estimate(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.estimate = EstimateState::Loading;
        let task = hub::queries(cx).send_back_estimate(
            &self.review.id,
            self.start(),
            self.variant_count(),
            cx,
        );
        self.estimating = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.estimate = match result {
                    Ok(estimate) => EstimateState::Ready(estimate),
                    Err(_) => EstimateState::Failed,
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Sends the review back and closes the dialog; nothing happens without a note.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(decision) = self.decision(cx) else {
            return;
        };
        hub::commands(cx).decide_review(&self.review.id, decision, cx);
        overlays(window, cx).close(DIALOG_ID, window, cx);
    }

    /// "about 6 min · $0.30".
    fn estimate_label(&self) -> SharedString {
        match &self.estimate {
            EstimateState::Loading => "Estimating…".into(),
            EstimateState::Failed => "No estimate".into(),
            EstimateState::Ready(estimate) => {
                let minutes = estimate.secs.div_ceil(60).max(1);
                let time = if minutes < 60 {
                    base::fmt::plural(minutes, "min", "min")
                } else {
                    base::fmt::duration_words(minutes * 60)
                };
                format!("about {time} · {}", base::fmt::cost(estimate.cost_usd)).into()
            }
        }
    }

    /// The dialog for this frame. Built inside the overlay host's layer.
    pub(crate) fn dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        if !self.focused {
            // The dialog focuses its first tab stop once drawn (a deferred call queued while
            // it renders, after this one); the note takes focus one turn later.
            self.focused = true;
            let note = self.note.clone();
            window.defer(cx, move |window, cx| {
                window.defer(cx, move |window, cx| note.read(cx).focus(window));
            });
        }
        let review = self.review.clone();
        let agent = if review.agent.is_empty() {
            "the agent".to_string()
        } else {
            review.agent.clone()
        };
        let letter = variant_letter(self.pick);
        let branch = review
            .variants
            .iter()
            .find(|v| v.index == self.pick)
            .and_then(|v| v.branch.clone());
        let count = review.variants.len();
        let mut sub = Vec::new();
        if let Some(key) = &review.issue {
            sub.push(key.to_string());
        }
        sub.push(review.title.clone());
        sub.push(base::fmt::plural(count as u64, "variant", "variants"));

        let note = div()
            .debug_selector(|| "send-back-note".into())
            .child(text_area_field("send-back-note-field", &self.note))
            .probe("send-back-note");
        let option = |mode: Mode, title: String, desc: String| {
            let name = format!("send-back-from-{}", mode.name());
            let card = OptionCard::new(gpui::ElementId::Name(name.clone().into()), title)
                .description(desc)
                .probe_name(name);
            if mode == Mode::Pick {
                card.right(Badge::new("your pick").tone(BadgeTone::Accent))
            } else {
                card
            }
        };
        let keeps = match branch {
            Some(branch) => format!("Keeps its branch {branch} and builds on it"),
            None => "Keeps its work and builds on it".to_string(),
        };
        let selected = Mode::ALL.iter().position(|m| *m == self.mode).unwrap_or(0);
        let modes = cx.entity().downgrade();
        let options = OptionGroup::new("send-back-from")
            .option(option(Mode::Pick, format!("Variant {letter}"), keeps))
            .option(option(
                Mode::All,
                format!(
                    "All {}",
                    base::fmt::plural(count as u64, "variant", "variants")
                ),
                "Each one gets the note and tries again".to_string(),
            ))
            .option(option(
                Mode::Over,
                "Start over".to_string(),
                "New branches, the variants are archived".to_string(),
            ))
            .selected(selected)
            .on_select(move |ix, _, cx| {
                let mode = Mode::ALL[ix.min(Mode::ALL.len() - 1)];
                modes.update(cx, |this, cx| this.set_mode(mode, cx)).ok();
            });

        let stepper = cx.entity().downgrade();
        let confirm_key = cx.entity().downgrade();
        let how_many = if self.mode == Mode::All {
            div()
                .tabular()
                .child(base::fmt::count(self.variant_count().into()))
                .into_any_element()
        } else {
            div()
                .child(
                    Stepper::new("send-back-stepper", self.variants as i32)
                        .range(1, MAX_VARIANTS as i32)
                        .on_change(move |n, _, cx| {
                            stepper
                                .update(cx, |this, cx| this.set_variants(n.max(1) as u32, cx))
                                .ok();
                        }),
                )
                .probe("send-back-variants")
                .into_any_element()
        };
        let sizing = row()
            .gap(px(SPACE_5))
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child("Variants to build"),
                    )
                    .child(how_many),
            )
            .child(grow())
            .child(
                div()
                    .debug_selector(|| "send-back-estimate".into())
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .tabular()
                    .child(self.estimate_label())
                    .probe_value("send-back-estimate", self.estimate_label()),
            );

        let empty = self.note.read(cx).text().trim().is_empty();
        let confirm = Button::new("send-back-confirm", "Send back")
            .primary()
            .icon(IconName::Replay)
            .kbd("⌘↵")
            .disabled(empty)
            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx)));
        let mut dialog = Dialog::new(DIALOG_ID, format!("Send back to {agent}"))
            .width(580.)
            .top(150.)
            .subtitle(sub.join(" · "))
            .child(
                Field::new("What should change?")
                    .hint(format!(
                        "{agent} gets your note, this thread and the diffs below it."
                    ))
                    .child(note),
            )
            .child(Field::new("Start from").child(options))
            .child(sizing)
            .cancel_button()
            .action(div().child(confirm).probe("send-back-confirm"))
            .on_confirm(move |window, cx| {
                confirm_key
                    .update(cx, |this, cx| this.confirm(window, cx))
                    .ok();
            });
        if review.issue.is_some() {
            dialog = dialog.note(
                row()
                    .gap(px(SPACE_1))
                    .child("The issue goes back to")
                    .child(div().text_color(TEXT_SECONDARY).child("In progress")),
            );
        }
        dialog
    }
}
