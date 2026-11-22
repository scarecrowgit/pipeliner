//! Right-hand details panel for the selected entry. Sections differ per kind:
//! orchestrators show their team and agent-creation limits, pipelines how they are called and
//! what is inside, agents their tools and the teams they belong to. Every kind has a
//! skills section (see [`crate::skills`]).

use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, IntoElement, SharedString, div, prelude::*, px,
};
use ui::nav::{Screen, navigate};
use ui::theme::*;
use ui::{Button, Stepper, checkbox, icon};

use crate::data::{Entry, Kind, Use};
use crate::looks::{heading, looks, name_with_kind, small_tile, tile};
use crate::{AgentsScreen, DETAIL_WIDTH};

/// `section` with the panel's standard padding; `last` drops the bottom rule.
pub(crate) fn section(gap: f32, last: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .py(px(SPACE_4))
        .px(px(SPACE_5))
        .when(!last, |d| d.border_b_1().border_color(LINE))
}

pub(crate) fn muted_12(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(TEXT_MUTED)
        .child(text.into())
}

/// `<dl>` with a 96px key column (`gap: 4px 12px`, 12px text).
fn facts(rows: Vec<(&'static str, SharedString)>, mono_values: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(12.))
        .children(rows.into_iter().map(|(k, v)| {
            div()
                .flex()
                .gap(px(12.))
                .child(div().w(px(96.)).flex_none().text_color(TEXT_MUTED).child(k))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(TEXT_SECONDARY)
                        .when(mono_values, |d| d.font_family(FONT_MONO))
                        .child(v),
                )
        }))
}

impl AgentsScreen {
    pub(crate) fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let e = self.selected();
        let mut sections: Vec<AnyElement> = vec![
            self.detail_head(e).into_any_element(),
            self.detail_actions(e).into_any_element(),
            self.detail_about(e, cx).into_any_element(),
            self.detail_uses(e, cx).into_any_element(),
            self.detail_skills(e, cx).into_any_element(),
        ];
        match e.kind {
            Kind::Orchestrator => {
                sections.push(self.detail_team(e, cx).into_any_element());
                sections.push(self.detail_create(e, cx).into_any_element());
                sections.push(
                    section(6., true)
                        .child(heading("How it routes"))
                        .child(div().text_color(TEXT_SECONDARY).child(
                            "Splits the issue, hands each part to the cheapest teammate with the right \
                             skill, and creates a helper only when nobody fits. Asks you when two plans \
                             disagree.",
                        ))
                        .into_any_element(),
                );
            }
            Kind::Pipeline => sections.push(self.detail_called(e).into_any_element()),
            Kind::Agent => sections.push(self.detail_tools(e).into_any_element()),
        }

        div()
            .id("agents-detail")
            .w(px(DETAIL_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .children(sections)
    }

    fn detail_head(&self, e: &Entry) -> impl IntoElement {
        let l = looks(e);
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(SPACE_4))
            .px(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .child(tile(l.icon, 36., RADIUS_LG, 16., l.tile_bg, l.tile_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(TEXT_TITLE))
                            .line_height(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(l.name_color)
                            .truncate()
                            .child(e.name),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .text_color(l.kind_color)
                                    .child(SharedString::from(self.kind_line(e))),
                            )
                            .child(div().text_color(TEXT_DISABLED).child("·"))
                            .child(
                                div()
                                    .text_color(e.state.color())
                                    .child(SharedString::from(e.status_text.clone())),
                            ),
                    ),
            )
    }

    fn detail_actions(&self, e: &Entry) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .py(px(12.))
            .px(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .child(
                Button::new("agents-chat", "Chat with it")
                    .small()
                    .on_click(|_, _, cx| navigate(Screen::Chat, cx)),
            )
            .child(
                Button::new("agents-assign", "Assign an issue")
                    .small()
                    .on_click(|_, _, cx| navigate(Screen::Issues, cx)),
            )
            .when(e.kind == Kind::Pipeline, |d| {
                d.child(
                    Button::new("agents-open-editor", "Open in editor")
                        .small()
                        .on_click(|_, _, cx| navigate(Screen::PipelineEditor, cx)),
                )
            })
    }

    fn detail_about(&self, e: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = Vec::new();
        if let Some(m) = e.model {
            rows.push(("Model", SharedString::from(m)));
        }
        if let Some(r) = e.runtime {
            rows.push(("Runtime", SharedString::from(r)));
        }
        rows.push(("Activity", SharedString::from(e.meta)));

        section(8., false)
            .child(div().text_color(TEXT_SECONDARY).child(e.desc))
            .when_some(e.created_by, |d, by| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .py(px(10.))
                        .px(px(12.))
                        .rounded(px(RADIUS_LG))
                        .bg(SPAWN_SOFT)
                        .border_1()
                        .border_color(SPAWN_LINE)
                        .child(div().text_size(px(12.)).text_color(SPAWN_TEXT).child(
                            SharedString::from(format!(
                                "Created by {} for {} · ends when the issue closes",
                                self.name_of(by),
                                e.for_issue
                            )),
                        ))
                        .child(
                            div().flex().child(
                                Button::new("agents-promote", "Keep as a permanent agent")
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| this.promote(cx))),
                            ),
                        ),
                )
            })
            .child(facts(rows, true))
    }

    fn detail_uses(&self, e: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = if e.kind == Kind::Pipeline {
            "In chat it appears in the agent picker next to Build and Plan. On the board it can be \
             assigned like a teammate."
        } else {
            "The same agent, with the same tools and limits, wherever it runs."
        };
        section(8., false)
            .child(heading("Use it from"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(Use::ALL.iter().map(|&u| {
                        let on = e.uses(u);
                        let (bg, border, color) = if on {
                            (ACCENT_SOFT, ACCENT_RING, ACCENT_TEXT)
                        } else {
                            (gpui::transparent_black(), LINE_STRONG, TEXT_MUTED)
                        };
                        div()
                            .id(("agents-use", u.index()))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(CONTROL_HEIGHT_SM))
                            .px(px(10.))
                            .rounded(px(RADIUS_MD))
                            .border_1()
                            .border_color(border)
                            .bg(bg)
                            .text_color(color)
                            .text_size(px(12.))
                            .cursor_pointer()
                            .when(!on, |d| d.hover(|s| s.text_color(TEXT).bg(SURFACE_RAISED)))
                            .child(icon(
                                "check",
                                12.,
                                if on { color } else { alpha(color, 0.) },
                            ))
                            .child(u.label())
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_use(u, cx)))
                    })),
            )
            .child(muted_12(hint))
    }

    fn detail_team(&self, e: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let team: Vec<&Entry> = e.team.iter().filter_map(|id| self.entry(id)).collect();
        let count = format!(
            "{} {}",
            team.len(),
            if team.len() == 1 { "member" } else { "members" }
        );
        let addable: Vec<&Entry> = self
            .list
            .iter()
            .filter(|m| {
                m.id != e.id
                    && m.created_by.is_none()
                    && !e.team.contains(&m.id)
                    && m.kind != Kind::Orchestrator
            })
            .collect();

        let rows = team.iter().enumerate().map(|(i, m)| {
            let id = m.id;
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(32.))
                .child(small_tile(looks(m)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(name_with_kind(m.name, m.kind.label())),
                )
                .child(muted_12(m.status_text.clone()))
                .child(
                    Button::new(("agents-team-remove", i), "")
                        .small()
                        .ghost()
                        .icon("x")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_member(id, cx))),
                )
        });

        let add = div()
            .flex()
            .flex_col()
            .child(
                div().flex().child(
                    Button::new("agents-team-add", "Add agent or pipeline")
                        .small()
                        .icon("plus")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_add(cx))),
                ),
            )
            .when(self.add_open, |d| {
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
                                    .child(muted_12("Everyone is already on the team")),
                            )
                        })
                        .children(addable.iter().enumerate().map(|(i, m)| {
                            let id = m.id;
                            div()
                                .id(("agents-team-addable", i))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .w_full()
                                .min_h(px(36.))
                                .py(px(4.))
                                .px(px(8.))
                                .rounded(px(RADIUS_MD))
                                .cursor_pointer()
                                .hover(|s| s.bg(SURFACE_HOVER))
                                .child(small_tile(looks(m)))
                                .child(name_with_kind(m.name, m.kind.label()))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.add_member(id, cx)),
                                )
                        })),
                )
            });

        section(8., false)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(heading("Team it runs"))
                    .child(muted_12(count)),
            )
            .children(rows)
            .child(add)
    }

    fn detail_create(&self, e: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let toggle = div()
            .id("agents-can-create")
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(CONTROL_HEIGHT_SM))
            .cursor_pointer()
            .child(checkbox("agents-can-create-box", e.can_create))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Can create its own agents"),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_create(cx)));

        // `items_start` keeps the stepper at its compact intrinsic width.
        let stepper_cell = |caption: &'static str, stepper: Stepper| {
            div()
                .flex()
                .flex_col()
                .items_start()
                .child(
                    div()
                        .mb(px(6.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child(caption),
                )
                .child(stepper)
        };

        // Agents this one created; "Nothing yet" placeholder when empty.
        let created: Vec<(&'static str, &'static str, String, Hsla)> = self
            .list
            .iter()
            .filter(|m| m.created_by == Some(e.id))
            .map(|m| (m.id, m.name, m.status_text.clone(), m.state.color()))
            .collect();

        section(10., false).child(toggle).when(e.can_create, |d| {
            let list: Vec<AnyElement> = if created.is_empty() {
                vec![created_row(0, "Nothing yet", String::new(), TEXT_MUTED).into_any_element()]
            } else {
                created
                    .into_iter()
                    .enumerate()
                    .map(|(i, (id, name, status, color))| {
                        created_row(i, name, status, color)
                            .cursor_pointer()
                            .hover(|s| s.bg(SURFACE_HOVER))
                            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                            .into_any_element()
                    })
                    .collect()
            };
            d.child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(px(12.))
                    .child(stepper_cell(
                        "Alive at once",
                        Stepper::new("agents-max-alive", e.max_alive)
                            .range(1, 8)
                            .on_change(cx.listener(|this, v: &i32, _, cx| this.set_max_alive(*v, cx))),
                    ))
                    .child(stepper_cell(
                        "Depth",
                        Stepper::new("agents-depth", e.depth)
                            .range(1, 3)
                            .on_change(cx.listener(|this, v: &i32, _, cx| this.set_depth(*v, cx))),
                    )),
            )
            .child(muted_12(
                "Each created agent gets a subset of its tools and ≤$0.50 per issue. Deeper than 2 asks you first.",
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(SPAWN_TEXT)
                    .child(SharedString::from(format!("Created by {}", e.name))),
            )
            .children(list)
        })
    }

    fn detail_called(&self, e: &Entry) -> impl IntoElement {
        section(8., false)
            .child(heading("Called as an agent"))
            .child(facts(
                vec![("Takes", e.input.into()), ("Returns", e.output.into())],
                false,
            ))
            .child(div().mt(px(4.)).child(muted_12("Inside")))
            .children(e.team.iter().filter_map(|id| self.entry(id)).map(|m| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(28.))
                    .child(small_tile(looks(m)))
                    .child(m.name)
            }))
    }

    fn detail_tools(&self, e: &Entry) -> impl IntoElement {
        let in_teams: Vec<&str> = self
            .list
            .iter()
            .filter(|o| o.team.contains(&e.id))
            .map(|o| o.name)
            .collect();
        let in_teams = if in_teams.is_empty() {
            "Not in any orchestrator’s team".to_string()
        } else {
            format!("In the team of {}", in_teams.join(", "))
        };
        section(8., false)
            .child(heading("Tools"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(e.tools.iter().map(|s| {
                        div()
                            .flex()
                            .items_center()
                            .h(px(24.))
                            .px(px(8.))
                            .rounded(px(RADIUS_MD))
                            .border_1()
                            .border_color(LINE_STRONG)
                            .bg(SURFACE_RAISED)
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT_SECONDARY)
                            .child(*s)
                    })),
            )
            .child(muted_12(in_teams))
    }
}

/// One row of the "Created by …" list (36px, bleeds 8px into the section padding).
fn created_row(ix: usize, name: &'static str, status: String, color: Hsla) -> gpui::Stateful<Div> {
    div()
        .id(("agents-created", ix))
        .flex()
        .items_center()
        .gap(px(10.))
        .min_h(px(36.))
        .py(px(4.))
        .px(px(8.))
        .mx(px(-8.))
        .rounded(px(RADIUS_MD))
        .child(
            div()
                .size(px(22.))
                .flex_none()
                .rounded(px(RADIUS_MD))
                .bg(SPAWN_SOFT)
                .border_1()
                .border_dashed()
                .border_color(SPAWN_LINE)
                .flex()
                .items_center()
                .justify_center()
                .child(icon("spawn", 12., SPAWN_TEXT)),
        )
        .child(div().flex_1().text_color(SPAWN_TEXT).child(name))
        .child(
            div()
                .text_size(px(12.))
                .text_color(color)
                .child(SharedString::from(status)),
        )
}
