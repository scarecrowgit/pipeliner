//! Settings: providers, models, workspace rules (prompt layer 1), budgets, skills.
//! Design: design/screens/settings.html. Starting point: a read-only summary of the config.

use super::prelude::*;

pub struct SettingsScreen {
    config: Config,
    _subs: Vec<Subscription>,
}

impl SettingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                if changes.iter().any(|c| matches!(c, Change::Config)) {
                    this.config = state::engine(cx).config();
                    cx.notify();
                }
            }
        });
        SettingsScreen { config: state::engine(cx).config(), _subs: vec![sub] }
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.config;
        let header = PageHeader::new("Settings").subtitle(Config::path().display().to_string());
        let content = if !c.is_configured() {
            body().child(EmptyState::new(
                AppIcon::Sliders,
                "No model provider yet",
                "Add an OpenAI-compatible provider and a default model to start chats and runs.",
            ))
        } else {
            let budget = |v: f64| if v > 0.0 { ui::fmt_usd(v) } else { "No budget".to_string() };
            body().child(
                card()
                    .max_w(px(720.))
                    .child(SectionHeader::new("Models").icon(AppIcon::Sliders))
                    .child(key_value("Default", div().text_mono().child(c.default_model.clone())))
                    .child(key_value("Fast", div().text_mono().child(c.fast().to_string())))
                    .child(key_value(
                        "Providers",
                        c.providers.iter().map(|p| p.id.clone()).collect::<Vec<_>>().join(", "),
                    ))
                    .child(key_value("Slots", c.parallel_slots.to_string()))
                    .child(key_value("Daily", budget(c.daily_budget_usd)))
                    .child(key_value("Monthly", budget(c.monthly_budget_usd))),
            )
        };
        screen().child(header).child(content)
    }
}
