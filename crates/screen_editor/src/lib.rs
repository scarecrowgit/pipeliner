//! Pipeline editor screen: header, node toolbar, zoomable node canvas with
//! draggable nodes and port-to-port linking, a per-kind inspector and the
//! test-run panel with a simulated run.

mod canvas;
mod data;
mod header;
mod inspector;
mod run_panel;
mod widgets;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Context, Entity, Focusable, IntoElement, Pixels, Point, Render, ScrollHandle, Subscription,
    Task, Window, div, point, prelude::*, px,
};
use ui::{TextInput, TextInputEvent, theme::*};

use data::*;

/// Which dropdown (select) is open in the inspector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    Library,
    Model,
    Every,
    PipeRef,
    Reviewers,
    Nudge,
}

/// Which multi-line field is being edited (shown as a single-line input while focused).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Area {
    Role,
    Jql,
}

/// A node being moved by its title.
struct Drag {
    id: String,
    start: Point<Pixels>,
    ox: f32,
    oy: f32,
}

/// A connection being dragged from an output port.
pub(crate) struct Link {
    pub from: String,
    pub port: i32,
    /// Pointer in layer coordinates.
    pub pt: (f32, f32),
}

pub struct PipelineEditorScreen {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) sel: Option<String>,
    pub(crate) zoom: f32,
    pub(crate) step: usize,
    pub(crate) running: bool,
    /// Run panel tab: 0 Test run, 1 Log, 2 Problems.
    pub(crate) tab: usize,
    pub(crate) pipeline_name: String,
    pub(crate) pick_each: bool,
    uid: usize,
    drag: Option<Drag>,
    pub(crate) link: Option<Link>,
    /// Window position of the canvas layer's top-left (updated every paint).
    pub(crate) layer_origin: Rc<Cell<Point<Pixels>>>,
    pub(crate) scroll: ScrollHandle,
    run_task: Option<Task<()>>,
    pub(crate) open_menu: Option<Menu>,
    pub(crate) editing_area: Option<Area>,
    pub(crate) pipeline_input: Entity<TextInput>,
    pub(crate) name_input: Entity<TextInput>,
    pub(crate) role_input: Entity<TextInput>,
    pub(crate) jql_input: Entity<TextInput>,
    pub(crate) glob_input: Entity<TextInput>,
    // Local-only controls the mockup does not store per node.
    pub(crate) agents_at_once: i32,
    pub(crate) issues_per_pull: i32,
    pub(crate) reviewers: usize,
    pub(crate) nudge: usize,
    _subscriptions: Vec<Subscription>,
}

impl PipelineEditorScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pipeline_input = cx.new(|cx| TextInput::new("Pipeline name", cx));
        let name_input = cx.new(|cx| TextInput::new("Name", cx));
        let role_input = cx.new(|cx| TextInput::new("", cx));
        let jql_input = cx.new(|cx| TextInput::new("JQL", cx));
        let glob_input = cx.new(|cx| TextInput::new("path/**", cx));

        let mut subs = Vec::new();
        subs.push(
            cx.subscribe(&pipeline_input, |this, input, ev: &TextInputEvent, cx| {
                if *ev == TextInputEvent::Changed {
                    this.pipeline_name = input.read(cx).text().to_string();
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe(&name_input, |this, input, ev: &TextInputEvent, cx| {
                if *ev == TextInputEvent::Changed {
                    let text = input.read(cx).text().to_string();
                    this.update_sel(|n| n.title = text);
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe(&glob_input, |this, input, ev: &TextInputEvent, cx| {
                if *ev == TextInputEvent::Changed {
                    let text = input.read(cx).text().to_string();
                    this.update_sel(|n| n.glob = text);
                    cx.notify();
                }
            }),
        );
        for (input, area) in [(&role_input, Area::Role), (&jql_input, Area::Jql)] {
            // Leaving a multi-line field returns it to its wrapped, read-only look.
            let handle = input.focus_handle(cx);
            subs.push(cx.on_blur(&handle, window, move |this, _, cx| {
                if this.editing_area == Some(area) {
                    this.editing_area = None;
                    cx.notify();
                }
            }));
            subs.push(cx.subscribe_in(
                input,
                window,
                move |this, input, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => {
                        let text = input.read(cx).text().to_string();
                        this.update_sel(|n| match area {
                            Area::Role => n.role = text,
                            Area::Jql => n.jql = text,
                        });
                        cx.notify();
                    }
                    // Enter / Esc finish editing; the blur listener restores the wrapped view.
                    TextInputEvent::Submit | TextInputEvent::Cancel => window.blur(),
                    _ => {}
                },
            ));
        }

        let mut this = Self {
            nodes: initial_nodes(),
            edges: initial_edges(),
            sel: Some("fixer".into()),
            zoom: 0.76,
            step: 0,
            running: false,
            tab: 0,
            pipeline_name: "PAY triage".into(),
            pick_each: false,
            uid: 1,
            drag: None,
            link: None,
            layer_origin: Rc::new(Cell::new(Point::default())),
            scroll: ScrollHandle::new(),
            run_task: None,
            open_menu: None,
            editing_area: None,
            pipeline_input,
            name_input,
            role_input,
            jql_input,
            glob_input,
            agents_at_once: 4,
            issues_per_pull: 3,
            reviewers: 0,
            nudge: 1,
            _subscriptions: subs,
        };
        this.sync_inputs(cx);
        this
    }

    // ── Model helpers ───────────────────────────────────────────────────

    pub(crate) fn by_id(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub(crate) fn selected(&self) -> Option<&Node> {
        self.sel.as_deref().and_then(|id| self.by_id(id))
    }

    pub(crate) fn status_of(&self, id: &str) -> RunStatus {
        scripted_status(id, self.step)
    }

    pub(crate) fn queued(&self) -> u32 {
        if self.step >= 1 { 34 } else { 37 }
    }

    pub(crate) fn update_sel(&mut self, f: impl FnOnce(&mut Node)) {
        let Some(id) = self.sel.clone() else { return };
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            f(n);
        }
    }

    /// Change the selection and load the selected node's fields into the inputs.
    pub(crate) fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.sel == id {
            return;
        }
        self.sel = id;
        self.open_menu = None;
        self.editing_area = None;
        self.sync_inputs(cx);
        cx.notify();
    }

    pub(crate) fn sync_inputs(&mut self, cx: &mut Context<Self>) {
        let (title, role, jql, glob) = match self.selected() {
            Some(n) => (
                n.title.clone(),
                n.role.clone(),
                n.jql.clone(),
                n.glob.clone(),
            ),
            None => Default::default(),
        };
        let name = self.pipeline_name.clone();
        self.pipeline_input.update(cx, |i, cx| i.set_text(name, cx));
        self.name_input.update(cx, |i, cx| i.set_text(title, cx));
        self.role_input.update(cx, |i, cx| i.set_text(role, cx));
        self.jql_input.update(cx, |i, cx| i.set_text(jql, cx));
        self.glob_input.update(cx, |i, cx| i.set_text(glob, cx));
    }

    /// Toolbar "Add …": new node near the top-left of the visible canvas, selected.
    pub(crate) fn add_node(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let uid = self.uid;
        self.uid += 1;
        let z = self.zoom;
        let offset = self.scroll.offset();
        let (sx, sy) = (-f32::from(offset.x) / z, -f32::from(offset.y) / z);
        let off = (self.uid % 6) as f32 * 18.;
        let n = new_node(
            kind,
            uid,
            (sx + 60. + off).round(),
            (sy + 60. + off).round(),
        );
        let id = n.id.clone();
        self.nodes.push(n);
        self.select(Some(id), cx);
    }

    pub(crate) fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sel.clone() else { return };
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
        for n in &mut self.nodes {
            if n.parent.as_deref() == Some(id.as_str()) {
                n.parent = None;
            }
        }
        self.select(None, cx);
    }

    /// File mode switch; a read-mode file has no input port, so its incoming edges go.
    pub(crate) fn set_file_mode(&mut self, mode: FileMode) {
        let Some(id) = self.sel.clone() else { return };
        self.update_sel(|n| n.mode = mode);
        if self.by_id(&id).is_some_and(|n| !n.has_in()) {
            self.edges.retain(|e| e.to != id);
        }
    }

    /// "Test run": step through the scripted run every 1.1s.
    pub(crate) fn run(&mut self, cx: &mut Context<Self>) {
        if self.run_task.is_some() {
            return;
        }
        self.step = 1;
        self.running = true;
        cx.notify();
        self.run_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1100))
                    .await;
                let finished = this
                    .update(cx, |this, cx| {
                        this.step = (this.step + 1).min(LAST_STEP);
                        this.running = this.step < LAST_STEP;
                        cx.notify();
                        !this.running
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
            }
            this.update(cx, |this, _| this.run_task = None).ok();
        }));
    }

    pub(crate) fn step_zoom(&mut self, dz: f32, cx: &mut Context<Self>) {
        self.zoom = (((self.zoom + dz) * 100.).round() / 100.).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    pub(crate) fn fit(&mut self, cx: &mut Context<Self>) {
        let view = self.scroll.bounds().size;
        let f = 1.2f32
            .min((f32::from(view.width) - 8.) / LAYER_W)
            .min((f32::from(view.height) - 8.) / LAYER_H);
        self.zoom = ((f * 100.).floor() / 100.).max(ZOOM_MIN);
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    // ── Pointer interactions on the canvas ──────────────────────────────

    fn layer_point(&self, pos: Point<Pixels>) -> (f32, f32) {
        let o = self.layer_origin.get();
        (
            f32::from(pos.x - o.x) / self.zoom,
            f32::from(pos.y - o.y) / self.zoom,
        )
    }

    /// Pointer down on a node's title: select it and start moving it.
    pub(crate) fn grab(&mut self, id: &str, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(n) = self.by_id(id) {
            self.drag = Some(Drag {
                id: id.into(),
                start: pos,
                ox: n.x,
                oy: n.y,
            });
        }
        self.select(Some(id.into()), cx);
        cx.notify();
    }

    /// Pointer down on an output port: start a rubber-band connection.
    pub(crate) fn start_link(
        &mut self,
        id: &str,
        port: i32,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.link = Some(Link {
            from: id.into(),
            port,
            pt: self.layer_point(pos),
        });
        cx.notify();
    }

    pub(crate) fn pointer_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let z = self.zoom;
        if let Some(d) = &self.drag {
            let nx = (d.ox + f32::from(pos.x - d.start.x) / z)
                .round()
                .clamp(0., LAYER_W - NODE_W);
            let ny = (d.oy + f32::from(pos.y - d.start.y) / z)
                .round()
                .clamp(0., LAYER_H - 60.);
            let id = d.id.clone();
            if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
                n.x = nx;
                n.y = ny;
            }
            cx.notify();
        } else if self.link.is_some() {
            let pt = self.layer_point(pos);
            if let Some(l) = &mut self.link {
                l.pt = pt;
            }
            cx.notify();
        }
    }

    /// Pointer released: finish a move, or connect the link to the node under the pointer
    /// (its input port or anywhere on the node).
    pub(crate) fn pointer_up(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.drag = None;
        let Some(link) = self.link.take() else {
            cx.notify();
            return;
        };
        let (pointer_x, pointer_y) = self.layer_point(pos);
        let target = self
            .nodes
            .iter()
            .rev()
            .find(|n| {
                if !n.has_in() || n.id == link.from {
                    return false;
                }
                let (ix, iy) = in_pt(n);
                let on_port = (pointer_x - ix).abs() <= 14. && (pointer_y - iy).abs() <= 14.;
                let h = node_height(self.rows_for(n).len());
                let on_node = pointer_x >= n.x
                    && pointer_x <= n.x + NODE_W
                    && pointer_y >= n.y
                    && pointer_y <= n.y + h;
                on_port || on_node
            })
            .map(|n| n.id.clone());
        if let Some(to) = target {
            let edge = Edge {
                from: link.from.clone(),
                port: link.port,
                to: to.clone(),
            };
            if !self.edges.contains(&edge) {
                self.edges.push(edge);
            }
            let src_kind = self.by_id(&link.from).map(|n| n.kind());
            if let Some(t) = self.nodes.iter_mut().find(|n| n.id == to)
                && t.kind() == Kind::Spawn
                && matches!(src_kind, Some(Kind::Agent | Kind::Orch))
            {
                t.parent = Some(link.from.clone());
            }
            self.select(Some(to), cx);
        }
        cx.notify();
    }

    pub(crate) fn is_interacting(&self) -> bool {
        self.drag.is_some() || self.link.is_some()
    }
}

impl Render for PipelineEditorScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("pipeline-editor")
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(TEXT)
            .child(div().flex_none().w_full().child(self.render_header(cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(
                        div()
                            .id("editor-main-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .min_h_0()
                            .overflow_y_scroll()
                            .border_r_1()
                            .border_color(LINE)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_none()
                                    .w_full()
                                    .child(self.render_toolbar(cx))
                                    .child(self.render_canvas(window, cx))
                                    .child(self.render_run_panel(cx)),
                            ),
                    )
                    .child(self.render_inspector(window, cx)),
            )
    }
}
