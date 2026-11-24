//! Marks drawn on top of nodes: a problem the validator found, and during a test run the status
//! of each instance of the node (variant letters A, B, C…).

use std::collections::HashMap;

use gpui::{AnyElement, ElementId, Hsla, SharedString, div, prelude::*, px};
use hub::model::{NodeId, NodeStatus, Problem, Run, Severity};
use ui::assets::{IconName, icon};
use ui::theme::*;
use ui::tooltip::Tooltip;
use ui_rich::canvas::{NODE_WIDTH, NodeShape, Viewport};

/// The problems of one node: the worst severity and every message.
#[derive(Clone, Debug, PartialEq)]
pub struct ProblemMark {
    pub severity: Severity,
    pub message: SharedString,
}

/// The problems of the graph by the node they name; problems of the whole pipeline are not
/// drawn on the canvas.
pub fn problem_marks(problems: &[Problem]) -> HashMap<NodeId, ProblemMark> {
    let mut marks: HashMap<NodeId, (Severity, Vec<&str>)> = HashMap::new();
    for problem in problems {
        let Some(node) = &problem.node else { continue };
        let entry = marks.entry(node.clone()).or_insert((problem.severity, Vec::new()));
        if problem.severity == Severity::Error {
            entry.0 = Severity::Error;
        }
        entry.1.push(&problem.message);
    }
    marks
        .into_iter()
        .map(|(node, (severity, messages))| (node, ProblemMark { severity, message: messages.join(" · ").into() }))
        .collect()
}

/// One instance of a node in a run.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceMark {
    /// "A" for variant 1; `None` outside a variant scope.
    pub letter: Option<char>,
    pub status: NodeStatus,
    /// What the tooltip says.
    pub detail: SharedString,
}

/// The variant letter of `variant` (1 is "A").
pub fn letter(variant: u32) -> Option<char> {
    (1..=26).contains(&variant).then(|| char::from(b'A' + (variant - 1) as u8))
}

/// The instances of `run` by node, in variant order; a sent-back node shows its latest attempt.
pub fn instance_marks(run: &Run) -> HashMap<NodeId, Vec<InstanceMark>> {
    let mut latest: HashMap<(NodeId, u32), &hub::model::NodeInstance> = HashMap::new();
    for instance in &run.nodes {
        let key = (instance.node.clone(), instance.variant);
        if latest.get(&key).is_none_or(|known| known.attempt <= instance.attempt) {
            latest.insert(key, instance);
        }
    }
    let mut marks: HashMap<NodeId, Vec<(u32, InstanceMark)>> = HashMap::new();
    for ((node, variant), instance) in latest {
        let mut detail = format!("{} · {}", instance.name, status_word(instance.status));
        if let Some(l) = letter(variant) {
            detail = format!("{} {l} · {}", instance.name, status_word(instance.status));
        }
        if !instance.summary.is_empty() {
            detail = format!("{detail} · {}", instance.summary);
        }
        marks
            .entry(node)
            .or_default()
            .push((variant, InstanceMark { letter: letter(variant), status: instance.status, detail: detail.into() }));
    }
    marks
        .into_iter()
        .map(|(node, mut list)| {
            list.sort_by_key(|(variant, _)| *variant);
            (node, list.into_iter().map(|(_, mark)| mark).collect())
        })
        .collect()
}

/// The word for a status.
pub fn status_word(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "pending",
        NodeStatus::Running => "running",
        NodeStatus::Waiting => "waiting",
        NodeStatus::Done => "done",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Cancelled => "cancelled",
        NodeStatus::Interrupted => "interrupted",
    }
}

/// The dot colour of a status, as the kit's status dots colour them.
pub fn status_color(status: NodeStatus) -> Hsla {
    match status {
        NodeStatus::Running => INFO,
        NodeStatus::Waiting => WARNING,
        NodeStatus::Done => SUCCESS,
        NodeStatus::Failed | NodeStatus::Interrupted => DANGER,
        NodeStatus::Pending => TEXT_MUTED,
        NodeStatus::Skipped | NodeStatus::Cancelled => TEXT_DISABLED,
    }
}

/// Height of a status pill, canvas units.
const PILL_HEIGHT: f32 = 18.0;
/// Space between the pills and the node.
const PILL_GAP: f32 = 6.0;
/// The most pills drawn; the rest are counted.
const MAX_PILLS: usize = 6;
/// Diameter of a status dot.
const DOT: f32 = 6.0;
/// Diameter of the problem badge and its icon.
const BADGE: f32 = 18.0;
const BADGE_ICON: f32 = 11.0;

/// The status pills of a node's instances, in a row over its top-left corner.
pub fn status_strip(vp: Viewport, shape: &NodeShape, node: usize, id: &NodeId, marks: &[InstanceMark]) -> AnyElement {
    let at = vp.to_screen(shape.origin);
    let z = |u: f32| vp.z(u);
    let single = marks.len() == 1 && marks[0].letter.is_none();
    let pill = |i: usize, mark: &InstanceMark| {
        let text: SharedString = match mark.letter {
            Some(l) if !single => l.to_string().into(),
            _ => status_word(mark.status).into(),
        };
        let selector = format!("status-{id}-{i}");
        div()
            .id(ElementId::NamedInteger(format!("status-{node}").into(), i as u64))
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .items_center()
            .gap(z(SPACE_1))
            .h(z(PILL_HEIGHT))
            .px(z(SPACE_1_5))
            .rounded_full()
            .bg(SURFACE_RAISED)
            .border(z(1.0))
            .border_color(LINE_STRONG)
            .font_family(FONT_MONO)
            .text_size(z(MONO_SM.size))
            .line_height(z(MONO_SM.line))
            .text_color(TEXT_SECONDARY)
            .whitespace_nowrap()
            .child(div().flex_none().size(z(DOT)).rounded_full().bg(status_color(mark.status)))
            .child(text)
            .tooltip(Tooltip::text(mark.detail.clone()))
    };
    let hidden = marks.len().saturating_sub(MAX_PILLS);
    div()
        .absolute()
        .left(px(at.x))
        .top(px(at.y) - z(PILL_HEIGHT + PILL_GAP))
        .w(z(NODE_WIDTH))
        .flex()
        .flex_row()
        .gap(z(SPACE_1))
        .children(marks.iter().take(MAX_PILLS).enumerate().map(|(i, m)| pill(i, m)))
        .when(hidden > 0, |d| {
            d.child(
                div()
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(z(MONO_SM.size))
                    .line_height(z(PILL_HEIGHT))
                    .text_color(TEXT_MUTED)
                    .child(format!("+{hidden}")),
            )
        })
        .into_any_element()
}

/// The problem badge on a node's top-right corner, with the messages as its tooltip.
pub fn problem_badge(vp: Viewport, shape: &NodeShape, node: usize, id: &NodeId, mark: &ProblemMark) -> AnyElement {
    let corner = vp.to_screen(gpui::point(shape.origin.x + NODE_WIDTH, shape.origin.y));
    let tone = match mark.severity {
        Severity::Error => DANGER,
        Severity::Warning => WARNING,
    };
    let selector = format!("problem-{id}-{}", if mark.severity == Severity::Error { "error" } else { "warning" });
    div()
        .id(ElementId::NamedInteger("problem".into(), node as u64))
        .debug_selector(move || selector)
        .absolute()
        .left(px(corner.x) - vp.z(BADGE * 0.75))
        .top(px(corner.y) - vp.z(BADGE * 0.25))
        .size(vp.z(BADGE))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(SURFACE_RAISED)
        .border(vp.z(1.0))
        .border_color(tone)
        .child(icon(IconName::Alert, vp.scale(BADGE_ICON), tone))
        .tooltip(Tooltip::text(mark.message.clone()))
        .into_any_element()
}
