//! Right "Session details" panel: context, cost, todo, modified files, providers, MCP
//! servers, skills in context.

use gpui::{
    AppContext as _, Context, ElementId, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, px,
};
use ui::{Status, icon, progress, status_dot, theme::*};

use crate::extensions::{McpPhase, ServerStatus, fmt_k};
use crate::picker::SkillState;
use crate::session::truncate;

use crate::models::{self, ProviderModels};
use crate::todo::{self, TodoStatus};
use crate::widgets::*;
use crate::{ChatScreen, ConfigState, Trigger};

const PANEL_WIDTH: f32 = 288.;

/// Hover tooltip with mono text (MCP server errors).
struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(420.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(RADIUS_MD))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE_STRONG)
            .font_family(FONT_MONO)
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(TEXT_SECONDARY)
            .child(self.0.clone())
    }
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn mono_meta(text: impl Into<gpui::SharedString>) -> gpui::Div {
    div()
        .font_family(FONT_MONO)
        .text_size(px(12.))
        .text_color(TEXT_SECONDARY)
        .child(text.into())
}

fn empty_line(text: &'static str) -> gpui::Div {
    div().text_size(px(12.)).text_color(TEXT_MUTED).child(text)
}

impl ChatScreen {
    fn context_section(&self) -> gpui::Div {
        let model_id = self
            .last_usage
            .as_ref()
            .map(|(_, m)| m.clone())
            .or_else(|| self.model.as_ref().map(|m| m.model.clone()))
            .unwrap_or_default();
        let window = models::context_window(&model_id);
        let used = self
            .last_usage
            .as_ref()
            .map_or(0, |(u, _)| models::context_used(u));
        let frac = (used as f32 / window as f32).clamp(0., 1.);
        let usage_line = match &self.last_usage {
            None => "No requests yet".to_string(),
            Some((u, _)) => {
                let mut s = format!(
                    "{}% used · in {} · out {}",
                    (frac * 100.).round() as i32,
                    models::fmt_tokens(u.input_tokens),
                    models::fmt_tokens(u.output_tokens)
                );
                let cache = u.cache_read_tokens + u.cache_write_tokens;
                if cache > 0 {
                    s.push_str(&format!(" · cache {}", models::fmt_tokens(cache)));
                }
                s
            }
        };
        let cost = if self.cost_usd == 0. && self.unpriced {
            "—".to_string()
        } else {
            format!(
                "{}{} this session",
                models::fmt_cost(self.cost_usd),
                if self.unpriced { "+" } else { "" }
            )
        };
        let t = &self.total_usage;
        panel_section(8.)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_baseline()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Context"))
                    .child(mono_meta(format!(
                        "{} of {}",
                        models::fmt_tokens(used),
                        models::fmt_tokens(window)
                    ))),
            )
            .child(progress(frac, if frac > 0.9 { WARNING } else { ACCENT }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(usage_line),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .mt(px(4.))
                    .child(div().text_color(TEXT_MUTED).child("Cost"))
                    .child(div().font_family(FONT_MONO).text_size(px(12.)).child(cost)),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_color(TEXT_MUTED).child("Tokens"))
                    .child(mono_meta(format!(
                        "{} in · {} out",
                        models::fmt_tokens(
                            t.input_tokens + t.cache_read_tokens + t.cache_write_tokens
                        ),
                        models::fmt_tokens(t.output_tokens)
                    ))),
            )
    }

    fn todo_section(&self) -> gpui::Div {
        let n = self.todos.len();
        panel_section(6.)
            .child(panel_head(
                "Todo",
                if n == 0 {
                    String::new()
                } else {
                    format!("{} of {n}", todo::done_count(&self.todos))
                },
            ))
            .when(n == 0, |d| {
                d.child(empty_line("The agent's task list shows up here"))
            })
            .children(self.todos.iter().map(|t| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .child(status_dot(todo_status(t.status), true))
                    .child(
                        div()
                            .min_w_0()
                            .text_color(if t.status == TodoStatus::Completed {
                                TEXT_MUTED
                            } else {
                                TEXT
                            })
                            .child(t.content.clone()),
                    )
            }))
    }

    fn files_section(&self) -> gpui::Div {
        let n = self.modified.len();
        panel_section(6.)
            .child(panel_head(
                "Modified files",
                match n {
                    0 => String::new(),
                    1 => "1 file".into(),
                    n => format!("{n} files"),
                },
            ))
            .when(n == 0, |d| d.child(empty_line("No edits yet")))
            .children(self.modified.iter().map(|(path, add, del)| {
                div()
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .child(ellipsis_cell(path.clone()).text_color(TEXT_SECONDARY))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(px(6.))
                            .child(div().text_color(SUCCESS).child(format!("+{add}")))
                            .child(div().text_color(DANGER).child(format!("−{del}"))),
                    )
            }))
    }

    fn providers_section(&self) -> gpui::Div {
        let section =
            panel_section(6.).child(div().font_weight(FontWeight::MEDIUM).child("Providers"));
        match &self.config {
            ConfigState::Loading => {
                section.child(service_row(Status::Draft, "config", "loading…", TEXT_MUTED))
            }
            ConfigState::Failed(e) => {
                section.child(service_row(Status::Error, "config", e.clone(), DANGER))
            }
            ConfigState::Ready(c) if c.providers.is_empty() => {
                section.child(empty_line("None configured"))
            }
            ConfigState::Ready(_) => section.children(self.catalog.iter().map(|(id, m)| match m {
                ProviderModels::Loading => {
                    service_row(Status::Running, id.clone(), "loading…", TEXT_MUTED)
                }
                ProviderModels::Ready(v) => service_row(
                    Status::Live,
                    id.clone(),
                    format!("{} models", v.len()),
                    TEXT_MUTED,
                ),
                ProviderModels::Failed(_) => {
                    service_row(Status::Error, id.clone(), "models unavailable", DANGER)
                }
            })),
        }
        .child(
            div()
                .mt(px(4.))
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child("Workspace"),
        )
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(11.))
                .text_color(TEXT_SECONDARY)
                .child(self.workspace.root.display().to_string()),
        )
    }

    fn mcp_section(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let section =
            panel_section(6.).child(div().font_weight(FontWeight::MEDIUM).child("MCP servers"));
        match &self.mcp.phase {
            McpPhase::Loading => None,
            McpPhase::Failed(e) => Some(
                section.child(
                    service_row(Status::Error, "config", "invalid", DANGER)
                        .id("mcp-config-error")
                        .tooltip({
                            let e = SharedString::from(truncate(e, 1500));
                            move |_, cx| cx.new(|_| Tip(e.clone())).into()
                        }),
                ),
            ),
            McpPhase::Ready if self.mcp.servers.is_empty() => None,
            McpPhase::Ready => Some(section.children(self.mcp.servers.iter().enumerate().map(
                |(ix, s)| {
                    let name = s.config.name.clone();
                    match &s.status {
                        ServerStatus::Starting => {
                            service_row(Status::Running, name, "starting…", TEXT_MUTED)
                                .into_any_element()
                        }
                        ServerStatus::Ready => {
                            let n = s.tools.len();
                            service_row(
                                Status::Live,
                                name,
                                format!("{n} tool{}", if n == 1 { "" } else { "s" }),
                                TEXT_MUTED,
                            )
                            .into_any_element()
                        }
                        ServerStatus::Disabled => {
                            service_row(Status::Draft, name, "disabled", TEXT_MUTED)
                                .into_any_element()
                        }
                        ServerStatus::Failed { error, stderr } => {
                            let mut detail = truncate(error, 600);
                            let tail = tail_lines(stderr, 12);
                            if !tail.trim().is_empty() {
                                detail.push_str("\n\nstderr:\n");
                                detail.push_str(&tail);
                            }
                            let open = s.open;
                            let tip = SharedString::from(detail.clone());
                            let retry_name = name.clone();
                            let row = div()
                                .id(ElementId::Name(format!("mcp-row-{ix}").into()))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .cursor_pointer()
                                .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(s) = this.mcp.servers.get_mut(ix) {
                                        s.open = !s.open;
                                        cx.notify();
                                    }
                                }))
                                .child(status_dot(Status::Error, true))
                                .child(
                                    ellipsis_cell(name)
                                        .font_family(FONT_MONO)
                                        .text_color(TEXT_SECONDARY),
                                )
                                .child(
                                    div()
                                        .id(ElementId::Name(format!("mcp-retry-{ix}").into()))
                                        .flex_none()
                                        .text_color(DANGER)
                                        .hover(|s| s.underline())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.start_server(retry_name.clone(), cx);
                                        }))
                                        .child("failed · retry"),
                                );
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(row)
                                .when(open, |d| {
                                    d.child(
                                        div()
                                            .ml(px(14.))
                                            .px(px(8.))
                                            .py(px(6.))
                                            .rounded(px(RADIUS_MD))
                                            .bg(SURFACE)
                                            .border_1()
                                            .border_color(LINE)
                                            .font_family(FONT_MONO)
                                            .text_size(px(11.))
                                            .line_height(px(16.))
                                            .text_color(TEXT_SECONDARY)
                                            .child(detail),
                                    )
                                })
                                .into_any_element()
                        }
                    }
                },
            ))),
        }
    }

    /// "Skills in context": forced skills and skills loaded this session.
    fn skills_section(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if self.skills.is_empty() {
            return None;
        }
        let loaded = self.loaded_skills();
        let rows: Vec<(String, SkillState)> = self
            .skills
            .iter()
            .map(|s| (s.name.clone(), self.skill_state(&s.name, &loaded)))
            .filter(|(_, st)| *st != SkillState::Auto)
            .collect();
        let tokens: usize = rows.iter().map(|(n, _)| self.skill_tokens(n)).sum();
        Some(
            panel_section(6.)
                .child(panel_head(
                    "Skills in context",
                    format!("{} tokens", fmt_k(tokens)),
                ))
                .when(rows.is_empty(), |d| {
                    d.child(empty_line("None yet · skills load when relevant"))
                })
                .children(rows.into_iter().map(|(name, st)| {
                    let (how, color) = if st == SkillState::Forced {
                        ("forced", ACCENT_TEXT)
                    } else {
                        ("auto", SUCCESS)
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .child(icon("policy", 12., color))
                        .child(
                            ellipsis_cell(name)
                                .font_family(FONT_MONO)
                                .text_color(TEXT_SECONDARY),
                        )
                        .child(div().flex_none().text_color(color).child(how))
                }))
                .child(
                    div()
                        .id("manage-skills")
                        .text_size(px(12.))
                        .text_color(ACCENT_TEXT)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                            // Like the agent button: the picker's outside-click close skips it.
                            this.trigger_hover = hovered.then_some(Trigger::Agent);
                        }))
                        .on_click(cx.listener(|this, _, _, cx| this.manage_skills(cx)))
                        .child("Manage skills"),
                ),
        )
    }

    pub(crate) fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let skills = self.skills_section(cx);
        let mut sections = vec![self.providers_section()];
        sections.extend(self.mcp_section(cx));
        let last = sections.len() - 1;
        let sections = sections
            .into_iter()
            .enumerate()
            .map(|(i, s)| if i == last { s.border_b_0() } else { s });
        div()
            .id("session-details")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(PANEL_WIDTH))
            .h_full()
            .overflow_y_scroll()
            .bg(BG_SIDEBAR)
            .border_l_1()
            .border_color(LINE)
            .child(self.context_section())
            .children(skills)
            .child(self.todo_section())
            .child(self.files_section())
            .children(sections)
    }
}
