//! The "Talk to" picker above the agent button: Build / Plan, the user's Claude Code
//! agents (`.claude/agents/*.md`) and skills with their context state ("Force load").
//! Also the forced-skill chips next to the agent button and the selection logic.

use std::collections::BTreeSet;

use agent_tools::agents::{self, AgentDef, ToolGrants};
use agent_tools::skills::{SKILL_TOOL, Skill};
use gpui::{
    AnyElement, Context, ElementId, FontWeight, Hsla, IntoElement, Window, deferred, div,
    prelude::*, px,
};
use serde_json::Value;
use ui::{icon, kbd, theme::*};

use crate::data::*;
use crate::extensions::fmt_k;
use crate::models::{ModelRef, ProviderModels};
use crate::session::{Item, ToolStatus};
use crate::widgets::tile;
use crate::{ChatScreen, Trigger};

/// Picker width / max height (the mockup's `listbox`).
const PICKER_WIDTH: f32 = 400.;
const PICKER_MAX_H: f32 = 420.;

/// One selectable row of the agent part of the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentOpt {
    Mode(Mode),
    Custom(String),
}

/// How a skill reaches the context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillState {
    /// Full text in the system prompt for every request.
    Forced,
    /// Loaded with the `skill` tool in this session.
    Loaded,
    /// Listed by description; loads when relevant.
    Auto,
}

/// Skill name of a `skill` tool input (some gateways send the input JSON-encoded).
pub(crate) fn skill_name(input: &Value) -> Option<String> {
    let obj = match input {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        v => v.clone(),
    };
    obj.get("name").and_then(Value::as_str).map(str::to_string)
}

fn eid(name: &'static str, ix: usize) -> ElementId {
    ElementId::NamedInteger(name.into(), ix as u64)
}

fn group_header(text: &'static str) -> gpui::Div {
    div()
        .flex_none()
        .px(px(8.))
        .pt(px(8.))
        .pb(px(4.))
        .text_size(px(12.))
        .text_color(TEXT_MUTED)
        .child(text)
}

/// Whitespace-collapsed one-liner (descriptions may span lines).
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl ChatScreen {
    // ── Agents ──────────────────────────────────────────────────────────

    /// The selected custom agent, when it exists.
    pub(crate) fn active_agent(&self) -> Option<&AgentDef> {
        let name = self.custom_agent.as_deref()?;
        self.agents.iter().find(|a| a.name == name)
    }

    /// Tools granted by the selected agent's `tools:` list (`None`: the mode's toolset).
    pub(crate) fn agent_grants(&self) -> Option<ToolGrants> {
        self.active_agent()
            .and_then(|a| a.tools.as_ref())
            .map(|t| agents::map_tools(t))
    }

    pub(crate) fn agent_name(&self) -> String {
        match &self.custom_agent {
            Some(name) => name.clone(),
            None => self.mode.name().to_string(),
        }
    }

    pub(crate) fn agent_options(&self) -> Vec<AgentOpt> {
        MODES
            .iter()
            .map(|&m| AgentOpt::Mode(m))
            .chain(self.agents.iter().map(|a| AgentOpt::Custom(a.name.clone())))
            .collect()
    }

    fn current_option(&self) -> AgentOpt {
        match &self.custom_agent {
            Some(name) => AgentOpt::Custom(name.clone()),
            None => AgentOpt::Mode(self.mode),
        }
    }

    /// Index of a picker row among the scroll container's children: headers come first
    /// for "Agents" (0) and "Your agents" (3).
    fn option_child_ix(ix: usize) -> usize {
        if ix < MODES.len() { ix + 1 } else { ix + 2 }
    }

    /// Child index of the "Skills" header.
    fn skills_child_ix(&self) -> usize {
        MODES.len() + 2 + self.agents.len().max(1)
    }

    pub(crate) fn toggle_picker(&mut self, cx: &mut Context<Self>) {
        self.pick_open = !self.pick_open;
        self.model_open = false;
        if self.pick_open {
            let current = self.current_option();
            self.pick_sel = self
                .agent_options()
                .iter()
                .position(|o| *o == current)
                .unwrap_or(0);
            self.pick_scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
        cx.notify();
    }

    /// "Manage skills" in the panel: open the picker scrolled to the skills.
    pub(crate) fn manage_skills(&mut self, cx: &mut Context<Self>) {
        self.toggle_picker(cx);
        if self.pick_open {
            self.pick_scroll
                .scroll_to_top_of_item(self.skills_child_ix());
        }
    }

    pub(crate) fn move_pick(&mut self, up: bool, cx: &mut Context<Self>) {
        let n = self.agent_options().len();
        self.pick_sel = if up {
            (self.pick_sel + n - 1) % n
        } else {
            (self.pick_sel + 1) % n
        };
        self.pick_scroll
            .scroll_to_item(Self::option_child_ix(self.pick_sel));
        cx.notify();
    }

    pub(crate) fn pick_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(opt) = self.agent_options().get(self.pick_sel).cloned() {
            self.select_agent(opt, window, cx);
        }
    }

    pub(crate) fn select_agent(
        &mut self,
        opt: AgentOpt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match opt {
            AgentOpt::Mode(mode) => self.set_mode(mode, cx),
            AgentOpt::Custom(name) => {
                let model = self
                    .agents
                    .iter()
                    .find(|a| a.name == name)
                    .and_then(|a| a.model.clone());
                // `model:` applies only when it names a catalog model exactly (Claude Code
                // aliases such as `sonnet` / `inherit` keep the current model).
                if let Some(m) = model.and_then(|want| self.catalog_model(&want)) {
                    self.model = Some(m);
                }
                self.mode = Mode::Build;
                self.custom_agent = Some(name);
                self.pick_open = false;
                self.refresh_placeholder(cx);
                self.agent_changed(cx);
                cx.notify();
            }
        }
        self.focus_input(window, cx);
    }

    fn catalog_model(&self, label: &str) -> Option<ModelRef> {
        self.catalog.iter().find_map(|(p, m)| match m {
            ProviderModels::Ready(v) => v
                .iter()
                .map(|model| ModelRef {
                    provider: p.clone(),
                    model: model.clone(),
                })
                .find(|r| r.label() == label),
            _ => None,
        })
    }

    /// Drop a selected agent that no longer exists (after discovery / restore).
    pub(crate) fn validate_agent(&mut self, cx: &mut Context<Self>) {
        if self.agents_loaded && self.custom_agent.is_some() && self.active_agent().is_none() {
            self.custom_agent = None;
            self.refresh_placeholder(cx);
        }
    }

    /// Persist the agent / forced-skill choice of an already saved session.
    pub(crate) fn agent_changed(&mut self, cx: &mut Context<Self>) {
        if self.persisted && !self.busy() {
            self.save_session(cx);
        }
    }

    // ── Skills ──────────────────────────────────────────────────────────

    pub(crate) fn set_forced(&mut self, name: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.forced.insert(name.to_string());
        } else {
            self.forced.remove(name);
        }
        self.agent_changed(cx);
        cx.notify();
    }

    /// Skills loaded with the `skill` tool in this session (successful calls).
    pub(crate) fn loaded_skills(&self) -> BTreeSet<String> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Turn(t) => Some(t),
                _ => None,
            })
            .flat_map(|t| t.tools())
            .filter(|c| {
                c.name == SKILL_TOOL && matches!(&c.status, ToolStatus::Done(r) if !r.is_error)
            })
            .filter_map(|c| c.input.as_ref().and_then(skill_name))
            .collect()
    }

    pub(crate) fn skill_state(&self, name: &str, loaded: &BTreeSet<String>) -> SkillState {
        if self.forced.contains(name) {
            SkillState::Forced
        } else if loaded.contains(name) {
            SkillState::Loaded
        } else {
            SkillState::Auto
        }
    }

    pub(crate) fn skill_tokens(&self, name: &str) -> usize {
        self.skill_meta.get(name).map_or(0, |m| m.tokens)
    }

    /// Skills offered through the `skill` tool (forced ones are already in the prompt).
    pub(crate) fn auto_skills(&self) -> Vec<Skill> {
        self.skills
            .iter()
            .filter(|s| !self.forced.contains(&s.name))
            .cloned()
            .collect()
    }

    /// Forced skills that exist, in name order.
    pub(crate) fn forced_skills(&self) -> impl Iterator<Item = &Skill> {
        self.skills.iter().filter(|s| self.forced.contains(&s.name))
    }

    /// "# Forced skills" system-prompt section (empty when none).
    pub(crate) fn forced_section(&self) -> String {
        let mut out = String::new();
        for s in self.forced_skills() {
            let Some(text) = self.skill_meta.get(&s.name).and_then(|m| m.text.as_ref()) else {
                continue;
            };
            if out.is_empty() {
                out.push_str(
                    "# Forced skills\n\nThe user loaded these skills for the whole session. \
                     Follow their instructions whenever they apply; they are already loaded, \
                     do not load them with the `skill` tool.\n",
                );
            }
            out.push_str(&format!("\n## Skill: {}\n\n{}\n", s.name, text.trim_end()));
        }
        out
    }

    // ── Rendering ───────────────────────────────────────────────────────

    pub(crate) fn agent_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let st = if self.custom_agent.is_none() && self.mode == Mode::Build {
            &BUILD_STYLE
        } else {
            &AGENT_STYLE
        };
        let accent = std::ptr::eq(st, &BUILD_STYLE);
        let trigger = div()
            .id("agent-trigger")
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .pl(px(4.))
            .pr(px(8.))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(st.border)
            .bg(st.bg)
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(TEXT)
            .cursor_pointer()
            .when(!accent, |d| d.hover(|s| s.bg(SURFACE_HOVER)))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.trigger_hover = hovered.then_some(Trigger::Agent);
            }))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(cx)))
            .child(tile(st.icon, 20., RADIUS_SM, 12., st.tile_bg, st.tile_fg))
            .child(self.agent_name())
            .child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(TEXT_MUTED)
                    .child(st.label),
            )
            .child(icon("chevron-down", 12., TEXT_MUTED));

        div().relative().child(trigger).when_some(self.pops.picker, |d, f| {
            d.child(
                deferred(
                    ui::motion::pop_above(
                        div().absolute().left_0().bottom_full().w(px(PICKER_WIDTH)),
                        8.,
                        f,
                    )
                    .child(
                            div()
                                .id("agent-picker")
                                .occlude()
                                .child(self.agent_picker(cx)),
                        ),
                )
                .with_priority(1),
            )
        })
    }

    fn agent_row(&self, ix: usize, opt: AgentOpt, cx: &mut Context<Self>) -> AnyElement {
        let st = &AGENT_STYLE;
        let (name, desc, key) = match &opt {
            AgentOpt::Mode(m) => (m.name().to_string(), m.desc().to_string(), true),
            AgentOpt::Custom(n) => {
                let desc = self
                    .agents
                    .iter()
                    .find(|a| &a.name == n)
                    .map(|a| one_line(&a.description))
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "Custom agent".into());
                (n.clone(), desc, false)
            }
        };
        ui::motion::fade_row(
            div().id(eid("pick-agent", ix)),
            eid("pick-agent", ix),
            ix == self.pick_sel,
            SURFACE_HOVER,
        )
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .min_h(px(44.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(RADIUS_MD))
            .text_color(TEXT)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_agent(opt.clone(), window, cx);
            }))
            .child(tile(st.icon, 24., RADIUS_MD, 14., st.tile_bg, st.tile_fg))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::MEDIUM).child(name))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .truncate()
                            .child(desc),
                    ),
            )
            .when(key, |d| d.child(kbd("Tab")))
            .into_any_element()
    }

    fn skill_row(
        &self,
        ix: usize,
        skill: &Skill,
        loaded: &BTreeSet<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.skill_state(&skill.name, loaded);
        let on = state == SkillState::Forced;
        let tok = fmt_k(self.skill_tokens(&skill.name));
        let (state_text, state_color): (String, Hsla) = match state {
            SkillState::Forced => (
                format!("forced · always in context · {tok} tokens"),
                ACCENT_TEXT,
            ),
            SkillState::Loaded => (format!("auto · loaded this session · {tok}"), SUCCESS),
            SkillState::Auto => (format!("auto · loads when relevant · {tok}"), TEXT_MUTED),
        };
        let name = skill.name.clone();
        let toggle = div()
            .id(eid("force-skill", ix))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(if on { ACCENT_RING } else { LINE_STRONG })
            .when(on, |d| d.bg(ACCENT_SOFT))
            .text_color(if on { ACCENT_TEXT } else { TEXT_SECONDARY })
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .when(!on, |d| d.hover(|s| s.bg(SURFACE_HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| this.set_forced(&name, !on, cx)))
            .child(
                div()
                    .flex()
                    .when(!on, |d| d.opacity(0.))
                    .child(icon("check", 12., ACCENT_TEXT)),
            )
            .child(if on { "Forced" } else { "Force load" });
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(44.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(RADIUS_MD))
            .when(on, |d| d.bg(ACCENT_GLOW))
            .child(tile(
                "policy",
                24.,
                RADIUS_MD,
                12.,
                if on { ACCENT_SOFT } else { SURFACE_HOVER },
                if on { ACCENT_TEXT } else { TEXT_SECONDARY },
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT)
                            .truncate()
                            .child(skill.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .truncate()
                            .child(one_line(&skill.description)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(state_color)
                            .child(state_text),
                    ),
            )
            .child(toggle)
            .into_any_element()
    }

    fn agent_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        let opts = self.agent_options();
        rows.push(group_header("Agents").into_any_element());
        for (ix, opt) in opts.iter().take(MODES.len()).enumerate() {
            rows.push(self.agent_row(ix, opt.clone(), cx));
        }
        rows.push(group_header("Your agents").into_any_element());
        if self.agents.is_empty() {
            rows.push(
                div()
                    .flex_none()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(if self.agents_loaded {
                        "No agents yet. Add Claude Code agent files to ~/.claude/agents or .claude/agents."
                    } else {
                        "Looking for agents…"
                    })
                    .into_any_element(),
            );
        }
        for (ix, opt) in opts.iter().enumerate().skip(MODES.len()) {
            rows.push(self.agent_row(ix, opt.clone(), cx));
        }

        let forced_n = self.forced_skills().count();
        rows.push(
            div()
                .flex_none()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(8.))
                .px(px(8.))
                .pt(px(12.))
                .pb(px(4.))
                .mt(px(4.))
                .border_t_1()
                .border_color(LINE)
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child("Skills")
                .child(format!("{forced_n} forced · others load when relevant"))
                .into_any_element(),
        );
        if self.skills.is_empty() {
            rows.push(
                div()
                    .flex_none()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("No skills found in ~/.claude/skills, ~/.config/harness/skills or .claude/skills.")
                    .into_any_element(),
            );
        }
        let loaded = self.loaded_skills();
        for (ix, s) in self.skills.iter().enumerate() {
            rows.push(self.skill_row(ix, s, &loaded, cx));
        }
        rows.push(
            div()
                .flex_none()
                .p(px(8.))
                .mt(px(4.))
                .border_t_1()
                .border_color(LINE)
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(
                    "Forced skills go into context before the first message and stay there, \
                     whichever agent you pick. Your agents come from ~/.claude/agents and the \
                     workspace's .claude/agents.",
                )
                .into_any_element(),
        );

        div()
            .id("agent-picker-list")
            .flex()
            .flex_col()
            .max_h(px(PICKER_MAX_H))
            .overflow_y_scroll()
            .track_scroll(&self.pick_scroll)
            .p(px(4.))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE_RAISED)
            .shadow(popover_shadow())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.trigger_hover != Some(Trigger::Agent) {
                    this.pick_open = false;
                    cx.notify();
                }
            }))
            .children(rows)
    }

    /// Forced-skill chips next to the agent button.
    pub(crate) fn forced_chips(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.forced_skills()
            .enumerate()
            .map(|(ix, s)| {
                let name = s.name.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(ACCENT_RING)
                    .bg(ACCENT_SOFT)
                    .text_color(ACCENT_TEXT)
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .child(icon("policy", 12., ACCENT_TEXT))
                    .child(s.name.clone())
                    .child(
                        div()
                            .id(eid("unforce-skill", ix))
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_SM))
                            .cursor_pointer()
                            .hover(|st| st.bg(ACCENT_GLOW))
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.set_forced(&name, false, cx)
                                }),
                            )
                            .child(icon("x", 12., ACCENT_TEXT)),
                    )
                    .into_any_element()
            })
            .collect()
    }
}
