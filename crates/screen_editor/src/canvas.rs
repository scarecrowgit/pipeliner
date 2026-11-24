//! The canvas: nodes with their ports and rows, edges by kind, pan and zoom, dragging nodes,
//! drawing edges, adding and deleting.
//!
//! | Area | Module |
//! | --- | --- |
//! | What a node shows, by kind | `look` |
//! | Nodes, ports and edges in canvas units | `scene` |
//! | Which edge fits two nodes | `rules` |
//! | Problem and run-status marks | `marks` |
//! | Drawing the view | `render` |
//! | Pointer and keys | `interaction` |
//! | The add menu and new nodes | `add` |
//!
//! Everything is drawn from the [`Draft`]'s graph on every frame; the view keeps only the
//! viewport and the gesture in progress.

mod add;
mod interaction;
mod look;
mod marks;
mod render;
mod rules;
mod scene;

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Context, Entity, EventEmitter, FocusHandle, Pixels, Point, Subscription, Window, point};
use hub::RunModel;
use hub::model::{NodeId, RunId};
use ui_rich::canvas::{MAX_ZOOM, MIN_ZOOM, Viewport, ZOOM_STEP};

pub use add::NewNode;
pub use look::PORT_SEND_BACK;

use crate::{Draft, DraftEvent, Selection};
use interaction::Gesture;
use scene::Scene;

/// Space kept around the graph by [`CanvasView::zoom_to_fit`], px. The mockup's graph runs to
/// about 20px from the canvas's edges (the legend sits in the bottom-left corner beside it),
/// so the graph is drawn as large as the canvas allows.
const FIT_PADDING: f32 = 24.0;

/// What the canvas tells the frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasEvent {
    /// A node was double-clicked: the frame shows it in the inspector and focuses it.
    Open(NodeId),
}

/// The canvas of one draft.
pub struct CanvasView {
    draft: Entity<Draft>,
    viewport: Viewport,
    focus: FocusHandle,
    /// Where the canvas was laid out last frame, in window pixels.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Keep the graph fitted into view (until the user pans or zooms): refit when the canvas
    /// gets a size or changes it.
    fitting: bool,
    /// The canvas size the graph was last fitted to.
    fitted: Option<gpui::Size<f32>>,
    gesture: Option<Gesture>,
    /// The node the arrow keys moved last: further presses fold into that undo step.
    nudging: Option<NodeId>,
    /// What the last frame laid out (node, mark and status selectors), for the tests.
    #[cfg(test)]
    drawn: Vec<String>,
    /// The test run on show and the model it is read from.
    run: Option<(RunId, Entity<RunModel>, Subscription)>,
    /// Draw the add button and the zoom controls (off when the frame's toolbar has them).
    controls: bool,
    _draft: Subscription,
}

impl EventEmitter<CanvasEvent> for CanvasView {}

impl CanvasView {
    /// A canvas over `draft`; it observes the draft and edits it through [`Draft::edit`].
    pub fn new(draft: Entity<Draft>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&draft, |this, _, event, cx| {
            match event {
                DraftEvent::RunShown => this.follow_run(cx),
                DraftEvent::Selected => this.end_nudge(cx),
                _ => {}
            }
            cx.notify();
        });
        let mut this = Self {
            draft,
            viewport: Viewport::default(),
            focus: cx.focus_handle(),
            bounds: Rc::new(Cell::new(None)),
            fitting: true,
            fitted: None,
            gesture: None,
            nudging: None,
            #[cfg(test)]
            drawn: Vec::new(),
            run: None,
            controls: true,
            _draft: subscription,
        };
        this.follow_run(cx);
        this
    }

    /// The draft it shows.
    pub fn draft(&self) -> &Entity<Draft> {
        &self.draft
    }

    /// The focus handle of the canvas: keys (Delete, arrows, Escape) act while it is focused.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Hide or show the canvas's own add button and zoom controls, for a frame whose toolbar
    /// offers them (through [`Self::add_node`], [`Self::zoom_in`]…).
    pub fn set_controls(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.controls = shown;
        cx.notify();
    }

    /// The viewport: zoom and where canvas `(0, 0)` is on screen.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The size of the canvas as last laid out, px.
    fn size(&self) -> Option<gpui::Size<f32>> {
        self.bounds.get().map(|b| gpui::size(f32::from(b.size.width), f32::from(b.size.height)))
    }

    fn scene(&self, cx: &gpui::App) -> Option<Scene> {
        let draft = self.draft.read(cx);
        Some(Scene::build(draft.graph()?, draft.selection()))
    }

    /// Fits the whole graph into view.
    pub fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        self.fitting = true;
        self.fitted = None;
        if let (Some(size), Some(content)) = (self.size(), self.scene(cx).and_then(|s| s.bounds())) {
            self.viewport = Viewport::fit(content, size, FIT_PADDING);
            self.fitted = Some(size);
        }
        cx.notify();
    }

    /// The zoom factor, 1.0 for natural size.
    pub fn zoom(&self) -> f32 {
        self.viewport.zoom
    }

    /// Sets the zoom factor, keeping the centre of the view in place.
    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let centre = self.size().map_or(point(0.0, 0.0), |s| point(s.width / 2.0, s.height / 2.0));
        self.viewport.set_zoom_at(centre, zoom);
        self.fitting = false;
        cx.notify();
    }

    /// One step closer ([`ZOOM_STEP`]).
    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.set_zoom((self.zoom() * ZOOM_STEP).min(MAX_ZOOM), cx);
    }

    /// One step further.
    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.set_zoom((self.zoom() / ZOOM_STEP).max(MIN_ZOOM), cx);
    }

    /// Zooms by `factor` keeping the canvas point under `at` (px from the canvas's top-left)
    /// where it is: what ctrl + wheel does.
    pub fn zoom_at(&mut self, at: Point<f32>, factor: f32, cx: &mut Context<Self>) {
        self.viewport.zoom_at(at, factor);
        self.fitting = false;
        cx.notify();
    }

    /// Pans so node `id` is in the middle of the view, at the current zoom.
    pub fn center_on(&mut self, id: &NodeId, cx: &mut Context<Self>) {
        let (Some(size), Some(scene)) = (self.size(), self.scene(cx)) else { return };
        let Some(node) = scene.node(id) else { return };
        let b = node.shape.bounds();
        let centre = point(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0);
        let zoom = self.viewport.zoom;
        self.viewport.origin = point(size.width / 2.0 - centre.x * zoom, size.height / 2.0 - centre.y * zoom);
        self.fitting = false;
        cx.notify();
    }

    /// The canvas point in the middle of the view.
    fn view_centre(&self) -> Point<f32> {
        let s = self.size().unwrap_or(gpui::size(0.0, 0.0));
        self.viewport.to_canvas(point(s.width / 2.0, s.height / 2.0))
    }

    /// Adds a node of `what` centred on `at` (canvas units), or in the middle of the view, and
    /// selects it. Returns its id; `None` for a kind that cannot be added yet.
    pub fn add_node(&mut self, what: NewNode, at: Option<Point<f32>>, cx: &mut Context<Self>) -> Option<NodeId> {
        let at = at.unwrap_or_else(|| self.view_centre());
        let node = add::new_node(self.draft.read(cx).graph()?, what, at)?;
        let id = node.id.clone();
        self.draft.update(cx, |draft, cx| {
            draft.edit("Add node", |graph| graph.nodes.push(node), cx);
            draft.select(Selection::Node(id.clone()), cx);
        });
        Some(id)
    }

    /// Opens the add menu under the canvas's add button; the node goes to the middle of the
    /// view.
    pub fn open_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = ui::overlay::Anchor::of(render::ADD_ANCHOR, window, cx);
        self.open_add_menu_at(anchor, None, window, cx);
    }

    fn open_add_menu_at(
        &mut self,
        anchor: ui::overlay::Anchor,
        at: Option<Point<f32>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let canvas = cx.entity().downgrade();
        ui::overlays(window, cx).open_menu(
            anchor,
            ui::overlay::Placement::below(),
            move |_, _| add::menu(canvas.clone(), at),
            window,
            cx,
        );
    }

    /// Removes the selected node with its edges, or the selected edge.
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let selection = self.draft.read(cx).selection().clone();
        self.draft.update(cx, |draft, cx| match selection {
            Selection::Node(id) => draft.edit("Delete node", |graph| rules::remove_node(graph, &id), cx),
            Selection::Edge(index) => draft.edit("Delete edge", |graph| rules::remove_edge(graph, index), cx),
            Selection::None => {}
        });
    }

    /// Takes the run on show from the draft and follows its model.
    fn follow_run(&mut self, cx: &mut Context<Self>) {
        let wanted = self.draft.read(cx).run().cloned();
        if wanted.as_ref() == self.run.as_ref().map(|(id, _, _)| id) {
            return;
        }
        self.run = wanted.map(|id| {
            let model = hub::run(&id, cx);
            let subscription = cx.observe(&model, |_, _, cx| cx.notify());
            (id, model, subscription)
        });
        cx.notify();
    }
}
