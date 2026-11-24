//! Pointer and keys: selecting, panning, moving nodes, drawing edges, the wheel, Delete,
//! arrows and Escape. What a gesture changes in the graph goes through the draft.

use std::collections::HashSet;

use gpui::{
    Context, KeyDownEvent, Keystroke, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Window,
    point, px,
};
use hub::model::{EdgeKind, GraphDef, NodeId, Pos};
use ui_rich::canvas::{GRID_UNIT, PORT_HIT_RADIUS, PortSide, distance, hit_node, hit_node_header, hit_port};

use super::scene::Scene;
use super::{CanvasEvent, CanvasView, rules};
use crate::Selection;

/// How far the pointer moves (px) before a press on a node becomes a drag.
const DRAG_THRESHOLD: f32 = 3.0;
/// How close (px on screen) a click must be to an edge to pick it.
const EDGE_TOLERANCE: f32 = 6.0;
/// The smallest port hit radius, px, so ports stay reachable when zoomed out.
const MIN_PORT_RADIUS: f32 = 6.0;
/// What one notch of a ctrl + wheel turn does to the zoom, per pixel of scroll.
const WHEEL_ZOOM: f32 = 0.002;
/// A wheel "line" in pixels, for mice that report lines.
const WHEEL_LINE: f32 = 20.0;
/// Arrow keys move a node by a grid step; with Shift by one unit.
const NUDGE: f32 = GRID_UNIT;
const NUDGE_FINE: f32 = 1.0;

/// An edge being drawn from an out-port.
#[derive(Clone, Debug)]
pub struct Connect {
    pub from: NodeId,
    pub port: String,
    /// Where the port is, canvas units.
    pub start: Point<f32>,
    /// The pointer, canvas units.
    pub pointer: Point<f32>,
    /// The node it would end on now, and the kind it would be.
    pub target: Option<(NodeId, EdgeKind)>,
    /// Every node it may end on.
    pub valid: HashSet<NodeId>,
}

/// The gesture in progress.
#[derive(Clone, Debug)]
pub enum Gesture {
    /// Dragging the background.
    Pan { last: Point<Pixels> },
    /// Pressed on a node's header; moves it once the pointer travels.
    /// Its moves fold into one undo step, sealed when the drag ends.
    Move { id: NodeId, start: Pos, press: Point<Pixels>, grab: Point<f32>, moved: bool },
    Connect(Connect),
}

/// Points `connect` at `p` (canvas units): the node under it, or whose in-port it is near,
/// becomes the target when the edge may end there.
fn aim(graph: &GraphDef, zoom: f32, connect: &mut Connect, p: Point<f32>) {
    connect.pointer = p;
    let scene = Scene::build(graph, &Selection::None);
    let shapes = scene.shapes();
    let radius = (PORT_HIT_RADIUS * 2.0).max(2.0 * MIN_PORT_RADIUS / zoom);
    connect.target = hit_node(&shapes, p)
        .or_else(|| hit_port(&shapes, p, radius).filter(|h| h.side == PortSide::Input).map(|h| h.node))
        .and_then(|ix| {
            let to = scene.nodes[ix].id.clone();
            let kind = rules::edge_kind(graph, &connect.from, &connect.port, &to)?;
            Some((to, kind))
        });
}

fn snap(v: f32) -> f32 {
    (v / GRID_UNIT).round() * GRID_UNIT
}

impl CanvasView {
    /// A window position in canvas units.
    fn to_canvas(&self, at: Point<Pixels>) -> Point<f32> {
        let origin = self.bounds.get().map_or(point(px(0.0), px(0.0)), |b| b.origin);
        self.viewport.pointer_to_canvas(at, origin)
    }

    /// A window position in px from the canvas's top-left.
    fn to_local(&self, at: Point<Pixels>) -> Point<f32> {
        let origin = self.bounds.get().map_or(point(px(0.0), px(0.0)), |b| b.origin);
        point(f32::from(at.x - origin.x), f32::from(at.y - origin.y))
    }

    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.draft.update(cx, |draft, cx| draft.select(selection, cx));
    }

    pub(super) fn pointer_pressed(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let Some(scene) = self.scene(cx) else { return };
        let p = self.to_canvas(event.position);
        let shapes = scene.shapes();
        let radius = PORT_HIT_RADIUS.max(MIN_PORT_RADIUS / self.viewport.zoom);
        let double = event.click_count >= 2;

        if let Some(hit) = hit_port(&shapes, p, radius) {
            let node = &scene.nodes[hit.node];
            match hit.side {
                PortSide::Output => {
                    let Some(port) = node.look.ports.get(hit.index).cloned() else { return };
                    self.start_connect(&node.id.clone(), &port, p, cx);
                }
                PortSide::Input => self.select(Selection::Node(node.id.clone()), cx),
            }
            cx.notify();
            return;
        }
        if let Some(ix) = hit_node_header(&shapes, p).or_else(|| hit_node(&shapes, p)) {
            let id = scene.nodes[ix].id.clone();
            self.select(Selection::Node(id.clone()), cx);
            if double {
                cx.emit(CanvasEvent::Open(id));
            } else if hit_node_header(&shapes, p) == Some(ix) {
                let origin = scene.nodes[ix].shape.origin;
                self.gesture = Some(Gesture::Move {
                    id,
                    start: Pos { x: origin.x, y: origin.y },
                    press: event.position,
                    grab: p,
                    moved: false,
                });
            }
            cx.notify();
            return;
        }
        if let Some(index) = scene.hit_edge(p, EDGE_TOLERANCE / self.viewport.zoom) {
            self.select(Selection::Edge(index), cx);
            cx.notify();
            return;
        }
        self.select(Selection::None, cx);
        if double {
            let anchor = ui::overlay::Anchor::point(event.position);
            self.open_add_menu_at(anchor, Some(p), window, cx);
        } else {
            self.gesture = Some(Gesture::Pan { last: event.position });
        }
        cx.notify();
    }

    pub(super) fn pointer_moved(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() && self.gesture.is_some() {
            // The button went up somewhere we did not hear about.
            self.finish(None, cx);
            return;
        }
        let p = self.to_canvas(event.position);
        match &mut self.gesture {
            None => {}
            Some(Gesture::Pan { last }) => {
                let delta = point(f32::from(event.position.x - last.x), f32::from(event.position.y - last.y));
                *last = event.position;
                self.viewport.pan(delta);
                self.fitting = false;
                cx.notify();
            }
            Some(Gesture::Move { id, start, press, grab, moved }) => {
                let travel = distance(
                    point(f32::from(press.x), f32::from(press.y)),
                    point(f32::from(event.position.x), f32::from(event.position.y)),
                );
                if !*moved && travel < DRAG_THRESHOLD {
                    return;
                }
                *moved = true;
                let to = Pos { x: start.x + snap(p.x - grab.x), y: start.y + snap(p.y - grab.y) };
                let id = id.clone();
                self.draft.update(cx, |draft, cx| {
                    draft.edit_folded(
                        "Move node",
                        |graph: &mut GraphDef| {
                            if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
                                node.pos = to;
                            }
                        },
                        cx,
                    );
                });
            }
            Some(Gesture::Connect(connect)) => {
                if let Some(graph) = self.draft.read(cx).graph() {
                    aim(graph, self.viewport.zoom, connect, p);
                }
                cx.notify();
            }
        }
    }

    /// Starts drawing an edge from out-port `port` of `from` with the pointer at `pointer`
    /// (canvas units), as a drag from that port does. The pointer, Escape or
    /// [`Self::finish_connect`] end it. Returns whether the port exists.
    pub fn start_connect(&mut self, from: &NodeId, port: &str, pointer: Point<f32>, cx: &mut Context<Self>) -> bool {
        let Some(scene) = self.scene(cx) else { return false };
        let Some(start) = scene.node(from).and_then(|node| node.out_port(port)) else { return false };
        let Some(graph) = self.draft.read(cx).graph() else { return false };
        let valid = graph
            .nodes
            .iter()
            .filter(|to| rules::edge_kind(graph, from, port, &to.id).is_some())
            .map(|to| to.id.clone())
            .collect();
        let mut connect = Connect { from: from.clone(), port: port.to_string(), start, pointer, target: None, valid };
        aim(graph, self.viewport.zoom, &mut connect, pointer);
        self.gesture = Some(Gesture::Connect(connect));
        cx.notify();
        true
    }

    /// Lands the edge being drawn where the pointer is, if it may end there.
    pub fn finish_connect(&mut self, cx: &mut Context<Self>) {
        let target = match &self.gesture {
            Some(Gesture::Connect(connect)) => connect.target.clone(),
            _ => None,
        };
        self.finish(target, cx);
    }

    pub(super) fn pointer_released(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_connect(cx);
    }

    /// Ends the gesture: a drawn edge lands on `target`; a drag closes its undo step, so the
    /// next drag is a step of its own.
    fn finish(&mut self, target: Option<(NodeId, EdgeKind)>, cx: &mut Context<Self>) {
        match (self.gesture.take(), target) {
            (Some(Gesture::Connect(connect)), Some((to, kind))) => {
                self.connect(&connect.from, &connect.port, &to, kind, cx);
            }
            (Some(Gesture::Move { moved: true, .. }), _) => self.draft.update(cx, |draft, _| draft.seal()),
            _ => {}
        }
        cx.notify();
    }

    /// Adds the edge from out-port `port` of `from` to `to` when its kind fits both ends.
    /// Returns whether it was added.
    pub fn connect(&mut self, from: &NodeId, port: &str, to: &NodeId, kind: EdgeKind, cx: &mut Context<Self>) -> bool {
        let fits = self.draft.read(cx).graph().and_then(|g| rules::edge_kind(g, from, port, to)) == Some(kind);
        if fits {
            self.draft.update(cx, |draft, cx| draft.edit("Add edge", |graph| rules::add_edge(graph, from, port, to, kind), cx));
        }
        fits
    }

    pub(super) fn wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(WHEEL_LINE));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let m = &event.modifiers;
        if m.control || m.platform {
            let at = self.to_local(event.position);
            self.zoom_at(at, (dy * WHEEL_ZOOM).exp(), cx);
        } else {
            self.viewport.pan(point(dx, dy));
            self.fitting = false;
            cx.notify();
        }
        cx.stop_propagation();
    }

    /// Escape: drops the edge being drawn, or else the selection.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if matches!(self.gesture, Some(Gesture::Connect(_))) {
            self.gesture = None;
        } else {
            self.gesture = None;
            self.select(Selection::None, cx);
        }
        cx.notify();
    }

    /// Moves the selected node by `dx`, `dy` canvas units; presses in a row on one node are
    /// one undo step, sealed when the selection changes ([`Self::end_nudge`]).
    pub fn nudge(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let Selection::Node(id) = self.draft.read(cx).selection().clone() else { return };
        if self.nudging.as_ref() != Some(&id) {
            self.end_nudge(cx);
        }
        let target = id.clone();
        let changed = self.draft.update(cx, |draft, cx| {
            let before = draft.revision();
            draft.edit_folded(
                "Nudge node",
                |graph: &mut GraphDef| {
                    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == target) {
                        node.pos = Pos { x: node.pos.x + dx, y: node.pos.y + dy };
                    }
                },
                cx,
            );
            draft.revision() != before
        });
        if changed {
            self.nudging = Some(id);
        }
    }

    /// Ends a run of nudges: the next nudge is an undo step of its own.
    pub(super) fn end_nudge(&mut self, cx: &mut Context<Self>) {
        if self.nudging.take().is_some() {
            self.draft.update(cx, |draft, _| draft.seal());
        }
    }

    pub(super) fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.press_key(&event.keystroke, cx) {
            cx.stop_propagation();
        }
    }

    /// What a key does on the canvas: Delete and Backspace remove the selection, arrows
    /// nudge the selected node, Escape cancels. Returns whether the key did anything. The
    /// frame passes keys here too while focus rests on something that has no use for them.
    pub fn press_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return false;
        }
        let step = if m.shift { NUDGE_FINE } else { NUDGE };
        match keystroke.key.as_str() {
            "delete" | "backspace" => self.delete_selected(cx),
            "left" => self.nudge(-step, 0.0, cx),
            "right" => self.nudge(step, 0.0, cx),
            "up" => self.nudge(0.0, -step, cx),
            "down" => self.nudge(0.0, step, cx),
            "escape" => self.cancel(cx),
            _ => return false,
        }
        true
    }
}
