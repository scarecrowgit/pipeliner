//! Where everything sits on the canvas, computed from the definition: one node per step in
//! run order, left to right, wrapping into rows of `cols` columns, then an End node. An
//! agent step with sub-agents gets a spawn node below it. Edges:
//! - **next**: consecutive steps (a row wrap goes right, down the gap, back left);
//! - **jump**: a decision's "no" → `on_no` (or End), routed along the lane under the row and
//!   down the gap left of the target;
//! - **back**: an approval's "send back" → `back_to`, routed over the nodes (dashed loop);
//! - **spawn**: a delegating agent → its sub-agent node (dashed).
//!
//! Units are layer units; the canvas multiplies them by the zoom factor.

use gpui::Hsla;
use pipeline::{Delegate, PipelineDef, StepDef, StepKind, Tools};
use ui::theme::*;

use crate::snap::{first_line, model_short};

pub const NODE_W: f32 = 200.;
pub const END_W: f32 = 120.;
pub const COL_GAP: f32 = 76.;
/// Between a row's bottom and the next row's loop space.
pub const ROW_GAP: f32 = 44.;
/// Above the nodes of a row, for "send back" loops.
pub const LOOP_SPACE: f32 = 56.;
pub const MARGIN_X: f32 = 64.;
pub const MARGIN_TOP: f32 = 4.;
pub const SPAWN_GAP: f32 = 52.;
pub const TITLE_H: f32 = 40.;
pub const META_H: f32 = 26.;
pub const ROW_H: f32 = 26.;
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 1.4;

/// Total height of a node with `rows` key/value rows (border, title, meta, rows, padding).
pub fn node_h(rows: usize) -> f32 {
    2. + TITLE_H + META_H + ROW_H * rows as f32 + 4.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    /// A decision's "no".
    Jump,
    /// Sub-agents.
    Spawn,
}

/// A key/value row inside a node.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub value: String,
    pub port: Option<PortKind>,
}

fn row(label: &'static str, value: impl Into<String>) -> Row {
    Row { label, value: value.into(), port: None }
}

pub fn tools_short(t: Tools) -> &'static str {
    match t {
        Tools::ReadOnly => "read",
        Tools::ReadAndRun => "read + run",
        Tools::Full => "read, edit, run",
    }
}

/// `20m`, `90s`, `1h`.
pub fn secs_short(s: u64) -> String {
    match s {
        0..120 => format!("{s}s"),
        _ if s.is_multiple_of(3600) => format!("{}h", s / 3600),
        _ => format!("{}m", s / 60),
    }
}

/// Name of the step `id` refers to (`end` → End).
pub fn target_name(def: &PipelineDef, id: &str) -> String {
    match id {
        "" => "next step".into(),
        "end" => "End".into(),
        _ => def.step(id).map(|s| s.name.clone()).unwrap_or_else(|| format!("{id} · missing")),
    }
}

/// Rows of a step node; rows with ports match the edges in [`layout`].
pub fn rows_for(s: &StepDef, def: &PipelineDef) -> Vec<Row> {
    let wd = || {
        def.workdir(&s.workdir)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "none".into())
    };
    let model = || row("model", model_short(&s.model).to_string());
    match s.kind {
        StepKind::Agent => {
            let mut rows = vec![model(), row("works in", wd()), row("tools", tools_short(s.tools))];
            if let Some(d) = &s.delegate {
                rows.push(Row { label: "sub-agents", value: d.name.clone(), port: Some(PortKind::Spawn) });
            }
            rows
        }
        StepKind::Decision => vec![
            model(),
            row("works in", wd()),
            Row {
                label: "if no",
                value: target_name(def, &s.on_no),
                port: (!s.on_no.is_empty()).then_some(PortKind::Jump),
            },
        ],
        StepKind::Render => vec![row("works in", wd()), row("files", s.files.clone())],
        StepKind::Approval => vec![
            row("send back to", if s.back_to.is_empty() { "—".into() } else { target_name(def, &s.back_to) }),
            row("skip if empty", if s.skip_if_nothing_to_review { "yes" } else { "no" }),
        ],
        StepKind::Command => {
            let mut rows = vec![
                row("works in", wd()),
                row("runs", first_line(&s.command).to_string()),
                row("timeout", secs_short(s.timeout_secs)),
            ];
            if s.allow_failure {
                rows.push(row("on failure", "go on"));
            }
            rows
        }
        StepKind::Merge => vec![
            row("works in", wd()),
            row("into", def.workdir(&s.workdir).map(|w| w.base_branch.clone()).unwrap_or_default()),
        ],
        StepKind::PullRequest => vec![
            row("works in", wd()),
            row("push", if s.push { "origin" } else { "off" }),
            row("draft", if !s.push { "—" } else if s.draft { "yes" } else { "no" }),
        ],
        StepKind::Report => vec![row("posts to", "Inbox")],
    }
}

pub fn spawn_rows(d: &Delegate) -> Vec<Row> {
    vec![
        row("model", model_short(&d.model).to_string()),
        row("tools", tools_short(d.tools)),
        row("max steps", d.max_steps.to_string()),
    ]
}

/// Icon and tile colors of a step kind.
pub fn kind_look(kind: StepKind, delegating: bool) -> (&'static str, Hsla, Hsla) {
    match kind {
        StepKind::Agent if delegating => ("router", ACCENT_SOFT, ACCENT_TEXT),
        StepKind::Agent => ("agent", ACCENT_SOFT, ACCENT_TEXT),
        StepKind::Decision => ("filter", INFO_SOFT, INFO_TEXT),
        StepKind::Render => ("file", SURFACE_HOVER, TEXT_SECONDARY),
        StepKind::Approval => ("inbox", WARNING_SOFT, WARNING),
        StepKind::Command => ("play", SURFACE_HOVER, TEXT_SECONDARY),
        StepKind::Merge => ("git-branch", SURFACE_HOVER, TEXT_SECONDARY),
        StepKind::PullRequest => ("git-branch", SURFACE_HOVER, SUCCESS),
        StepKind::Report => ("message", SURFACE_HOVER, TEXT_SECONDARY),
    }
}

/// Kind line of a step, e.g. `Agent · leads Coder`.
pub fn kind_line(s: &StepDef) -> String {
    match (&s.delegate, s.kind) {
        (Some(d), StepKind::Agent) => format!("Agent · leads {}", d.name),
        _ => s.kind.label().to_string(),
    }
}

// ── Layout ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Step(usize),
    /// Sub-agents of step `i`.
    Spawn(usize),
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeBox {
    pub key: NodeKey,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NodeBox {
    pub fn in_pt(&self) -> (f32, f32) {
        (self.x, self.y + 21.)
    }
    pub fn out_pt(&self) -> (f32, f32) {
        (self.x + self.w, self.y + 21.)
    }
    pub fn row_pt(&self, row: usize) -> (f32, f32) {
        (self.x + self.w, self.y + 1. + TITLE_H + META_H + ROW_H * row as f32 + ROW_H / 2.)
    }
    pub fn top_mid(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y)
    }
    pub fn bottom_mid(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Next,
    Jump,
    Back,
    Spawn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EdgeGeo {
    pub kind: EdgeKind,
    pub from: NodeKey,
    pub to: NodeKey,
    /// Polyline (drawn with rounded corners); two points = a straight line.
    pub pts: Vec<(f32, f32)>,
    pub label: Option<&'static str>,
    pub label_at: (f32, f32),
}

pub struct Layout {
    pub nodes: Vec<NodeBox>,
    pub edges: Vec<EdgeGeo>,
    pub w: f32,
    pub h: f32,
}

#[cfg(test)]
impl Layout {
    pub fn node(&self, key: NodeKey) -> Option<&NodeBox> {
        self.nodes.iter().find(|n| n.key == key)
    }
}

/// Columns that fit a viewport `view_w` pixels wide at `zoom`.
pub fn cols_for(view_w: f32, zoom: f32) -> usize {
    let fit = (view_w / zoom.max(0.1) - 2. * MARGIN_X + COL_GAP) / (NODE_W + COL_GAP);
    (fit.floor().max(2.)) as usize
}

/// Middle of the longest horizontal segment (else the middle of the path).
fn label_point(pts: &[(f32, f32)]) -> (f32, f32) {
    let mut best: Option<((f32, f32), f32)> = None;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a.1 - b.1).abs() < 0.5 {
            let len = (a.0 - b.0).abs();
            if best.is_none_or(|(_, l)| len > l) {
                best = Some((((a.0 + b.0) / 2., a.1), len));
            }
        }
    }
    best.map(|(p, _)| p).unwrap_or_else(|| {
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        ((a.0 + b.0) / 2., (a.1 + b.1) / 2.)
    })
}

fn edge(kind: EdgeKind, from: NodeKey, to: NodeKey, pts: Vec<(f32, f32)>, label: Option<&'static str>) -> EdgeGeo {
    let label_at = label_point(&pts);
    EdgeGeo { kind, from, to, pts, label, label_at }
}

pub fn layout(def: &PipelineDef, cols: usize) -> Layout {
    let cols = cols.max(1);
    let n = def.steps.len();
    let count = n + 1;
    let col_x = |c: usize| MARGIN_X + c as f32 * (NODE_W + COL_GAP);

    // Rows: tops, the bottom of everything in them.
    let heights: Vec<f32> = def.steps.iter().map(|s| node_h(rows_for(s, def).len())).collect();
    let spawn_h = |i: usize| {
        def.steps[i]
            .delegate
            .as_ref()
            .filter(|_| def.steps[i].kind == StepKind::Agent)
            .map(|d| node_h(spawn_rows(d).len()))
    };
    let mut nodes = Vec::new();
    let mut row_of = vec![0usize; count];
    let mut node_y = Vec::new();
    let mut row_bottom = Vec::new();
    let mut top = MARGIN_TOP;
    for (r, chunk) in (0..count).collect::<Vec<_>>().chunks(cols).enumerate() {
        let y = top + LOOP_SPACE;
        let mut bottom = y + TITLE_H + 2.;
        for (c, &i) in chunk.iter().enumerate() {
            row_of[i] = r;
            if i == n {
                nodes.push(NodeBox { key: NodeKey::End, x: col_x(c), y, w: END_W, h: TITLE_H + 2. });
                continue;
            }
            let h = heights[i];
            nodes.push(NodeBox { key: NodeKey::Step(i), x: col_x(c), y, w: NODE_W, h });
            bottom = bottom.max(y + h);
            if let Some(sh) = spawn_h(i) {
                let sy = y + h + SPAWN_GAP;
                nodes.push(NodeBox { key: NodeKey::Spawn(i), x: col_x(c), y: sy, w: NODE_W, h: sh });
                bottom = bottom.max(sy + sh);
            }
        }
        node_y.push(y);
        row_bottom.push(bottom);
        top = bottom + ROW_GAP;
    }
    let rows = row_bottom.len();
    let used_cols = count.min(cols);
    let w = 2. * MARGIN_X + used_cols as f32 * NODE_W + (used_cols.saturating_sub(1)) as f32 * COL_GAP;
    let h = row_bottom.last().copied().unwrap_or(0.) + ROW_GAP;

    let find = |k: NodeKey| *nodes.iter().find(|b| b.key == k).expect("node");
    let item = |i: usize| if i == n { NodeKey::End } else { NodeKey::Step(i) };
    let g = COL_GAP;
    let mut edges = Vec::new();

    // Next: consecutive steps.
    for i in 0..n {
        let (a, b) = (find(item(i)), find(item(i + 1)));
        let (p, q) = (a.out_pt(), b.in_pt());
        let pts = if row_of[i] == row_of[i + 1] {
            vec![p, q]
        } else {
            let lane = row_bottom[row_of[i]] + ROW_GAP * 0.3;
            vec![p, (p.0 + g / 2., p.1), (p.0 + g / 2., lane), (q.0 - g / 2., lane), (q.0 - g / 2., q.1), q]
        };
        let s = &def.steps[i];
        let label = (s.kind == StepKind::Decision && !s.on_no.is_empty()).then_some("yes");
        edges.push(edge(EdgeKind::Next, item(i), item(i + 1), pts, label));
    }

    // Loops over the nodes: from the top of `from` to the top of `to`.
    let over = |a: NodeBox, b: NodeBox, ra: usize, rb: usize, k: usize| {
        let (p, q) = (a.top_mid(), b.top_mid());
        let lift = 24. + 8. * k as f32;
        if ra == rb {
            vec![p, (p.0, p.1 - lift), (q.0, p.1 - lift), q]
        } else {
            let xg = b.x - g * 0.62;
            let (la, lb) = (node_y[ra] - lift, node_y[rb] - lift);
            vec![p, (p.0, la), (xg, la), (xg, lb), (q.0, lb), q]
        }
    };

    let mut loops = vec![0usize; rows];
    for (i, s) in def.steps.iter().enumerate() {
        match s.kind {
            StepKind::Decision if !s.on_no.is_empty() => {
                let t = if s.on_no == "end" { Some(n) } else { def.step_index(&s.on_no) };
                let Some(t) = t else { continue };
                let (a, b) = (find(item(i)), find(item(t)));
                let port = rows_for(s, def).iter().position(|r| r.port == Some(PortKind::Jump)).unwrap_or(0);
                let pts = if t > i {
                    let p = a.row_pt(port);
                    let q = b.in_pt();
                    let lane = row_bottom[row_of[i]] + ROW_GAP * 0.62;
                    let (x1, xt) = (p.0 + g * 0.3, q.0 - g * 0.3);
                    vec![p, (x1, p.1), (x1, lane), (xt, lane), (xt, q.1), q]
                } else {
                    let k = loops[row_of[i]];
                    loops[row_of[i]] += 1;
                    over(a, b, row_of[i], row_of[t], k)
                };
                edges.push(edge(EdgeKind::Jump, item(i), item(t), pts, Some("no")));
            }
            StepKind::Approval if !s.back_to.is_empty() => {
                let Some(t) = def.step_index(&s.back_to) else { continue };
                if t == i {
                    continue;
                }
                let k = loops[row_of[i]];
                loops[row_of[i]] += 1;
                let pts = over(find(item(i)), find(item(t)), row_of[i], row_of[t], k);
                edges.push(edge(EdgeKind::Back, item(i), item(t), pts, Some("send back")));
            }
            StepKind::Agent if s.delegate.is_some() => {
                let (a, b) = (find(item(i)), find(NodeKey::Spawn(i)));
                edges.push(edge(
                    EdgeKind::Spawn,
                    item(i),
                    NodeKey::Spawn(i),
                    vec![a.bottom_mid(), b.top_mid()],
                    Some("delegate"),
                ));
            }
            _ => {}
        }
    }

    Layout { nodes, edges, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> PipelineDef {
        let mut d = pipeline::config::legacy_default();
        d.id = "t".into();
        d
    }

    #[test]
    fn every_step_gets_a_node_and_edges_follow_the_definition() {
        let d = def();
        let l = layout(&d, 4);
        let n = d.steps.len();
        assert!(l.node(NodeKey::End).is_some());
        for i in 0..n {
            assert!(l.node(NodeKey::Step(i)).is_some());
        }
        let count = |k| l.edges.iter().filter(|e| e.kind == k).count();
        assert_eq!(count(EdgeKind::Next), n);
        assert_eq!(count(EdgeKind::Jump), 1);
        assert_eq!(count(EdgeKind::Back), 1);
        assert_eq!(count(EdgeKind::Spawn), 1);
        // The triage "no" goes to Build.
        let build = d.step_index("build").unwrap();
        let jump = l.edges.iter().find(|e| e.kind == EdgeKind::Jump).unwrap();
        assert_eq!(jump.to, NodeKey::Step(build));
        assert_eq!(jump.pts.last().copied(), Some(l.node(NodeKey::Step(build)).unwrap().in_pt()));
    }

    #[test]
    fn rows_wrap_and_stay_inside_the_layer() {
        let d = def();
        for cols in 2..9 {
            let l = layout(&d, cols);
            for b in &l.nodes {
                assert!(b.x >= 0. && b.x + b.w <= l.w + 0.5, "{cols} {b:?}");
                assert!(b.y >= 0. && b.y + b.h <= l.h, "{cols} {b:?}");
            }
            for e in &l.edges {
                for p in &e.pts {
                    assert!(p.0 >= 0. && p.0 <= l.w && p.1 >= 0. && p.1 <= l.h, "{cols} {e:?}");
                }
            }
        }
        // 7 steps + End in 4 columns: two rows.
        let l = layout(&d, 4);
        let ys: std::collections::BTreeSet<i32> = l
            .nodes
            .iter()
            .filter(|b| !matches!(b.key, NodeKey::Spawn(_)))
            .map(|b| b.y as i32)
            .collect();
        assert_eq!(ys.len(), 2);
    }

    #[test]
    fn empty_pipeline_has_only_the_end() {
        let d = PipelineDef { id: "e".into(), ..Default::default() };
        let l = layout(&d, 4);
        assert_eq!(l.nodes.len(), 1);
        assert!(l.edges.is_empty());
        assert_eq!(cols_for(1000., 1.), 3);
    }

    #[test]
    fn decision_port_row_matches() {
        let d = def();
        let triage = d.step("triage").unwrap();
        let rows = rows_for(triage, &d);
        assert!(rows.iter().any(|r| r.port == Some(PortKind::Jump) && r.value == "Build"));
    }
}
