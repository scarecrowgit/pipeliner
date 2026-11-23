//! The node canvas: dot grid, edges (painted polylines with rounded corners and arrowheads),
//! edge labels, and one node per step (plus sub-agent nodes and End). Everything is laid out
//! in layer units by `layout` and multiplied by the zoom factor (GPUI has no element
//! transforms), so text, padding and strokes scale like a CSS `scale()`. Status dots and
//! words show the latest runs of this pipeline.

use gpui::{
    AnyElement, Bounds, BoxShadow, Context, ElementId, FontWeight, Hsla, IntoElement, MouseButton,
    PathBuilder, Pixels, SharedString, Window, canvas, div, fill, point, prelude::*, px, size,
};
use pipeline::{PipelineDef, StepKind};
use ui::{Status, icon, theme::*};

use crate::PipelineEditorScreen;
use crate::layout::*;
use crate::snap::{Counts, Snap};

/// Canvas viewport height: follows the content between these.
pub(crate) const VIEWPORT_MIN_H: f32 = 320.;
pub(crate) const VIEWPORT_MAX_H: f32 = 640.;

/// One edge ready to paint, in layer units.
struct EdgePaint {
    pts: Vec<(f32, f32)>,
    color: Hsla,
    width: f32,
    dash: Option<[f32; 2]>,
    /// Arrowhead stops this far before the end (port dot radius).
    inset: f32,
}

struct EdgeLabel {
    at: (f32, f32),
    text: &'static str,
    color: Hsla,
}

/// What a node shows, whatever it stands for.
struct NodeView {
    id: ElementId,
    title: String,
    icon: &'static str,
    tile: (Hsla, Hsla),
    meta: String,
    rows: Vec<Row>,
    counts: Counts,
    word: Option<(String, Hsla)>,
    spawn: bool,
    selected: bool,
    in_port: bool,
    out_port: bool,
    /// Step to select on click.
    select: Option<String>,
}

impl PipelineEditorScreen {
    fn edge_paints(&self, def: &PipelineDef, l: &Layout, snap: &Snap) -> (Vec<EdgePaint>, Vec<EdgeLabel>) {
        let sel = self.sel.as_deref().and_then(|id| def.step_index(id));
        let touches = |k: NodeKey| match (k, sel) {
            (NodeKey::Step(i) | NodeKey::Spawn(i), Some(s)) => i == s,
            _ => false,
        };
        let running = |k: NodeKey| match k {
            NodeKey::Step(i) => snap.counts(Some(&def.steps[i].id)).running > 0,
            NodeKey::Spawn(i) => snap.task_counts(&def.steps[i].id).running > 0,
            NodeKey::End => false,
        };
        let mut cold = Vec::new();
        let mut hot = Vec::new();
        let mut labels = Vec::new();
        for e in &l.edges {
            let (mut color, mut width, mut dash, mut inset) = (TEXT_DISABLED, 1.5, None, 5.);
            let mut label_color = TEXT_SECONDARY;
            match e.kind {
                EdgeKind::Next | EdgeKind::Jump => {}
                EdgeKind::Back => {
                    dash = Some([4., 4.]);
                    inset = 0.;
                    label_color = WARNING;
                }
                EdgeKind::Spawn => {
                    color = SPAWN;
                    dash = Some([6., 4.]);
                    inset = 0.;
                    label_color = SPAWN_TEXT;
                }
            }
            if e.kind != EdgeKind::Back && running(e.to) {
                color = if e.kind == EdgeKind::Spawn { SPAWN } else { INFO };
                width = 2.;
            }
            let is_hot = touches(e.from) || (touches(e.to) && e.kind != EdgeKind::Spawn);
            if is_hot {
                color = ACCENT;
                width = 2.;
            }
            if let Some(text) = e.label {
                labels.push(EdgeLabel { at: e.label_at, text, color: label_color });
            }
            let paint = EdgePaint { pts: e.pts.clone(), color, width, dash, inset };
            if is_hot { hot.push(paint) } else { cold.push(paint) }
        }
        // The selected step's edges are drawn on top.
        cold.extend(hot);
        (cold, labels)
    }

    pub(crate) fn render_canvas(&mut self, def: &PipelineDef, snap: &Snap, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let z = self.zoom;
        let l = layout(def, cols_for(self.view_w(), z));
        let (edges, labels) = self.edge_paints(def, &l, snap);
        let layer_paint = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_grid(bounds, z, window);
                for e in &edges {
                    paint_edge(bounds, z, e, window);
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let views: Vec<(NodeBox, NodeView)> = l.nodes.iter().map(|b| (*b, self.node_view(def, b.key, snap))).collect();
        let nodes: Vec<AnyElement> = views.into_iter().map(|(b, v)| self.render_node(b, v, cx)).collect();
        let empty = def.steps.is_empty();
        let view_h = (l.h * z + 4.).clamp(VIEWPORT_MIN_H, VIEWPORT_MAX_H);

        div()
            .id("canvas-scroll")
            .relative()
            .h(px(view_h))
            .flex_none()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .bg(BG)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.select(None, cx)))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px((l.w * z).max(self.view_w())))
                    .h(px((l.h * z).max(view_h)))
                    .child(layer_paint)
                    .children(labels.into_iter().map(|lb| edge_label(lb, z)))
                    .children(nodes)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(MARGIN_X * z + END_W * z + 32.))
                                .top(px((MARGIN_TOP + LOOP_SPACE) * z))
                                .max_w(px(360.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(div().font_weight(FontWeight::MEDIUM).child("No steps yet"))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(TEXT_MUTED)
                                        .child("Use “Add step” to add the first one: an agent, a decision, your approval, a command, a merge or a pull request."),
                                ),
                        )
                    }),
            )
    }

    fn node_view(&self, def: &PipelineDef, key: NodeKey, snap: &Snap) -> NodeView {
        match key {
            NodeKey::Step(i) => {
                let s = &def.steps[i];
                let (ic, bg, fg) = kind_look(s.kind, s.delegate.is_some());
                let counts = snap.counts(Some(&s.id));
                let done = match s.kind {
                    StepKind::Decision => "answered",
                    StepKind::Approval => "approved",
                    StepKind::PullRequest => "opened",
                    StepKind::Render => "rendered",
                    StepKind::Merge => "merged",
                    StepKind::Report => "posted",
                    _ => "done",
                };
                NodeView {
                    id: ElementId::Name(format!("node-{}", s.id).into()),
                    title: s.name.clone(),
                    icon: ic,
                    tile: (bg, fg),
                    meta: kind_line(s),
                    rows: rows_for(s, def),
                    word: counts.word(done),
                    counts,
                    spawn: false,
                    selected: self.sel.as_deref() == Some(s.id.as_str()),
                    in_port: true,
                    out_port: true,
                    select: Some(s.id.clone()),
                }
            }
            NodeKey::Spawn(i) => {
                let s = &def.steps[i];
                let d = s.delegate.clone().unwrap_or_default();
                let counts = snap.task_counts(&s.id);
                NodeView {
                    id: ElementId::Name(format!("spawn-{}", s.id).into()),
                    title: d.name.clone(),
                    icon: "spawn",
                    tile: (SPAWN_SOFT, SPAWN_TEXT),
                    meta: "Sub-agents".into(),
                    rows: spawn_rows(&d),
                    word: counts.word(if counts.done == 1 { "task" } else { "tasks" }),
                    counts,
                    spawn: true,
                    selected: false,
                    in_port: false,
                    out_port: false,
                    select: Some(s.id.clone()),
                }
            }
            NodeKey::End => {
                let counts = snap.counts(None);
                NodeView {
                    id: "node-end".into(),
                    title: "End".into(),
                    icon: "check",
                    tile: (SURFACE_HOVER, TEXT_SECONDARY),
                    meta: String::new(),
                    rows: Vec::new(),
                    word: None,
                    counts,
                    spawn: false,
                    selected: false,
                    in_port: true,
                    out_port: false,
                    select: None,
                }
            }
        }
    }

    fn render_node(&self, b: NodeBox, v: NodeView, cx: &mut Context<Self>) -> AnyElement {
        let z = self.zoom;
        let s = |x: f32| px(x * z);
        let running = v.counts.running > 0;
        let waiting = v.counts.waiting > 0;
        let border = if v.selected {
            ACCENT_RING
        } else if running {
            INFO
        } else if waiting {
            WARNING
        } else if v.spawn {
            SPAWN_LINE
        } else {
            LINE_STRONG
        };
        let glow = if v.selected {
            Some(ACCENT_GLOW)
        } else if running {
            Some(INFO_SOFT)
        } else if waiting {
            Some(WARNING_SOFT)
        } else {
            None
        };
        let is_end = v.select.is_none();

        let title = div()
            .flex()
            .items_center()
            .gap(s(8.))
            .w_full()
            .h(s(TITLE_H))
            .px(s(10.))
            .when(!is_end, |d| d.border_b_1().border_color(LINE_SUBTLE))
            .child(
                div()
                    .size(s(24.))
                    .flex_none()
                    .rounded(s(6.))
                    .bg(v.tile.0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(v.icon, 14. * z, v.tile.1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(SharedString::from(v.title)),
            )
            .when(is_end && v.counts.done > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(s(12.))
                        .text_color(SUCCESS)
                        .child(SharedString::from(v.counts.done.to_string())),
                )
            })
            .when(!is_end, |d| d.child(scaled_dot(v.counts.dot(), z)));

        let meta = div()
            .h(s(META_H))
            .px(s(10.))
            .flex()
            .items_center()
            .justify_between()
            .gap(s(6.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(s(12.))
                    .text_color(if v.spawn { SPAWN_TEXT } else { TEXT_MUTED })
                    .child(SharedString::from(v.meta)),
            )
            .when_some(v.word, |d, (w, color)| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(s(12.))
                        .text_color(color)
                        .child(SharedString::from(w)),
                )
            });

        let row_els: Vec<_> = v
            .rows
            .into_iter()
            .map(|r| {
                let spawn = r.port == Some(PortKind::Spawn);
                div()
                    .relative()
                    .h(s(ROW_H))
                    .px(s(10.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(s(8.))
                    .border_t_1()
                    .border_color(LINE_SUBTLE)
                    .text_size(s(12.))
                    .line_height(s(18.))
                    .child(div().flex_none().text_color(TEXT_MUTED).whitespace_nowrap().child(r.label))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_color(if spawn { SPAWN_TEXT } else { TEXT_SECONDARY })
                            .child(SharedString::from(r.value)),
                    )
                    .when(r.port == Some(PortKind::Jump), |d| {
                        d.child(port_dot(TEXT_MUTED, z).right(s(-12.)).top_0())
                    })
            })
            .collect();

        let select = v.select.clone();
        div()
            .id(v.id)
            .absolute()
            .left(s(b.x))
            .top(s(b.y))
            .w(s(b.w))
            .when(!is_end, |d| d.pb(s(4.)))
            .bg(SURFACE)
            .border_1()
            .when(v.spawn, |d| d.border_dashed())
            .border_color(border)
            .rounded(s(10.))
            .when_some(glow, |d, color| {
                d.shadow(vec![BoxShadow {
                    color,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: s(3.),
                }])
            })
            .text_color(TEXT)
            .text_size(s(13.))
            .line_height(s(20.))
            .when(!is_end, |d| d.cursor_pointer().hover(|st| st.border_color(if v.selected { ACCENT_RING } else { TEXT_DISABLED })))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.select(select.clone(), cx);
                }),
            )
            .child(title)
            .when(!is_end, |d| d.child(meta).children(row_els))
            .when(v.in_port, |d| d.child(port_dot(TEXT_MUTED, z).left(s(-12.)).top(s(9.))))
            .when(v.out_port, |d| d.child(port_dot(TEXT_MUTED, z).right(s(-12.)).top(s(9.))))
            .into_any_element()
    }
}

/// 24×24 box with a 10px ring dot.
fn port_dot(color: Hsla, z: f32) -> gpui::Div {
    div()
        .absolute()
        .size(px(24. * z))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(10. * z))
                .rounded_full()
                .bg(BG)
                .border(px(2. * z))
                .border_color(color),
        )
}

/// Status dot scaled with the canvas (6px base).
fn scaled_dot(status: Status, z: f32) -> gpui::Div {
    let d = div().size(px(6. * z)).flex_none().rounded_full();
    if status == Status::Draft {
        d.border(px(1.5 * z)).border_color(TEXT_MUTED)
    } else {
        d.bg(status.color())
    }
}

/// Label pill centered on a point of the edge.
fn edge_label(l: EdgeLabel, z: f32) -> impl IntoElement {
    let s = |v: f32| px(v * z);
    div()
        .absolute()
        .left(s(l.at.0 - 60.))
        .top(s(l.at.1 - 10.))
        .w(s(120.))
        .h(s(20.))
        .flex()
        .justify_center()
        .child(
            div()
                .h(s(20.))
                .px(s(7.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(BG)
                .border_1()
                .border_color(LINE_STRONG)
                .font_family(FONT_MONO)
                .text_size(s(12.))
                .text_color(l.color)
                .whitespace_nowrap()
                .child(l.text),
        )
}

/// Dots every 20px; only those inside the visible (clipped) area are painted.
fn paint_grid(bounds: Bounds<Pixels>, z: f32, window: &mut Window) {
    let step = 20. * z;
    let d = (2.4 * z).max(1.2);
    let visible = bounds.intersect(&window.content_mask().bounds);
    if f32::from(visible.size.width) <= 0. || f32::from(visible.size.height) <= 0. {
        return;
    }
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let first_col = (((f32::from(visible.left()) - ox) / step).floor().max(0.)) as i32;
    let last_col = (((f32::from(visible.right()) - ox) / step).ceil()) as i32;
    let first_row = (((f32::from(visible.top()) - oy) / step).floor().max(0.)) as i32;
    let last_row = (((f32::from(visible.bottom()) - oy) / step).ceil()) as i32;
    for row in first_row..last_row {
        for col in first_col..last_col {
            let cx = ox + (col as f32 + 0.5) * step;
            let cy = oy + (row as f32 + 0.5) * step;
            window.paint_quad(
                fill(Bounds::new(point(px(cx - d / 2.), px(cy - d / 2.)), size(px(d), px(d))), GRID_DOT)
                    .corner_radii(px(d / 2.)),
            );
        }
    }
}

fn unit(a: (f32, f32), b: (f32, f32)) -> ((f32, f32), f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 { ((0., 0.), 0.) } else { ((dx / len, dy / len), len) }
}

/// The polyline with rounded corners, then an arrowhead at its end.
fn paint_edge(bounds: Bounds<Pixels>, z: f32, e: &EdgePaint, window: &mut Window) {
    let n = e.pts.len();
    if n < 2 {
        return;
    }
    let at = |p: (f32, f32)| point(bounds.origin.x + px(p.0 * z), bounds.origin.y + px(p.1 * z));
    // The last segment stops at the arrowhead's base.
    let (dir, last_len) = unit(e.pts[n - 2], e.pts[n - 1]);
    let head = 7.;
    let back = (e.inset + head).min(last_len);
    let tip = (e.pts[n - 1].0 - dir.0 * e.inset, e.pts[n - 1].1 - dir.1 * e.inset);
    let base = (e.pts[n - 1].0 - dir.0 * back, e.pts[n - 1].1 - dir.1 * back);

    let mut path = PathBuilder::stroke(px(e.width * z));
    if let Some([on, off]) = e.dash {
        path = path.dash_array(&[px(on * z), px(off * z)]);
    }
    path.move_to(at(e.pts[0]));
    for i in 1..n - 1 {
        let (p0, p, p1) = (e.pts[i - 1], e.pts[i], e.pts[i + 1]);
        let (din, lin) = unit(p0, p);
        let (dout, lout) = unit(p, p1);
        let r = 10f32.min(lin / 2.).min(lout / 2.);
        path.line_to(at((p.0 - din.0 * r, p.1 - din.1 * r)));
        path.curve_to(at((p.0 + dout.0 * r, p.1 + dout.1 * r)), at(p));
    }
    path.line_to(at(base));
    if let Ok(path) = path.build() {
        window.paint_path(path, e.color);
    }

    let norm = (-dir.1, dir.0);
    let w = 4.;
    let mut arrow = PathBuilder::fill();
    arrow.move_to(at(tip));
    arrow.line_to(at((base.0 + norm.0 * w, base.1 + norm.1 * w)));
    arrow.line_to(at((base.0 - norm.0 * w, base.1 - norm.1 * w)));
    arrow.close();
    if let Ok(arrow) = arrow.build() {
        window.paint_path(arrow, e.color);
    }
}
