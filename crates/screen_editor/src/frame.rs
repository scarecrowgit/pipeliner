//! The editor's frame: loading and saving the draft, the layout of canvas, inspector and dock,
//! the toolbar, problems, test runs and deploys.
//!
//! | Area | Module |
//! | --- | --- |
//! | Loading, reloading, saving and checking the draft | `load_save` |
//! | The shell's header: version, state, undo and the actions | `header` |
//! | The toolbar over the canvas: add, hint, zoom | `toolbar` |
//! | The dock under the canvas and its tabs | `dock` |
//! | The Problems tab | `problems` |
//! | The Test run tab: the item, the numbers, the timeline | `test_run` |
//! | The Log tab | `log` |
//! | The deploy dialog | `deploy` |
//!
//! Probes: `editor` (the whole content), and those named in each module.

mod deploy;
mod dock;
mod header;
mod load_save;
mod log;
mod problems;
mod test_run;
mod toolbar;

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, IntoElement, KeyDownEvent,
    MouseButton, Render, SharedString, Subscription, Window, canvas, div, prelude::*, px,
};
use hub::model::{NodeId, PipelineId};
use hub::{CatalogModel, HubNotice, Route, Router};
use ui::dev::Probe;
use ui::prelude::*;

use crate::canvas::{CanvasEvent, CanvasView};
use crate::selectable::Texts;
use crate::{Draft, DraftEvent, InspectorView, Selection};

pub use dock::DockTab;
use load_save::{Document, Load};

/// The parts that share one draft. Replaced together when another pipeline is opened.
struct Parts {
    draft: Entity<Draft>,
    canvas: Entity<CanvasView>,
    inspector: Entity<InspectorView>,
    _subscriptions: Vec<Subscription>,
}

impl Parts {
    /// A fresh draft of `pipeline` with a canvas and an inspector over it.
    fn mount(pipeline: PipelineId, window: &mut Window, cx: &mut Context<EditorView>) -> Self {
        let draft = cx.new(|_| Draft::new(pipeline));
        let canvas = cx.new(|cx| CanvasView::new(draft.clone(), window, cx));
        // The frame's toolbar has the add chips and the zoom controls.
        canvas.update(cx, |canvas, cx| canvas.set_controls(false, cx));
        let inspector = cx.new(|cx| InspectorView::new(draft.clone(), window, cx));
        let subscriptions = vec![
            cx.subscribe(&draft, |this, _, event: &DraftEvent, cx| this.draft_event(event, cx)),
            cx.observe(&draft, |_, _, cx| cx.notify()),
            cx.subscribe_in(&canvas, window, |this, _, event: &CanvasEvent, window, cx| match event {
                CanvasEvent::Open(node) => {
                    this.select_node(node.clone(), false, cx);
                    this.parts.inspector.update(cx, |inspector, cx| inspector.focus_first_field(window, cx));
                }
            }),
            // The zoom shown in the toolbar.
            cx.observe(&canvas, |_, _, cx| cx.notify()),
        ];
        Self { draft, canvas, inspector, _subscriptions: subscriptions }
    }
}

/// The editor's width until its first frame is measured, px: a wide window, so the first
/// frame looks as it always did.
const WIDTH_UNTIL_MEASURED: f32 = 1484.0;

/// What the editor measured of its own layout in the last frame. Narrow layouts follow it;
/// a change is drawn again at once.
pub(super) struct Measured {
    /// Width of the editor's content, px.
    pub(super) width: Cell<f32>,
    /// Width of the toolbar's "Add" and its chips with their labels, once drawn so.
    pub(super) labelled_add: Cell<Option<f32>>,
    /// Width of the toolbar's zoom controls and "Fit".
    pub(super) view_controls: Cell<f32>,
}

impl Default for Measured {
    fn default() -> Self {
        Self { width: Cell::new(WIDTH_UNTIL_MEASURED), labelled_add: Cell::new(None), view_controls: Cell::new(0.) }
    }
}

impl Measured {
    /// Width of the column under the toolbar beside the inspector: the canvas and the dock.
    pub(super) fn left_width(&self) -> f32 {
        (self.width.get() - PANEL_WIDTH).max(0.)
    }
}

/// An invisible layer over its (relative) parent that hands the parent's width to `store`;
/// when `store` says it changed the window is drawn again.
pub(super) fn measure_width(store: impl Fn(f32) -> bool + 'static) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            if store(f32::from(bounds.size.width)) {
                // A refresh asked for while drawing is dropped: ask once this frame is done.
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Sets `cell` to `width` and says whether that changed it by half a pixel or more.
pub(super) fn store_width(cell: &Cell<f32>, width: f32) -> bool {
    if width <= 0. || (cell.get() - width).abs() < 0.5 {
        return false;
    }
    cell.set(width);
    true
}

/// The whole content of `Route::Editor`.
pub struct EditorView {
    pipeline: PipelineId,
    catalog: Entity<CatalogModel>,
    parts: Parts,
    doc: Document,
    dock: dock::Dock,
    test: test_run::TestRun,
    deploy: Option<Entity<deploy::DeployDialog>>,
    focus: FocusHandle,
    /// Move focus into the editor on the next frame: it was just shown.
    wants_focus: bool,
    measured: Rc<Measured>,
    /// Selectable texts drawn in the editor's own frame (problems, errors).
    texts: Texts,
    _subscriptions: Vec<Subscription>,
}

impl EditorView {
    /// The editor of `pipeline`, with `node` selected when given.
    pub fn new(pipeline: PipelineId, node: Option<NodeId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let catalog = hub::catalog(cx);
        let parts = Parts::mount(pipeline.clone(), window, cx);
        let test = test_run::TestRun::new(window, cx);
        let runs = hub::runs(cx);
        let subscriptions = vec![
            cx.observe(&catalog, |this, _, cx| this.catalog_changed(cx)),
            cx.subscribe(&hub::hub(cx), |this, _, notice: &HubNotice, cx| this.hub_notice(notice, cx)),
            cx.observe(&hub::router(cx), |this, router, cx| this.route_changed(router, cx)),
            cx.observe(&runs, |this, _, cx| this.offer_last_test_run(cx)),
            cx.on_release(|this, cx| this.save_on_release(cx)),
        ];
        let mut this = Self {
            pipeline,
            catalog,
            parts,
            doc: Document::new(node),
            dock: dock::Dock::default(),
            test,
            deploy: None,
            focus: cx.focus_handle(),
            wants_focus: true,
            measured: Rc::default(),
            texts: Texts::default(),
            _subscriptions: subscriptions,
        };
        this.start_loading(cx);
        this.load_runs(cx);
        this
    }

    /// Follows the route: another pipeline, or another node of the same one.
    pub fn show(&mut self, pipeline: PipelineId, node: Option<NodeId>, window: &mut Window, cx: &mut Context<Self>) {
        self.wants_focus = true;
        if pipeline != self.pipeline {
            self.save_now(cx);
            self.close_deploy(window, cx);
            self.pipeline = pipeline.clone();
            self.parts = Parts::mount(pipeline, window, cx);
            self.doc = Document::new(node);
            self.test.forget_run();
            self.start_loading(cx);
            self.load_runs(cx);
        } else if let Some(node) = node {
            self.select_node(node, true, cx);
        }
        cx.notify();
    }

    /// The pipeline it edits.
    pub fn pipeline(&self) -> &PipelineId {
        &self.pipeline
    }

    /// The draft being edited.
    pub fn draft(&self) -> &Entity<Draft> {
        &self.parts.draft
    }

    /// The canvas of the draft.
    pub fn canvas(&self) -> &Entity<CanvasView> {
        &self.parts.canvas
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, cx: &App) -> SharedString {
        if let Some(graph) = self.parts.draft.read(cx).graph() {
            return graph.name.clone().into();
        }
        let reference = hub::model::RunnableRef::pipeline(self.pipeline.as_str());
        match self.catalog.read(cx).runnable(&reference) {
            Some(runnable) => runnable.name.clone().into(),
            None => self.pipeline.to_string().into(),
        }
    }

    /// What follows the title in the shell's header: version and state marks.
    pub fn header_badges(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.badges(cx)
    }

    /// The controls on the right of the shell's header, left to right.
    pub fn header_actions(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.actions(cx)
    }

    /// Selects `node` (the route named it, or the canvas opened it); `centre` pans to it.
    fn select_node(&mut self, node: NodeId, centre: bool, cx: &mut Context<Self>) {
        if self.parts.draft.read(cx).graph().is_none() {
            self.doc.pending_node = Some(node);
            return;
        }
        self.parts.draft.update(cx, |draft, cx| draft.select(Selection::Node(node.clone()), cx));
        if centre {
            self.parts.canvas.update(cx, |canvas, cx| canvas.center_on(&node, cx));
        }
    }

    fn draft_event(&mut self, event: &DraftEvent, cx: &mut Context<Self>) {
        match event {
            DraftEvent::Changed => {
                self.schedule_save(cx);
                self.schedule_check(cx);
            }
            DraftEvent::RunShown => self.follow_run(cx),
            DraftEvent::Selected | DraftEvent::Checked => {}
        }
        cx.notify();
    }

    /// Leaving the editor saves what is not saved yet.
    fn route_changed(&mut self, router: Entity<Router>, cx: &mut Context<Self>) {
        let here = matches!(router.read(cx).current(), Route::Editor { pipeline, .. } if *pipeline == self.pipeline);
        if !here {
            self.save_now(cx);
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.parts.draft.update(cx, |draft, cx| draft.undo(cx));
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        self.parts.draft.update(cx, |draft, cx| draft.redo(cx));
    }

    /// ⇧⌘Z redoes and ⌘S saves at once; ⌘Z arrives as the kit's `Undo` action.
    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let primary = k.modifiers.platform || k.modifiers.control;
        if !primary {
            return;
        }
        match k.key.as_str() {
            "z" if k.modifiers.shift => self.redo(cx),
            "s" if !k.modifiers.shift => {
                self.keep_mine(cx);
                self.save_now(cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Toolbar, canvas, inspector and dock once the draft is loaded.
    fn render_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let canvas_area = col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .relative()
            .child(div().size_full().child(self.parts.canvas.clone()))
            .children(self.file_banners(cx));
        let left = col().flex_1().min_w_0().h_full().child(canvas_area).child(self.render_dock(window, cx));
        let inspector = div()
            .w(px(PANEL_WIDTH))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(LINE)
            .bg(BG)
            .overflow_hidden()
            .child(self.parts.inspector.clone());
        col()
            .size_full()
            .child(self.render_toolbar(window, cx))
            .child(div().flex().flex_1().min_h_0().child(left).child(inspector))
            .into_any_element()
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.wants_focus {
            self.wants_focus = false;
            let focus = self.parts.canvas.read(cx).focus_handle().clone();
            window.focus(&focus);
        }
        self.texts.begin_frame();
        let body = match self.doc.load.clone() {
            Load::Ready => self.render_editor(window, cx),
            other => self.render_placeholder(other, window, cx),
        };
        self.texts.end_frame();
        let measured = self.measured.clone();
        div()
            .id("editor")
            .debug_selector(|| "editor".into())
            .key_context("Editor")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .bg(BG)
            .child(measure_width(move |width| store_width(&measured.width, width)))
            .on_action(cx.listener(|this, _: &keys::Undo, _, cx| this.undo(cx)))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Keys keep working after a click on something that takes no focus.
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus);
                    }
                }),
            )
            .child(body)
            .probe("editor")
    }
}
