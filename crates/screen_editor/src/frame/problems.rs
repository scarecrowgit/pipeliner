//! The Problems tab: what is wrong with the draft, errors first, each with its node. A click
//! selects the node and brings it into view. The list belongs to the graph as last checked;
//! while a newer graph is being checked the tab says so.
//!
//! Probes: `problem-<n>` for the n-th row as shown (from 0).

use gpui::{AnyElement, App, Context, ElementId, IntoElement, Window, div, prelude::*, px};
use hub::model::{Problem, Severity};
use ui::dev::Probe;
use ui::motion;
use ui::prelude::*;

use super::EditorView;
use crate::Selection;

/// Height of a problem row.
const PROBLEM_ROW_HEIGHT: f32 = 36.0;

/// `problems` as the tab lists them: errors first, then warnings, each in the checker's order.
pub(crate) fn sorted(problems: &[Problem]) -> Vec<Problem> {
    let mut sorted = problems.to_vec();
    sorted.sort_by_key(|problem| problem.severity != Severity::Error);
    sorted
}

impl EditorView {
    /// The problems as the tab lists them.
    pub(crate) fn listed_problems(&self, cx: &App) -> Vec<Problem> {
        sorted(self.parts.draft.read(cx).problems())
    }

    /// Selects the node of the `ix`-th problem as listed and centres it.
    pub(crate) fn open_problem(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(problem) = self.listed_problems(cx).into_iter().nth(ix) else { return };
        match problem.node {
            Some(node) => self.select_node(node, true, cx),
            None => self.parts.draft.update(cx, |draft, cx| draft.select(Selection::None, cx)),
        }
    }

    pub(super) fn render_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (problems, current) = {
            let draft = self.parts.draft.read(cx);
            (sorted(draft.problems()), draft.problems_are_current())
        };
        let errors = problems.iter().filter(|p| p.severity == Severity::Error).count();
        let warnings = problems.len() - errors;
        let summary = if !current {
            "Checking…".to_string()
        } else if problems.is_empty() {
            "No problems: this draft can be deployed".to_string()
        } else {
            let mut parts = Vec::new();
            if errors > 0 {
                parts.push(base::fmt::plural(errors as u64, "error", "errors"));
            }
            if warnings > 0 {
                parts.push(base::fmt::plural(warnings as u64, "warning", "warnings"));
            }
            let tail = if errors > 0 { " · errors block a deploy" } else { "" };
            format!("{}{tail}", parts.join(" · "))
        };
        let names: Vec<String> = {
            let draft = self.parts.draft.read(cx);
            problems
                .iter()
                .map(|p| match (&p.node, draft.graph()) {
                    (Some(node), Some(graph)) => graph.node(node).map_or_else(|| node.to_string(), |n| n.name.clone()),
                    (Some(node), None) => node.to_string(),
                    (None, _) => "Pipeline".to_string(),
                })
                .collect()
        };
        let selected = match self.parts.draft.read(cx).selection() {
            Selection::Node(id) => Some(id.clone()),
            _ => None,
        };
        let rows: Vec<AnyElement> = problems
            .iter()
            .zip(names)
            .enumerate()
            .map(|(ix, (problem, name))| {
                let on = selected.is_some() && problem.node == selected;
                problem_row(ix, problem, name, on, !current, window, cx)
            })
            .collect();
        col()
            .id("editor-problems")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(SPACE_2))
            .child(
                div()
                    .flex_none()
                    .px(px(SPACE_5))
                    .pb(px(SPACE_1_5))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(summary)
                    .probe_value("problems-summary", if current { "current" } else { "checking" }),
            )
            .children(rows)
            .into_any_element()
    }
}

/// One problem: the severity mark, the node's name and the message.
fn problem_row(
    ix: usize,
    problem: &Problem,
    name: String,
    selected: bool,
    stale: bool,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> AnyElement {
    let id = ElementId::Name(format!("problem-{ix}").into());
    let selector = format!("problem-{ix}");
    let (mark, word) = match problem.severity {
        Severity::Error => (DANGER, "Error"),
        Severity::Warning => (WARNING, "Warning"),
    };
    let (rest, hover) = motion::row_looks(selected, SURFACE_RAISED);
    motion::hover_fade_state(id.clone(), rest, hover, window, cx)
        .apply(div().id(id).debug_selector(move || selector))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(SPACE_2_5))
        .h(px(PROBLEM_ROW_HEIGHT))
        .px(px(SPACE_5))
        .cursor_pointer()
        .when(stale, |d| d.opacity(0.6))
        .on_click(cx.listener(move |this, _, _, cx| this.open_problem(ix, cx)))
        .child(icon(IconName::Alert, 14., mark))
        .child(div().flex_none().w(px(56.)).type_style(LABEL).text_color(mark).child(word))
        .child(div().flex_none().max_w(px(200.)).type_style(BODY_STRONG).child(truncate(name)))
        .child(div().flex_1().min_w_0().text_color(TEXT_SECONDARY).child(truncate(problem.message.clone())))
        .probe(format!("problem-{ix}"))
        .into_any_element()
}
