//! The charts and the pipeline canvas with the numbers of the mockups: Home's "Spend per day"
//! (Today and 90 days), "Spend by agent", the test run timeline of the pipeline editor and the
//! canvas at 86 % with its twelve nodes.
//!
//! `cargo run -p ui_rich --example charts_canvas`

use gpui::{Context, Div, IntoElement, Render, SharedString, Window, div, point, prelude::*, px};
use ui::assets::IconName;
use ui::components::TileTone;
use ui::theme::*;
use ui_rich::canvas::{
    DotGrid, Edge, EdgeKind, Edges, Legend, NodeFrame, NodePorts, NodeState, Outputs, Viewport,
};
use ui_rich::charts::{Bar, BarChart, BarTone, Tick, Timeline, TimelineRow, ValueBars, ValueRow};

fn main() {
    ui::run_app("Charts and canvas", 1280., 1040., |_, cx| cx.new(|_| Demo));
}

struct Demo;

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().child(
            div()
                .id("page")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_4))
                        .p(px(SPACE_6))
                        .child(div().flex().flex_row().gap(px(SPACE_4)).items_start().child(today()).child(ninety_days()).child(compact()))
                        .child(timeline())
                        .child(pipeline_canvas()),
                ),
        )
    }
}

// ------------------------------------------------------------------------------- cards

fn card(width: Option<f32>) -> Div {
    let d = div()
        .flex()
        .flex_col()
        .p(px(SPACE_4))
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL));
    match width {
        Some(w) => d.w(px(w)).flex_none(),
        None => d.w_full(),
    }
}

fn heading(title: &'static str, sub: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .mb(px(SPACE_4))
        .child(div().type_style(HEADING).child(title))
        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(sub))
}

fn note(text: &'static str) -> Div {
    div().mt(px(SPACE_3)).type_style(LABEL).text_color(TEXT_MUTED).child(text)
}

fn bar(label: &str, value: f64) -> Bar {
    let name = if label.is_empty() { "week" } else { label };
    Bar::new(label.to_string(), value).tooltip(format!("{name}: {}", base::fmt::cost(value)))
}

fn by_agent(rows: &[(&str, f64)]) -> ValueBars {
    ValueBars::new(rows.iter().map(|(n, v)| ValueRow::new(n.to_string(), *v)))
}

fn today() -> Div {
    let days = [
        ("Mon", 6.20),
        ("Tue", 7.40),
        ("Wed", 8.80),
        ("Thu", 5.90),
        ("Fri", 7.10),
        ("Sat", 4.30),
        ("Today", 3.81),
    ];
    card(Some(340.))
        .child(heading("Spend per day", "last 7 days · limit $10.00 a day"))
        .child(
            BarChart::new("spend-today", days.iter().map(|(l, v)| bar(l, *v)))
                .limit(10.0)
                .max(10.0)
                .axis_labels("$10", "$5", "$0"),
        )
        .child(note("Today so far: $3.81 of $10.00. Highest day this week: Wed, $8.80."))
        .child(
            div()
                .mt(px(SPACE_4))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .mb(px(SPACE_2))
                        .child(div().type_style(HEADING).child("Spend by agent"))
                        .child(div().flex_1())
                        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("today")),
                )
                .child(by_agent(&[
                    ("Payments lead", 1.12),
                    ("PAY triage", 0.94),
                    ("Build", 0.62),
                    ("Fixer", 0.41),
                    ("Codex", 0.18),
                    ("Auditor", 0.15),
                    ("Others · 6", 0.39),
                ])),
        )
}

fn ninety_days() -> Div {
    let weeks = [
        ("W35", 38.2),
        ("", 41.5),
        ("", 44.0),
        ("W38", 52.3),
        ("", 47.1),
        ("", 39.8),
        ("W41", 55.6),
        ("", 61.2),
        ("", 49.4),
        ("W44", 43.7),
        ("", 50.9),
        ("", 58.3),
        ("Now", 30.4),
    ];
    card(Some(340.))
        .child(heading("Spend per week", "last 13 weeks · limit $70.00 a week"))
        .child(
            BarChart::new("spend-90", weeks.iter().map(|(l, v)| bar(l, *v)))
                .limit(70.0)
                .max(70.0)
                .axis_labels("$70", "$35", "$0"),
        )
        .child(note("This week so far: $30.40 of $70.00. Highest week: W42, $61.20. No week went over the limit."))
        .child(
            div()
                .mt(px(SPACE_4))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .mb(px(SPACE_2))
                        .child(div().type_style(HEADING).child("Spend by agent"))
                        .child(div().flex_1())
                        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("last 90 days")),
                )
                .child(by_agent(&[
                    ("Payments lead", 184.20),
                    ("PAY triage", 151.60),
                    ("Build", 102.30),
                    ("Fixer", 71.90),
                    ("Codex", 29.40),
                    ("Auditor", 24.10),
                    ("Others · 6", 48.90),
                ])),
        )
}

/// The over-limit bar and the compact value bars, which the mockup does not show.
fn compact() -> Div {
    card(Some(300.))
        .child(heading("Over the limit", "limit $5.00 a day"))
        .child(
            BarChart::new(
                "spend-over",
                [("Mon", 3.2), ("Tue", 6.8), ("Wed", 4.9), ("Thu", 8.1), ("Fri", 2.4)]
                    .iter()
                    .map(|(l, v)| bar(l, *v)),
            )
            .limit(5.0)
            .now(4),
        )
        .child(div().mt(px(SPACE_4)).child(heading("Compact, in a popover", "with a spawned row")))
        .child(
            ValueBars::new([
                ValueRow::new("Payments lead", 1.12).probe("spend-payments-lead"),
                ValueRow::new("Migration checker", 0.44).spawn().probe("spend-migration-checker"),
                ValueRow::new("Log digger", 0.02).spawn(),
                ValueRow::new("Build", 0.62),
            ])
            .compact(),
        )
}

// ----------------------------------------------------------------------------- timeline

fn clock(ms: u64) -> SharedString {
    format!("{}:{:02}", ms / 60_000, ms / 1000 % 60).into()
}

fn key_dot(color: gpui::Hsla, word: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_1))
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(word)
}

fn timeline() -> Div {
    const TOTAL: u64 = 105_000;
    let row = |label: &'static str, meta: &'static str, start: u64, width: u64, tone| {
        // The mockup gives left and width in percent of the run.
        TimelineRow::from_ms(label, meta, TOTAL * start / 100, TOTAL * (start + width) / 100, TOTAL, tone)
    };
    let rows = [
        row("Jira puller", "3 issues", 0, 6, BarTone::Agent),
        row("Router", "3 routed", 5, 4, BarTone::Agent),
        row("Fixer ×3", "PAY-1423", 9, 76, BarTone::Agent),
        row("Test writer ×2", "by Fixer", 30, 32, BarTone::Spawn),
        row("Reproducer", "PAY-1427", 9, 49, BarTone::Agent),
        row("Log digger", "by Reproducer", 24, 23, BarTone::Spawn),
        row("Analyst ×2", "PAY-1431", 9, 62, BarTone::Agent),
        row("Review", "6 variants", 86, 14, BarTone::Waiting),
    ];
    let ticks = base_ticks(TOTAL, 30_000);
    card(None)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .type_style(LABEL)
                .child(div().type_style(LABEL_STRONG).child("Last test run #42"))
                .child(div().text_color(TEXT_MUTED).child("· 3 issues · 1:45 · $0.84"))
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_3))
                        .text_color(TEXT_MUTED)
                        .child(key_dot(ACCENT, "agent"))
                        .child(key_dot(SPAWN, "spawned"))
                        .child(key_dot(WARNING, "waiting for you")),
                ),
        )
        .child(div().mt(px(SPACE_1)).child(Timeline::new(rows).ticks(ticks)))
}

fn base_ticks(total_ms: u64, step_ms: u64) -> Vec<Tick> {
    ui_rich::charts::time_ticks(total_ms, step_ms)
        .into_iter()
        .map(|(at, ms)| Tick::new(at, clock(ms)))
        .collect()
}

// ------------------------------------------------------------------------------- canvas

/// One node of the mockup: `(id, x, y, icon, tone, title, sub-line, rows, state)`.
type Spec = (
    &'static str,
    f32,
    f32,
    IconName,
    TileTone,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    NodeState,
);

const NODES: &[Spec] = &[
    ("jira", 20., 170., IconName::Source, TileTone::Info, "Jira puller", "PAY · JQL", &[("queued", "37"), ("poll", "every 2m")], NodeState::Default),
    ("repo", 20., 400., IconName::File, TileTone::Neutral, "Repo context", "docs/**, CODEOWNERS", &[("mode", "read"), ("matched", "14 files")], NodeState::Default),
    ("router", 212., 120., IconName::Router, TileTone::Neutral, "Router", "first match wins", &[("rule 1", "Bug ≥High"), ("rule 2", "Bug"), ("rule 3", "Story"), ("else", "skip")], NodeState::Default),
    ("fixer", 404., 40., IconName::Agent, TileTone::Accent, "Fixer", "opus-5.5 · ×3", &[("tools", "4"), ("spawns", "depth ≤2")], NodeState::Selected),
    ("repro", 404., 280., IconName::Agent, TileTone::Neutral, "Reproducer", "sonnet-5.5 · ×1", &[("tools", "3"), ("spawns", "depth ≤1")], NodeState::Default),
    ("analyst", 404., 510., IconName::Agent, TileTone::Neutral, "Analyst", "sonnet-5.5 · ×2", &[("tools", "2"), ("spawns", "off")], NodeState::Default),
    ("testw", 596., 0., IconName::Spawn, TileTone::Spawn, "Test writer", "by Fixer", &[("per run", "≤3"), ("returns to", "Fixer")], NodeState::Spawned),
    ("logd", 596., 380., IconName::Spawn, TileTone::Spawn, "Log digger", "by Reproducer", &[("per run", "≤2"), ("returns to", "Reproducer")], NodeState::Spawned),
    ("spec", 596., 560., IconName::File, TileTone::Neutral, "spec.md", "out/{key}/spec.md", &[("mode", "write"), ("copies", "per variant")], NodeState::Default),
    ("review", 788., 230., IconName::Inbox, TileTone::Neutral, "Review", "ship 1 of N", &[("reviewers", "oncall"), ("on approve", "ship"), ("on send back", "re-run")], NodeState::Default),
    ("pr", 980., 170., IconName::Zap, TileTone::Neutral, "Open PR", "fix/{key} → main", &[("repo", "payments-api")], NodeState::Default),
    ("jiraupd", 980., 380., IconName::Zap, TileTone::Neutral, "Update Jira", "status → In review", &[("comment", "PR link")], NodeState::Default),
];

fn frame(vp: Viewport, spec: &Spec) -> NodeFrame {
    let (id, x, y, icon, tone, title, sub, rows, state) = *spec;
    let outputs = match id {
        "router" => Outputs::PerRow,
        "jiraupd" => Outputs::None,
        _ => Outputs::One,
    };
    rows.iter()
        .fold(NodeFrame::new(id, vp, point(x, y)), |f, (k, v)| f.row(*k, *v))
        .icon(icon)
        .tone(tone)
        .title(title)
        .subtitle(sub)
        .state(state)
        .input(!matches!(id, "jira" | "repo"))
        .outputs(outputs)
}

fn pipeline_canvas() -> Div {
    let vp = Viewport::new(0.86, point(16., 20.));
    let frames: Vec<(&str, NodeFrame)> = NODES.iter().map(|s| (s.0, frame(vp, s))).collect();
    let ports = |id: &str| -> NodePorts {
        frames.iter().find(|(i, _)| *i == id).map(|(_, f)| f.ports()).unwrap_or_default()
    };
    let out = |id: &str, row: usize| ports(id).outputs.get(row).copied().unwrap_or_default();
    let input = |id: &str| ports(id).input.unwrap_or_default();

    let mut edges = vec![
        Edge::new(out("jira", 0), input("router")),
        Edge::new(out("router", 0), input("fixer")).kind(EdgeKind::Selected).label("×3"),
        Edge::new(out("router", 1), input("repro")).label("×1"),
        Edge::new(out("router", 2), input("analyst")).label("×2"),
    ];
    for target in ["fixer", "repro", "analyst"] {
        edges.push(Edge::new(out("repo", 0), input(target)).kind(EdgeKind::Dim));
    }
    edges.extend([
        Edge::new(out("fixer", 0), input("testw")).kind(EdgeKind::Spawn),
        Edge::new(out("repro", 0), input("logd")).kind(EdgeKind::Spawn),
        Edge::new(out("fixer", 0), input("review")).kind(EdgeKind::Selected),
        Edge::new(out("repro", 0), input("review")),
        Edge::new(out("analyst", 0), input("spec")),
        Edge::new(out("spec", 0), input("review")),
        Edge::new(out("review", 0), input("pr")),
        // Sent back, on the selected path: the back edge's loop and dots, in accent.
        Edge::new(out("review", 0), input("fixer")).kind(EdgeKind::SelectedBack),
    ]);
    // Open PR to Update Jira: straight down from the middle of one to the middle of the other.
    let pr = frames.iter().find(|(i, _)| *i == "pr").map(|(_, f)| f.shape());
    let jira = frames.iter().find(|(i, _)| *i == "jiraupd").map(|(_, f)| f.shape());
    if let (Some(pr), Some(jira)) = (pr, jira) {
        edges.push(Edge::vertical(pr.bottom_center(), jira.top_center()));
    }

    div()
        .relative()
        .w_full()
        .h(px(760.))
        .flex_none()
        .overflow_hidden()
        .bg(BG)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
        .child(DotGrid::new(vp))
        .child(Edges::new(vp, edges))
        .children(frames.into_iter().map(|(_, f)| f))
        .child(
            div()
                .absolute()
                .right(px(SPACE_4))
                .top(px(SPACE_3))
                .type_style(MONO_SM)
                .text_color(TEXT_SECONDARY)
                .child(format!("{}%", vp.percent())),
        )
        .child(div().absolute().left(px(SPACE_4)).bottom(px(SPACE_3)).child(Legend::new()))
}
