//! Bottom run panel: Test run (stats + timeline), Log and Problems tabs.

use gpui::{AnyElement, Context, FontWeight, IntoElement, div, prelude::*, px, relative};
use ui::{Button, Stat, TabItem, Tabs, Tone, icon, theme::*};

use crate::PipelineEditorScreen;
use crate::data::*;
use crate::widgets::link_button;

const AXIS: [&str; 5] = ["0:00", "0:30", "1:00", "1:30", "1:45"];

impl PipelineEditorScreen {
    fn log_count(&self) -> usize {
        self.step.min(LOG.len())
    }

    pub(crate) fn render_run_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let select = cx.listener(|this, i: &usize, _, cx| {
            this.tab = *i;
            cx.notify();
        });
        let tabs = Tabs::new(
            "run-tabs",
            vec![
                TabItem::new("Test run"),
                TabItem::new("Log").count(self.log_count().to_string()),
                TabItem::new("Problems").count("1"),
            ],
            self.tab,
        )
        .on_select(move |i, window, cx| select(&i, window, cx));

        let body = match self.tab {
            0 => self.run_tab(cx),
            1 => self.log_tab(),
            _ => self.problems_tab(cx),
        };
        div()
            .border_t_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(
                div()
                    .px(px(24.))
                    .border_b_1()
                    .border_color(LINE)
                    .child(tabs),
            )
            .child(body)
    }

    fn run_tab(&self, _cx: &mut Context<Self>) -> AnyElement {
        let running = self
            .nodes
            .iter()
            .filter(|n| n.kind() == Kind::Agent && self.status_of(&n.id) == RunStatus::Running)
            .count();
        let spawned = if self.step >= 3 { 3 } else { 0 };
        let waiting = if self.step >= 5 { 6 } else { 0 };
        let stat_cell = |first: bool, stat: Stat| {
            div()
                .px(px(24.))
                .when(first, |d| d.pl_0())
                .when(!first, |d| d.border_l_1().border_color(LINE))
                .child(stat)
        };
        let stats = div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap_y(px(16.))
            .px(px(24.))
            .py(px(14.))
            .border_b_1()
            .border_color(LINE)
            .child(stat_cell(
                true,
                Stat::new("Queued in Jira", self.queued().to_string()).sub("issues"),
            ))
            .child(stat_cell(
                false,
                Stat::new("Agents running", running.to_string()).sub("of 4 max"),
            ))
            .child(stat_cell(
                false,
                Stat::new("Spawned", spawned.to_string())
                    .sub("depth ≤2")
                    .tone(Tone::Spawn),
            ))
            .child(stat_cell(
                false,
                Stat::new("Waiting for you", waiting.to_string())
                    .sub("variants")
                    .tone(if waiting > 0 {
                        Tone::Warning
                    } else {
                        Tone::Neutral
                    }),
            ))
            .child(stat_cell(
                false,
                Stat::new("Spend this run", SPEND[self.step.min(LAST_STEP)]).sub("of $3.00"),
            ))
            .child(div().flex_1())
            .when(waiting > 0, |d| {
                d.child(
                    link_button("open-review", 40.)
                        .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Inbox, cx))
                        .child(div().size(px(8.)).rounded_full().bg(WARNING))
                        .child("Open review · 6 variants"),
                )
            });

        let p = PROGRESS[self.step.min(LAST_STEP)];
        let grid_row = |h: f32| {
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .px(px(24.))
                .h(px(h))
        };
        let rows = TIMELINE.iter().map(|r| {
            let end = r.e.min(p);
            let width = (end - r.s).max(0.);
            let color = match r.tone {
                BarTone::Wait => WARNING,
                BarTone::Spawn => SPAWN_BAR,
                BarTone::Run if p >= r.e => SUCCESS,
                BarTone::Run => INFO,
            };
            let spawn = r.tone == BarTone::Spawn;
            grid_row(26.)
                .id(r.name)
                .hover(|s| s.bg(SURFACE))
                .child(
                    div()
                        .w(px(220.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .when(spawn, |d| d.pl(px(16.)))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if spawn { SPAWN_TEXT } else { TEXT })
                                .whitespace_nowrap()
                                .child(r.name),
                        )
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .truncate()
                                .child(r.key),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .relative()
                        .h(px(8.))
                        .rounded_full()
                        .bg(LINE_SUBTLE)
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(relative(r.s / 100.))
                                .w(relative(width / 100.))
                                .rounded_full()
                                .bg(color),
                        ),
                )
        });
        let timeline = div()
            .pt(px(8.))
            .pb(px(12.))
            .child(
                grid_row(20.)
                    .child(
                        div()
                            .w(px(220.))
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child("Test run #42 · 3 issues"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_between()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .children(AXIS.iter().map(|t| div().child(*t))),
                    ),
            )
            .children(rows);

        div().child(stats).child(timeline).into_any_element()
    }

    fn log_tab(&self) -> AnyElement {
        let lines = &LOG[..self.log_count()];
        div()
            .px(px(24.))
            .pt(px(12.))
            .pb(px(16.))
            .min_h(px(120.))
            .font_family(FONT_MONO)
            .text_size(px(12.))
            .line_height(px(22.))
            .when(lines.is_empty(), |d| {
                d.child(
                    div()
                        .font_family(FONT_SANS)
                        .text_size(px(13.))
                        .text_color(TEXT_MUTED)
                        .child("No runs yet. Press Test run to send 3 issues from Jira through this draft."),
                )
            })
            .children(lines.iter().map(|g| {
                let color = match g.tone {
                    LogTone::Spawn => SPAWN_TEXT,
                    LogTone::Wait => WARNING,
                    LogTone::Normal => ACCENT_TEXT,
                };
                div()
                    .flex()
                    .gap(px(16.))
                    .whitespace_nowrap()
                    .child(div().text_color(TEXT_MUTED).child(g.t))
                    .child(div().w(px(72.)).flex_none().text_color(color).child(g.who))
                    .child(div().text_color(TEXT_SECONDARY).child(g.msg))
            }))
            .into_any_element()
    }

    fn problems_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .px(px(24.))
            .pt(px(12.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(12.))
                    .border_1()
                    .border_color(LINE)
                    .rounded(px(RADIUS_XL))
                    .bg(SURFACE)
                    .child(icon("router", 14., TEXT_MUTED))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(4.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Router ·")
                                    .child(div().font_family(FONT_MONO).text_size(px(12.)).child("else"))
                                    .child("has no target"),
                            )
                            .child(div().text_size(px(12.)).text_color(TEXT_MUTED).child(
                                "4 of the last 100 issues matched no rule and were skipped. Connect the port to an agent or a Review node.",
                            )),
                    )
                    .child(
                        Button::new("show-router", "Show node")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let router = this
                                    .by_id("router")
                                    .or_else(|| this.nodes.iter().find(|n| n.kind() == Kind::Router))
                                    .map(|n| n.id.clone());
                                if router.is_some() {
                                    this.select(router, cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }
}
