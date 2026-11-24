//! Drawing the canvas: the dot grid, the edges, the nodes with their marks, the ports a drag
//! can reach, the legend and the floating controls.

use gpui::{
    AnyElement, BoxShadow, Bounds, Context, DispatchPhase, ElementId, Hsla, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent, Point, Render, Window, canvas, div, point, prelude::*, px, size,
};
use hub::model::EdgeKind;
use ui::assets::IconName;
use ui::components::{Button, IconButton, TileTone};
use ui::dev::Probe;
use ui::motion;
use ui::overlay::PopoverAnchorExt;
use ui::theme::*;
use ui_rich::canvas::{
    BACK_DASH, Curve, DotGrid, Edges, Legend, NodeFrame, NodeState, PORT_HIT_RADIUS, PORT_SIZE, SPAWN_DASH, Viewport,
    back_curve, paint_curve, route_curve,
};

use super::interaction::Gesture;
use super::marks::{self, InstanceMark};
use super::scene::Scene;
use super::{CanvasView, FIT_PADDING};
use crate::Selection;

/// The key the add button records its bounds under, for the menu's anchor.
pub(super) const ADD_ANCHOR: &str = "canvas-add";
/// Canvas units kept around the view when deciding what to lay out: room for status pills.
const CULL_MARGIN: f32 = 40.0;
/// The smallest port target, px, so ports stay reachable when zoomed out.
const MIN_PORT_TARGET: f32 = 12.0;
/// The glow of a selected node (`--selected-glow`, a 3px ring).
const GLOW: f32 = 3.0;
/// Width of the zoom percentage in the zoom control, px.
const PERCENT_WIDTH: f32 = 40.0;

/// The edge being drawn, painted over the others.
#[derive(Clone, Copy)]
struct Overlay {
    curve: Curve,
    color: Hsla,
    dash: Option<[f32; 2]>,
}

/// A transparent target centred on a port, named for the dev driver; during a drag it rings
/// the ports the edge may end on.
fn port_target(vp: Viewport, at: Point<f32>, name: String, ring: Option<(Hsla, bool)>) -> AnyElement {
    let s = vp.to_screen(at);
    let target = f32::from(vp.z(PORT_HIT_RADIUS * 2.0)).max(MIN_PORT_TARGET);
    let selector = name.clone();
    div()
        .debug_selector(move || selector)
        .absolute()
        .left(px(s.x - target / 2.0))
        .top(px(s.y - target / 2.0))
        .size(px(target))
        .flex()
        .items_center()
        .justify_center()
        .when_some(ring, |d, (color, strong)| {
            let ring = if strong { PORT_SIZE + 8.0 } else { PORT_SIZE + 4.0 };
            d.child(
                div()
                    .flex_none()
                    .size(vp.z(ring))
                    .rounded_full()
                    .border(vp.z(2.0))
                    .border_color(color)
                    .bg(if strong { ACCENT_SOFT } else { TRANSPARENT }),
            )
        })
        .probe(name)
        .into_any_element()
}

impl CanvasView {
    /// Fits the graph while the view follows it, once the canvas has a (new) size and the
    /// graph has nodes.
    fn fit_if_pending(&mut self, scene: &Scene) {
        if !self.fitting {
            return;
        }
        if let (Some(size), Some(content)) = (self.size(), scene.bounds())
            && self.fitted != Some(size)
        {
            self.viewport = Viewport::fit(content, size, FIT_PADDING);
            self.fitted = Some(size);
        }
    }

    /// Records where the canvas is, and while a gesture runs follows the pointer anywhere in
    /// the window (a drag may leave the canvas).
    fn recorder(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cell = self.bounds.clone();
        let refit = self.fitting;
        let id = cx.entity_id();
        let entity = cx.entity().downgrade();
        let gesture = self.gesture.is_some();
        canvas(
            move |bounds, window, _| {
                let changed = cell.get() != Some(bounds);
                cell.set(Some(bounds));
                if changed && refit {
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            move |_, (), window, _| {
                if !gesture {
                    return;
                }
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        moved.update(cx, |this, cx| this.pointer_moved(event, window, cx)).ok();
                    }
                });
                let released = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        released.update(cx, |this, cx| this.pointer_released(event, window, cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    fn zoom_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let (out, inn, fit) = (this.clone(), this.clone(), this);
        div()
            .absolute()
            .right(px(SPACE_4))
            .bottom(px(SPACE_3))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SPACE_2))
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(CONTROL_HEIGHT_SM))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(SURFACE)
                    .child(
                        IconButton::new("canvas-zoom-out", IconName::Minus)
                            .small()
                            .tooltip("Zoom out")
                            .on_click(move |_, _, cx| {
                                out.update(cx, |v, cx| v.zoom_out(cx)).ok();
                            })
                            .probe("canvas-zoom-out"),
                    )
                    .child(
                        div()
                            .w(px(PERCENT_WIDTH))
                            .flex()
                            .justify_center()
                            .type_style(MONO_SM)
                            .text_color(TEXT_SECONDARY)
                            .child(format!("{}%", self.viewport.percent()))
                            .probe_value("canvas-zoom", self.viewport.percent().to_string()),
                    )
                    .child(
                        IconButton::new("canvas-zoom-in", IconName::Plus)
                            .small()
                            .tooltip("Zoom in")
                            .on_click(move |_, _, cx| {
                                inn.update(cx, |v, cx| v.zoom_in(cx)).ok();
                            })
                            .probe("canvas-zoom-in"),
                    ),
            )
            .child(
                Button::new("canvas-fit", "Fit")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| {
                        fit.update(cx, |v, cx| v.zoom_to_fit(cx)).ok();
                    })
                    .probe("canvas-fit"),
            )
    }

    fn add_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        div().absolute().left(px(SPACE_4)).top(px(SPACE_3)).occlude().child(
            Button::new("canvas-add", "Add node")
                .secondary()
                .small()
                .icon(IconName::Plus)
                .on_click(move |_, window, cx| {
                    this.update(cx, |v, cx| v.open_add_menu(window, cx)).ok();
                })
                .popover_anchor(ADD_ANCHOR)
                .probe("canvas-add"),
        )
    }

    /// The edge being drawn, and which nodes it may end on.
    fn pending_overlay(&self, scene: &Scene) -> Option<Overlay> {
        let Some(Gesture::Connect(connect)) = &self.gesture else { return None };
        let end = connect
            .target
            .as_ref()
            .and_then(|(id, _)| scene.node(id))
            .and_then(|node| node.in_port())
            .unwrap_or(connect.pointer);
        let kind = connect.target.as_ref().map(|(_, kind)| *kind);
        let curve = match kind {
            Some(EdgeKind::Back) => Curve::Cubic(back_curve(connect.start, end)),
            _ => Curve::Cubic(route_curve(connect.start, end)),
        };
        let dash = match kind {
            Some(EdgeKind::Back) => Some(BACK_DASH),
            Some(EdgeKind::Spawn) => Some(SPAWN_DASH),
            _ => None,
        };
        let color = match kind {
            Some(EdgeKind::Spawn) => SPAWN_LINE,
            Some(_) => ACCENT,
            None => TEXT_MUTED,
        };
        Some(Overlay { curve, color, dash })
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (scene, selection, problems, empty, loaded) = {
            let draft = self.draft.read(cx);
            let selection = draft.selection().clone();
            let scene = draft.graph().map(|g| Scene::build(g, &selection)).unwrap_or_default();
            let empty = draft.graph().is_some_and(|g| g.nodes.is_empty());
            (scene, selection, marks::problem_marks(draft.problems()), empty, draft.graph().is_some())
        };
        let instances = self
            .run
            .as_ref()
            .and_then(|(_, model, _)| model.read(cx).run().map(|run| marks::instance_marks(run)))
            .unwrap_or_default();
        self.fit_if_pending(&scene);
        let vp = self.viewport.sanitized();
        let visible = self.size().map(|s| {
            let v = vp.visible(s);
            Bounds::new(
                point(v.origin.x - CULL_MARGIN, v.origin.y - CULL_MARGIN),
                size(v.size.width + 2.0 * CULL_MARGIN, v.size.height + 2.0 * CULL_MARGIN),
            )
        });
        let in_view = |b: &Bounds<f32>| visible.as_ref().is_none_or(|v| v.intersects(b));

        let pending = self.pending_overlay(&scene);
        let overlay_layer = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                if let Some(o) = &pending {
                    paint_curve(window, vp, bounds.origin, o.curve, o.color, o.dash);
                }
            },
        )
        .absolute()
        .size_full();

        let connect = match &self.gesture {
            Some(Gesture::Connect(connect)) => Some(connect.clone()),
            _ => None,
        };
        let selected_id = match &selection {
            Selection::Node(id) => Some(id.clone()),
            _ => None,
        };

        #[cfg(test)]
        self.drawn.clear();
        let mut nodes: Vec<AnyElement> = Vec::new();
        let mut targets: Vec<AnyElement> = Vec::new();
        let mut marks_layer: Vec<AnyElement> = Vec::new();
        for (ix, node) in scene.nodes.iter().enumerate() {
            if !in_view(&node.shape.bounds()) {
                continue;
            }
            let look = &node.look;
            let selected = selected_id.as_ref() == Some(&node.id);
            let state = if selected {
                NodeState::Selected
            } else if look.spawned {
                NodeState::Spawned
            } else if look.soon {
                NodeState::Soon
            } else {
                NodeState::Default
            };
            let mut frame = NodeFrame::new(ElementId::NamedInteger("node".into(), ix as u64), vp, node.shape.origin)
                .icon(look.icon)
                .tone(if selected { TileTone::Accent } else { look.tone })
                .title(node.name.clone())
                .subtitle(look.subtitle.clone())
                .state(state)
                .input(look.input)
                .outputs(look.outputs);
            for (key, value) in &look.rows {
                frame = frame.row(key.clone(), value.clone());
            }
            let rest = if look.spawned { SPAWN_LINE } else { LINE_STRONG };
            let border = motion::fade_color(
                ElementId::Name(format!("node-border-{}", node.id).into()),
                if selected { ACCENT } else { rest },
                window,
                cx,
            );
            let glow = motion::fade_value(
                ElementId::Name(format!("node-glow-{}", node.id).into()),
                if selected { 1.0 } else { 0.0 },
                window,
                cx,
            );
            let shadow = if glow > 0.001 {
                vec![BoxShadow {
                    color: Hsla { a: ACCENT_GLOW.a * glow, ..ACCENT_GLOW },
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: vp.z(GLOW),
                }]
            } else {
                Vec::new()
            };
            let name = format!("node-{}", node.id);
            let selector = name.clone();
            nodes.push(
                frame
                    .build()
                    .border_color(border)
                    .shadow(shadow)
                    .debug_selector(move || selector)
                    .probe(name)
                    .into_any_element(),
            );

            // Ports: named targets, ringed while an edge is drawn and could end here.
            let ring = connect.as_ref().and_then(|c| {
                if c.from == node.id {
                    return None;
                }
                let hovered = c.target.as_ref().is_some_and(|(id, _)| *id == node.id);
                c.valid.contains(&node.id).then_some((ACCENT, hovered))
            });
            if let Some(at) = node.in_port() {
                targets.push(port_target(vp, at, format!("port-{}-in", node.id), ring));
            }
            for (port, at) in look.ports.iter().zip(node.shape.output_ports()) {
                let source = connect.as_ref().is_some_and(|c| c.from == node.id && c.port == *port);
                targets.push(port_target(vp, at, format!("port-{}-{port}-out", node.id), source.then_some((ACCENT, false))));
            }

            if let Some(mark) = problems.get(&node.id) {
                marks_layer.push(marks::problem_badge(vp, &node.shape, ix, &node.id, mark));
            }
            let pills = instances.get(&node.id).filter(|l: &&Vec<InstanceMark>| !l.is_empty());
            if let Some(list) = pills {
                marks_layer.push(marks::status_strip(vp, &node.shape, ix, &node.id, list));
            }
            #[cfg(test)]
            {
                self.drawn.push(format!("node-{}", node.id));
                if let Some(mark) = problems.get(&node.id) {
                    let tone = if mark.severity == hub::model::Severity::Error { "error" } else { "warning" };
                    self.drawn.push(format!("problem-{}-{tone}", node.id));
                }
                for i in 0..pills.map_or(0, |l| l.len()) {
                    self.drawn.push(format!("status-{}-{i}", node.id));
                }
            }
        }

        let edges = scene.edges.iter().filter(|e| in_view(&e.edge.curve().bounds())).map(|e| e.edge.clone());

        let layer = div()
            .id("canvas")
            .key_context("Canvas")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(BG)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::pointer_pressed))
            .on_scroll_wheel(cx.listener(Self::wheel))
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(|this, _: &ui::keys::Cancel, _, cx| this.cancel(cx)))
            .child(self.recorder(cx))
            .child(DotGrid::new(vp))
            .child(Edges::new(vp, edges))
            .child(overlay_layer)
            .children(nodes)
            .children(targets)
            .children(marks_layer)
            .child(div().absolute().left(px(SPACE_4)).bottom(px(SPACE_3)).child(Legend::new()))
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(SPACE_1))
                        .child(div().type_style(BODY_STRONG).child("No nodes yet"))
                        .child(
                            div()
                                .type_style(LABEL)
                                .text_color(TEXT_MUTED)
                                .child("Add a source, then drag from its port to a router or an agent."),
                        ),
                )
            })
            .when(self.controls && loaded, |d| d.child(self.add_button(cx)).child(self.zoom_controls(cx)));
        layer.probe("canvas")
    }
}
