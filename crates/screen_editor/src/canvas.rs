//! The node canvas: dot grid, bezier edges (painted), edge count labels, nodes with
//! ports. Everything is laid out in layer units and multiplied by the zoom factor
//! (GPUI has no element transforms), so text, padding and strokes scale like the
//! mockup's `transform: scale()`.

use gpui::{
    AnyElement, Bounds, BoxShadow, Context, CursorStyle, DispatchPhase, ElementId, FontWeight,
    Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    Window, canvas, div, fill, point, prelude::*, px, size,
};
use ui::{Status, icon, theme::*};

use crate::PipelineEditorScreen;
use crate::data::*;

/// Canvas viewport height (`height: 600px; overflow: auto`).
const VIEWPORT_H: f32 = 600.;

/// A key/value row inside a node.
pub(crate) struct Row {
    pub label: String,
    pub value: String,
    /// Has an output port on the right.
    pub port: bool,
    /// Spawn-tinted value + port.
    pub spawn: bool,
}

impl Row {
    fn new(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            port: false,
            spawn: false,
        }
    }
    fn port(mut self) -> Self {
        self.port = true;
        self
    }
    fn spawn(mut self) -> Self {
        self.port = true;
        self.spawn = true;
        self
    }
}

/// One edge ready to paint, in layer units.
struct EdgePaint {
    a: (f32, f32),
    b: (f32, f32),
    color: Hsla,
    width: f32,
    dash: Option<[f32; 2]>,
}

struct EdgeLabel {
    x: f32,
    y: f32,
    text: String,
}

impl PipelineEditorScreen {
    pub(crate) fn rows_for(&self, n: &Node) -> Vec<Row> {
        match n.kind() {
            Kind::Source => vec![
                Row::new("queued", self.queued().to_string()),
                Row::new(
                    "poll",
                    format!("every {}", if n.every.is_empty() { "2m" } else { &n.every }),
                ),
            ],
            Kind::Router => {
                let mut rows: Vec<Row> = ROUTES
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let value = r.iter().map(|c| c.2).collect::<Vec<_>>().join(" ≥ ");
                        Row::new(&format!("rule {}", i + 1), value).port()
                    })
                    .collect();
                rows[0].value = "Bug ≥High".into();
                rows.push(Row::new("else", "skip").port());
                rows
            }
            Kind::Agent => vec![
                Row::new("tools", n.tools.to_string()),
                if n.can_spawn {
                    Row::new("spawns", format!("depth ≤{}", n.depth)).spawn()
                } else {
                    Row::new("spawns", "off")
                },
            ],
            Kind::Spawn => {
                let parent = n.parent.as_deref().and_then(|p| self.by_id(p));
                vec![
                    Row::new("per run", format!("≤{}", n.max)),
                    Row::new(
                        "returns to",
                        parent.map(|p| p.title.clone()).unwrap_or("—".into()),
                    ),
                ]
            }
            Kind::Orch => vec![
                Row::new("runs", format!("{} agents", n.team.len())),
                if n.can_create {
                    Row::new("creates", format!("≤{} alive", n.max_alive)).spawn()
                } else {
                    Row::new("creates", "off")
                },
            ],
            Kind::Pipe => vec![
                Row::new("calls", n.reference.clone()),
                Row::new("returns", n.returns.clone()),
            ],
            Kind::File => match n.mode {
                FileMode::Read => vec![Row::new("mode", "read"), Row::new("matched", "14 files")],
                FileMode::Write => {
                    vec![Row::new("mode", "write"), Row::new("copies", "per variant")]
                }
            },
            Kind::Gate => vec![
                Row::new("reviewers", "oncall"),
                Row::new("on approve", "ship").port(),
                Row::new("on send back", "re-run"),
            ],
            Kind::Action => n.rows.iter().map(|(l, v)| Row::new(l, v.clone())).collect(),
        }
    }

    pub(crate) fn meta_for(&self, n: &Node) -> String {
        match n.kind() {
            Kind::Source => "PAY · JQL".into(),
            Kind::Router => "first match wins".into(),
            Kind::Agent => format!("{} · ×{}", n.model, n.variants),
            Kind::Orch => format!("{} · decides at runtime", n.model),
            Kind::Pipe => "pipeline as agent".into(),
            Kind::Spawn => {
                let parent = n.parent.as_deref().and_then(|p| self.by_id(p));
                format!(
                    "by {}",
                    parent.map(|p| p.title.as_str()).unwrap_or("any agent")
                )
            }
            Kind::File => n.glob.clone(),
            Kind::Gate => if self.pick_each {
                "approve each"
            } else {
                "ship 1 of N"
            }
            .into(),
            Kind::Action => n.meta.clone(),
        }
    }

    fn edge_paints(&self) -> (Vec<EdgePaint>, Vec<EdgeLabel>) {
        let sel = self.sel.as_deref();
        let mut cold = Vec::new();
        let mut hot = Vec::new();
        let mut labels = Vec::new();
        for e in &self.edges {
            let (Some(f), Some(t)) = (self.by_id(&e.from), self.by_id(&e.to)) else {
                continue;
            };
            let is_spawn = matches!(f.kind(), Kind::Agent | Kind::Orch) && e.port == 1;
            if is_spawn && !f.spawns() {
                continue;
            }
            let is_data = f.kind() == Kind::File && f.mode == FileMode::Read;
            let (a, b) = (out_pt(f, e.port), in_pt(t));
            let (ts, fs) = (self.status_of(&t.id), self.status_of(&f.id));
            let is_hot = sel == Some(e.from.as_str()) || sel == Some(e.to.as_str());
            let (mut color, mut width, mut dash) = (TEXT_DISABLED, 1.5, None);
            if is_data {
                dash = Some([2., 5.]);
            }
            if is_spawn {
                color = SPAWN;
                dash = Some([6., 4.]);
            }
            let active = matches!(ts, RunStatus::Running | RunStatus::Spawned)
                || (fs == RunStatus::Running && !is_spawn);
            if self.step > 0 && active && !is_data {
                color = if is_spawn { SPAWN } else { INFO };
                width = 2.;
            }
            if is_hot {
                color = ACCENT;
                width = 2.;
            }
            let paint = EdgePaint {
                a,
                b,
                color,
                width,
                dash,
            };
            if is_hot {
                hot.push(paint)
            } else {
                cold.push(paint)
            }

            let (mx, my) = (((a.0 + b.0) / 2.).round(), ((a.1 + b.1) / 2.).round());
            match (f.kind(), t.kind()) {
                (Kind::Orch, Kind::Gate) => labels.push(EdgeLabel {
                    x: mx,
                    y: my,
                    text: "plan + results".into(),
                }),
                (Kind::Agent, Kind::Gate) => labels.push(EdgeLabel {
                    x: mx,
                    y: my,
                    text: format!("×{}", f.variants),
                }),
                (Kind::File, Kind::Gate) if f.mode == FileMode::Write => {
                    let up = self
                        .edges
                        .iter()
                        .filter(|x| x.to == f.id)
                        .filter_map(|x| self.by_id(&x.from))
                        .find(|x| x.kind() == Kind::Agent);
                    if let Some(up) = up {
                        labels.push(EdgeLabel {
                            x: mx + 18.,
                            y: my,
                            text: format!("×{}", up.variants),
                        });
                    }
                }
                _ => {}
            }
        }
        // Selected node's edges are drawn on top.
        cold.extend(hot);
        (cold, labels)
    }

    pub(crate) fn render_canvas(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let z = self.zoom;
        let (mut edges, labels) = self.edge_paints();
        if let Some(link) = &self.link
            && let Some(src) = self.by_id(&link.from)
        {
            edges.push(EdgePaint {
                a: out_pt(src, link.port),
                b: (link.pt.0.round(), link.pt.1.round()),
                color: ACCENT,
                width: 2.,
                dash: Some([4., 4.]),
            });
        }

        let origin = self.layer_origin.clone();
        let interacting = self.is_interacting();
        let this = cx.weak_entity();
        let layer_paint = canvas(
            move |bounds, _, _| origin.set(bounds.origin),
            move |bounds, _, window, _| {
                paint_grid(bounds, z, window);
                for e in &edges {
                    paint_edge(bounds, z, e, window);
                }
                if !interacting {
                    return;
                }
                // Track the pointer window-wide while dragging a node or a link.
                let this_move = this.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    this_move
                        .update(cx, |this, cx| {
                            if ev.pressed_button == Some(MouseButton::Left) {
                                this.pointer_move(ev.position, cx);
                            } else {
                                this.pointer_up(ev.position, cx);
                            }
                        })
                        .ok();
                });
                let this_up = this.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                        this_up
                            .update(cx, |this, cx| this.pointer_up(ev.position, cx))
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let nodes: Vec<AnyElement> = self.nodes.iter().map(|n| self.render_node(n, cx)).collect();

        div()
            .id("canvas-scroll")
            .relative()
            .h(px(VIEWPORT_H))
            .flex_none()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .bg(BG)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.select(None, cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(LAYER_W * z))
                    .h(px(LAYER_H * z))
                    .child(layer_paint)
                    .children(labels.into_iter().map(|l| edge_label(l, z)))
                    .children(nodes),
            )
    }

    fn render_node(&self, n: &Node, cx: &mut Context<Self>) -> AnyElement {
        let z = self.zoom;
        let s = |v: f32| px(v * z);
        let kind = n.kind();
        let info = kind.info();
        let status = self.status_of(&n.id);
        let is_sel = self.sel.as_deref() == Some(n.id.as_str());
        let dimmed = kind == Kind::Spawn
            && n.parent
                .as_deref()
                .and_then(|p| self.by_id(p))
                .is_some_and(|p| !p.can_spawn);
        let status_word = if kind == Kind::Agent && status == RunStatus::Running {
            format!("running ×{}", n.variants)
        } else {
            status.word().to_string()
        };
        let border = if is_sel {
            ACCENT_RING
        } else if kind == Kind::Spawn {
            SPAWN_LINE
        } else {
            LINE_STRONG
        };
        let id = n.id.clone();
        let rows = self.rows_for(n);

        let title = {
            let id = id.clone();
            div()
                .id(ElementId::Name(format!("{}-head", n.id).into()))
                .flex()
                .items_center()
                .gap(s(8.))
                .w_full()
                .h(s(40.))
                .px(s(10.))
                .border_b_1()
                .border_color(LINE_SUBTLE)
                .rounded_t(s(9.))
                .cursor(CursorStyle::OpenHand)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.grab(&id, ev.position, cx);
                    }),
                )
                .child(
                    div()
                        .size(s(24.))
                        .flex_none()
                        .rounded(s(6.))
                        .bg(info.tile_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(info.icon, 14. * z, info.tile_fg)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(n.title.clone()),
                )
                .child(scaled_dot(status.dot(), z))
        };

        let meta = div()
            .h(s(26.))
            .px(s(10.))
            .flex()
            .items_center()
            .justify_between()
            .gap(s(6.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(s(12.))
                    .text_color(TEXT_MUTED)
                    .child(self.meta_for(n)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(s(12.))
                    .text_color(status.color())
                    .child(status_word),
            );

        let row_els = rows.into_iter().enumerate().map(|(i, r)| {
            let port_color = if r.spawn { SPAWN } else { TEXT_MUTED };
            div()
                .relative()
                .h(s(26.))
                .px(s(10.))
                .flex()
                .items_center()
                .justify_between()
                .gap(s(8.))
                .border_t_1()
                .border_color(LINE_SUBTLE)
                .text_size(s(12.))
                .line_height(s(18.))
                .child(
                    div()
                        .flex_none()
                        .text_color(TEXT_MUTED)
                        .whitespace_nowrap()
                        .child(r.label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_color(if r.spawn { SPAWN_TEXT } else { TEXT_SECONDARY })
                        .child(r.value),
                )
                .when(r.port, |d| {
                    d.child(
                        self.out_port(&n.id, i as i32, port_color, cx)
                            .right(s(-12.))
                            .top_0(),
                    )
                })
        });
        let row_els: Vec<_> = row_els.collect();

        div()
            .id(ElementId::Name(format!("node-{}", n.id).into()))
            .absolute()
            .left(s(n.x))
            .top(s(n.y))
            .w(s(NODE_W))
            .pb(s(4.))
            .bg(SURFACE)
            .border_1()
            .when(kind == Kind::Spawn, |d| d.border_dashed())
            .border_color(border)
            .rounded(s(10.))
            .when(is_sel, |d| {
                d.shadow(vec![BoxShadow {
                    color: ACCENT_GLOW,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: s(3.),
                }])
            })
            .when(dimmed, |d| d.opacity(0.45))
            .text_color(TEXT)
            .text_size(s(13.))
            .line_height(s(20.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.select(Some(id.clone()), cx);
                }),
            )
            .child(title)
            .child(meta)
            .children(row_els)
            .when(n.has_in(), |d| {
                let color = if kind == Kind::Spawn {
                    SPAWN
                } else {
                    TEXT_MUTED
                };
                d.child(
                    port_shell(&format!("{}-in", n.id), color, z)
                        .left(s(-12.))
                        .top(s(8.)),
                )
            })
            .when(n.has_out(), |d| {
                d.child(
                    self.out_port(&n.id, -1, TEXT_MUTED, cx)
                        .right(s(-12.))
                        .top(s(8.)),
                )
            })
            .into_any_element()
    }

    /// Output port: pointer down starts a link.
    fn out_port(
        &self,
        node_id: &str,
        port: i32,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = node_id.to_string();
        port_shell(&format!("{node_id}-out{port}"), color, self.zoom).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.start_link(&id, port, ev.position, cx);
            }),
        )
    }
}

/// 24×24 hit area with a 10px ring dot (`.pl-port` / `.pl-dot`); fills on hover.
fn port_shell(key: &str, color: Hsla, z: f32) -> gpui::Stateful<gpui::Div> {
    let group: gpui::SharedString = format!("port-{key}").into();
    div()
        .id(ElementId::Name(key.to_string().into()))
        .group(group.clone())
        .absolute()
        .size(px(24. * z))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::Crosshair)
        .child(
            div()
                .size(px(10. * z))
                .rounded_full()
                .bg(BG)
                .border(px(2. * z))
                .border_color(color)
                .group_hover(group, move |s| s.bg(color)),
        )
}

/// Status dot scaled with the canvas (6px base, `size="sm"`).
fn scaled_dot(status: Status, z: f32) -> gpui::Div {
    let d = div().size(px(6. * z)).flex_none().rounded_full();
    if status == Status::Draft {
        d.border(px(1.5 * z)).border_color(TEXT_MUTED)
    } else {
        d.bg(status.color())
    }
}

/// `×3` count pill centered on the edge midpoint.
fn edge_label(l: EdgeLabel, z: f32) -> impl IntoElement {
    let s = |v: f32| px(v * z);
    div()
        .absolute()
        .left(s(l.x - 60.))
        .top(s(l.y - 10.))
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
                .text_color(TEXT_SECONDARY)
                .whitespace_nowrap()
                .child(l.text),
        )
}

/// `radial-gradient(circle, #1f2024 1px, transparent 1.4px)` every 20px; only the
/// dots inside the visible (clipped) area are painted.
fn paint_grid(bounds: Bounds<gpui::Pixels>, z: f32, window: &mut Window) {
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
                fill(
                    Bounds::new(point(px(cx - d / 2.), px(cy - d / 2.)), size(px(d), px(d))),
                    GRID_DOT,
                )
                .corner_radii(px(d / 2.)),
            );
        }
    }
}

fn paint_edge(bounds: Bounds<gpui::Pixels>, z: f32, e: &EdgePaint, window: &mut Window) {
    let at = |p: (f32, f32)| point(bounds.origin.x + px(p.0 * z), bounds.origin.y + px(p.1 * z));
    let (c1, c2) = bez_controls(e.a, e.b);
    let mut path = PathBuilder::stroke(px(e.width * z));
    if let Some([on, off]) = e.dash {
        path = path.dash_array(&[px(on * z), px(off * z)]);
    }
    path.move_to(at(e.a));
    path.cubic_bezier_to(at(e.b), at(c1), at(c2));
    if let Ok(path) = path.build() {
        window.paint_path(path, e.color);
    }
}
