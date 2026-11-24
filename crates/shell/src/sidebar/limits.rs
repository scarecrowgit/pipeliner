//! "Limits and runtimes": the daily spend limit and how many agents work at once, saved in the
//! settings; the runtimes, read-only.
//!
//! Probes: `limits-no-limit`, `limits-amount`, `limits-slots`, `limits-save`.

use gpui::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, SharedString, Subscription, Window, div, prelude::*, px};
use model::Runtime;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::foot::runtime_row;

/// Dialog id of the limits dialog.
pub(crate) const LIMITS_DIALOG: &str = "limits";
/// Most agents at once the stepper allows.
pub const SLOTS_MAX: i32 = 64;

/// The dialog's state.
pub(crate) struct LimitsForm {
    amount: Entity<TextInput>,
    no_limit: bool,
    slots: u32,
    error: Option<SharedString>,
    _subscription: Subscription,
}

/// A typed limit in dollars: "10", "$10.50", "7,5". `None` when it is not a positive amount.
pub fn parse_amount(text: &str) -> Option<f64> {
    let cleaned: String = text.trim().trim_start_matches('$').replace(',', ".").trim().to_string();
    let value: f64 = cleaned.parse().ok()?;
    (value.is_finite() && value > 0.).then_some(value)
}

impl LimitsForm {
    fn new(limit: Option<f64>, slots: u32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = limit.map(|l| format!("{l:.2}")).unwrap_or_default();
        let amount = cx.new(|cx| {
            let mut input = TextInput::new(cx).with_text(&text).placeholder("10.00");
            input.set_disabled(limit.is_none(), cx);
            input
        });
        let subscription = cx.subscribe_in(&amount, window, |this: &mut Self, _, event, _, cx| {
            if *event == TextInputEvent::Changed {
                this.error = None;
                cx.notify();
            }
        });
        Self { amount, no_limit: limit.is_none(), slots: slots.max(1), error: None, _subscription: subscription }
    }

    /// Save the settings; `false` (with the reason shown) when the amount is not valid.
    fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let limit = if self.no_limit {
            None
        } else {
            match parse_amount(self.amount.read(cx).text()) {
                Some(value) => Some(value),
                None => {
                    self.error = Some("Type an amount in dollars, or choose No limit".into());
                    cx.notify();
                    return false;
                }
            }
        };
        let commands = hub::commands(cx);
        commands.set_daily_limit(limit, cx);
        commands.set_slots(self.slots, cx);
        true
    }
}

/// Open the dialog with the current settings. Does nothing until the settings have loaded.
pub(crate) fn open(window: &mut Window, cx: &mut App) {
    let ops = hub::ops(cx);
    let Some(settings) = ops.read(cx).settings().value().cloned() else { return };
    let form = cx.new(|cx| LimitsForm::new(settings.daily_limit_usd, settings.slots, window, cx));
    ui::overlays(window, cx).open_dialog(LIMITS_DIALOG, move |_, cx| render(&form, &ops, cx), window, cx);
}

fn render(form: &Entity<LimitsForm>, ops: &Entity<hub::OpsModel>, cx: &App) -> AnyElement {
    let this = form.read(cx);
    let runtimes: Vec<Runtime> = ops.read(cx).runtimes().value().map(|r| r.to_vec()).unwrap_or_default();
    let (toggle, step, save) = (form.clone(), form.clone(), form.clone());
    let commit = move |window: &mut Window, cx: &mut App| {
        if save.update(cx, |this, cx| this.save(cx)) {
            ui::overlays(window, cx).close(LIMITS_DIALOG, window, cx);
        }
    };
    let click = commit.clone();
    Dialog::new(LIMITS_DIALOG, "Limits and runtimes")
        .width(480.)
        .subtitle("Applies to new work; running work finishes as it started")
        .child(
            Field::new("Daily spend limit, in dollars")
                .hint("At the limit, running work finishes and new runs wait for you. Resets at 00:00.")
                .child(
                    row()
                        .gap(px(SPACE_3))
                        .child(
                            div().flex_1().child(
                                text_field("limits-amount", &this.amount).probe("limits-amount"),
                            ),
                        )
                        .child(
                            Checkbox::new("limits-no-limit", this.no_limit)
                                .label("No limit")
                                .on_change(move |on, _, cx| {
                                    toggle.update(cx, |this, cx| {
                                        this.no_limit = on;
                                        this.amount.update(cx, |input, cx| input.set_disabled(on, cx));
                                        this.error = None;
                                        cx.notify();
                                    })
                                })
                                .probe("limits-no-limit"),
                        ),
                )
                .children(this.error.clone().map(|e| div().type_style(LABEL).text_color(DANGER).child(e))),
        )
        .child(
            Field::new("Agents at the same time").hint("Across chats and pipeline runs").child(
                row().child(
                    Stepper::new("limits-slots", this.slots as i32)
                        .range(1, SLOTS_MAX)
                        .on_change(move |value, _, cx| {
                            step.update(cx, |this, cx| {
                                this.slots = value.clamp(1, SLOTS_MAX) as u32;
                                cx.notify();
                            })
                        })
                        .probe("limits-slots"),
                ),
            ),
        )
        .child(Field::new("Runtimes").child(col().children(runtimes.iter().map(runtime_row))))
        .cancel_button()
        .action(
            Button::new("limits-save", "Save")
                .primary()
                .kbd("⌘↵")
                .on_click(move |_, window, cx| click(window, cx))
                .probe("limits-save"),
        )
        .on_confirm(move |window, cx| commit(window, cx))
        .into_any_element()
}
