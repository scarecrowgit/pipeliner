//! "Skills" section of the details panel: the skills an agent has, with their descriptions
//! from the skill folders (`agent_tools::skills::discover`), and whether each one sits in the
//! system prompt or is loaded on demand.

use gpui::{Context, Div, IntoElement, SharedString, div, prelude::*, px};
use ui::Button;
use ui::theme::*;

use crate::AgentsScreen;
use crate::detail::{muted_12, section};
use crate::looks::{ellipsis_line, heading, tile};

/// Rows shown before "Show all".
const COLLAPSED_ROWS: usize = 6;

/// How a skill reaches the agent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillMode {
    /// Full text in the system prompt.
    Prompt,
    /// Listed; the agent loads it when the task needs it.
    OnDemand,
}

impl AgentsScreen {
    /// `skills` in display order; `hint` explains where they come from.
    pub(crate) fn detail_skills(
        &self,
        skills: Vec<(String, SkillMode)>,
        hint: impl Into<SharedString>,
        empty: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let prompt = skills
            .iter()
            .filter(|(_, m)| *m == SkillMode::Prompt)
            .count();
        let on_demand = skills.len() - prompt;
        let count = match (prompt, on_demand) {
            (0, 0) => String::new(),
            (0, n) => format!("{n} on demand"),
            (p, 0) => format!("{p} in the prompt"),
            (p, n) => format!("{p} in the prompt · {n} on demand"),
        };
        let total = skills.len();
        let collapsed = !self.skills_all && total > COLLAPSED_ROWS;
        let shown = if collapsed { COLLAPSED_ROWS } else { total };

        section(8., false)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_baseline()
                    .gap(px(8.))
                    .child(heading("Skills"))
                    .child(muted_12(count)),
            )
            .children(
                skills
                    .into_iter()
                    .take(shown)
                    .map(|(name, mode)| self.skill_row(name, mode)),
            )
            .when(total == 0, |d| d.child(muted_12(empty)))
            .when(total > COLLAPSED_ROWS, |d| {
                let label = if collapsed {
                    format!("Show all {total}")
                } else {
                    "Show fewer".into()
                };
                d.child(
                    div().flex().child(
                        Button::new("agents-skills-more", label)
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.skills_all = !this.skills_all;
                                cx.notify();
                            })),
                    ),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_MUTED)
                    .child(hint.into()),
            )
    }

    /// 44px skill row; prompt skills are tinted. Bleeds 8px into the section padding.
    fn skill_row(&self, name: String, mode: SkillMode) -> Div {
        let found = self.catalog.skill(&name);
        let (desc, desc_color) = match found {
            Some(s) if !s.description.is_empty() => (s.description.clone(), TEXT_MUTED),
            Some(_) => ("No description".to_string(), TEXT_MUTED),
            None => ("Not found in the skill folders".to_string(), WARNING),
        };
        let (bg, tile_bg, tile_fg, state, state_color) = match mode {
            SkillMode::Prompt => (
                ACCENT_GLOW,
                ACCENT_SOFT,
                ACCENT_TEXT,
                "in the prompt",
                ACCENT_TEXT,
            ),
            SkillMode::OnDemand => (
                gpui::transparent_black(),
                SURFACE_HOVER,
                TEXT_SECONDARY,
                "on demand",
                TEXT_MUTED,
            ),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(44.))
            .py(px(6.))
            .px(px(8.))
            .mx(px(-8.))
            .rounded(px(RADIUS_MD))
            .bg(bg)
            .child(tile("policy", 24., RADIUS_MD, 12., tile_bg, tile_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(TEXT)
                                    .child(SharedString::from(name)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(state_color)
                                    .child(state),
                            ),
                    )
                    .child(ellipsis_line(desc, 12., desc_color)),
            )
    }
}
