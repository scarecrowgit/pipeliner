//! "Skills" section of the details panel: the entry's own skills (forced or auto, toggle and
//! remove), forced skills inherited from the agent that created it, and an "Add skill" listbox
//! over the shared catalog.

use gpui::{Context, Div, FontWeight, Hsla, IntoElement, SharedString, div, prelude::*, px};
use ui::theme::*;
use ui::{Button, icon};

use crate::AgentsScreen;
use crate::data::{Entry, Kind, SKILL_LIB, SkillInfo, SkillMode, skill_info};
use crate::detail::{muted_12, section};
use crate::looks::{heading, tile};

/// One row of the list, ready to render (`skRows` in the mockup).
struct SkillRow {
    info: &'static SkillInfo,
    /// Own skill and its mode, or `None` for a forced skill inherited from the creator.
    own: Option<SkillMode>,
    state: SharedString,
}

impl AgentsScreen {
    /// Own skills first (insertion order), then the creator's forced skills this entry lacks.
    fn skill_rows(&self, e: &Entry) -> Vec<SkillRow> {
        let own = e.skills.iter().filter_map(|&(name, mode)| {
            let info = skill_info(name)?;
            let state = match mode {
                SkillMode::Forced => format!("forced · always in context · {}k tokens", info.tok),
                SkillMode::Auto => format!("auto · loads when relevant · {}k", info.tok),
            };
            Some(SkillRow {
                info,
                own: Some(mode),
                state: state.into(),
            })
        });
        let inherited = self.inherited_skills(e).into_iter().filter_map(|name| {
            let info = skill_info(name)?;
            let by = self.name_of(e.created_by?);
            Some(SkillRow {
                info,
                own: None,
                state: format!("forced · from {by} · {}k", info.tok).into(),
            })
        });
        own.chain(inherited).collect()
    }

    /// Forced skills of the creator that the entry does not list itself.
    fn inherited_skills(&self, e: &Entry) -> Vec<&'static str> {
        let Some(parent) = e.created_by.and_then(|by| self.entry(by)) else {
            return Vec::new();
        };
        parent
            .skills
            .iter()
            .filter(|&&(k, m)| m == SkillMode::Forced && !e.skills.iter().any(|(o, _)| *o == k))
            .map(|&(k, _)| k)
            .collect()
    }

    fn skill_hint(&self, e: &Entry) -> SharedString {
        match (e.kind, e.created_by) {
            (Kind::Orchestrator, _) => "Forced skills are in its context from the start and are \
                                        passed to every agent it creates."
                .into(),
            (Kind::Pipeline, _) => {
                "Forced skills load into every agent inside this pipeline, on top of their own."
                    .into()
            }
            (Kind::Agent, Some(by)) => format!(
                "Forced skills from {} come with it and can’t be removed here.",
                self.name_of(by)
            )
            .into(),
            (Kind::Agent, None) => "Forced skills are in context before the first message, \
                                    wherever it runs: chat, issues or a pipeline. Auto skills load \
                                    only when the task needs them."
                .into(),
        }
    }

    pub(crate) fn detail_skills(&self, e: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.skill_rows(e);
        let forced = rows
            .iter()
            .filter(|r| r.own != Some(SkillMode::Auto))
            .count();
        let count = format!("{forced} forced · {} auto", rows.len() - forced);
        let addable: Vec<&'static SkillInfo> = SKILL_LIB
            .iter()
            .filter(|s| !rows.iter().any(|r| r.info.name == s.name))
            .collect();

        let header = div()
            .flex()
            .justify_between()
            .items_baseline()
            .gap(px(8.))
            .child(heading("Skills"))
            .child(muted_12(count));

        let add = div()
            .flex()
            .flex_col()
            .child(
                div().flex().child(
                    Button::new("agents-skill-add", "Add skill")
                        .small()
                        .icon("plus")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_skill_add(cx))),
                ),
            )
            .when(self.sk_add_open, |d| {
                d.child(
                    div()
                        .mt(px(6.))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .border_1()
                        .border_color(LINE_STRONG)
                        .rounded(px(RADIUS_XL))
                        .bg(SURFACE_RAISED)
                        .when(addable.is_empty(), |d| {
                            d.child(
                                div()
                                    .px(px(8.))
                                    .py(px(8.))
                                    .child(muted_12("Every skill is already added")),
                            )
                        })
                        .children(addable.iter().enumerate().map(|(i, s)| {
                            let name = s.name;
                            div()
                                .id(("agents-skill-addable", i))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .w_full()
                                .min_h(px(40.))
                                .py(px(4.))
                                .px(px(8.))
                                .rounded(px(RADIUS_MD))
                                .cursor_pointer()
                                .hover(|s| s.bg(SURFACE_HOVER))
                                .child(tile(
                                    "policy",
                                    22.,
                                    RADIUS_MD,
                                    12.,
                                    SURFACE_HOVER,
                                    TEXT_SECONDARY,
                                ))
                                .child(name_desc(s, TEXT))
                                .child(muted_12(format!("{}k", s.tok)))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.add_skill(name, cx)),
                                )
                        })),
                )
            });

        let empty = rows.is_empty();
        section(8., false)
            .child(header)
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(i, r)| skill_row(i, r, cx)),
            )
            .when(empty, |d| {
                d.child(muted_12(
                    "No skills yet. It only knows what its instructions say.",
                ))
            })
            .child(add)
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_MUTED)
                    .child(self.skill_hint(e)),
            )
    }
}

/// Mono name over a one-line, ellipsized description.
fn name_desc(s: &SkillInfo, name_color: Hsla) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .text_color(name_color)
                .child(s.name),
        )
        .child(ellipsis_line(s.desc, 12., TEXT_MUTED))
}

/// One 16px-high line that ellipsizes instead of wrapping or hard-clipping: gpui measures
/// nowrap text once, so the text sits in an absolutely positioned box over a sized cell.
fn ellipsis_line(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .relative()
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(16.))
        .child(
            div()
                .absolute()
                .inset_0()
                .overflow_hidden()
                // `line_clamp(1)` instead of `whitespace_nowrap()`: gpui caches the first
                // (unconstrained) measurement of nowrap text, so it would never truncate.
                .line_clamp(1)
                .text_ellipsis()
                .text_size(px(size))
                .line_height(px(16.))
                .text_color(color)
                .child(text.into()),
        )
}

/// 44px skill row; bleeds 8px into the section padding so the forced tint lines up.
fn skill_row(ix: usize, r: SkillRow, cx: &mut Context<AgentsScreen>) -> Div {
    let forced = r.own == Some(SkillMode::Forced);
    let (bg, tile_bg, tile_fg, state_color) = match r.own {
        Some(SkillMode::Forced) => (ACCENT_GLOW, ACCENT_SOFT, ACCENT_TEXT, ACCENT_TEXT),
        Some(SkillMode::Auto) => (
            gpui::transparent_black(),
            SURFACE_HOVER,
            TEXT_SECONDARY,
            TEXT_MUTED,
        ),
        None => (
            gpui::transparent_black(),
            SPAWN_SOFT,
            SPAWN_TEXT,
            SPAWN_TEXT,
        ),
    };
    let name = r.info.name;

    let text = name_desc(r.info, TEXT).child(ellipsis_line(r.state, 11., state_color));

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
        .child(text)
        .map(|d| match r.own {
            Some(_) => {
                let (btn_bg, border, color) = if forced {
                    (ACCENT_SOFT, ACCENT_RING, ACCENT_TEXT)
                } else {
                    (gpui::transparent_black(), LINE_STRONG, TEXT_SECONDARY)
                };
                d.child(
                    div()
                        .id(("agents-skill-force", ix))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(CONTROL_HEIGHT_SM))
                        .px(px(10.))
                        .rounded(px(RADIUS_MD))
                        .border_1()
                        .border_color(border)
                        .bg(btn_bg)
                        .text_color(color)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .when(!forced, |d| {
                            d.hover(|s| s.text_color(TEXT).bg(SURFACE_RAISED))
                        })
                        .child(icon(
                            "check",
                            12.,
                            if forced { color } else { alpha(color, 0.) },
                        ))
                        .child(if forced { "Forced" } else { "Force load" })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_force(name, cx))),
                )
                .child(
                    Button::new(("agents-skill-remove", ix), "")
                        .small()
                        .ghost()
                        .icon("x")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_skill(name, cx))),
                )
            }
            None => d.child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(SPAWN_TEXT)
                    .child("inherited"),
            ),
        })
}
