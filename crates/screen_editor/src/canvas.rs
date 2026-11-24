//! The canvas: nodes with their ports and rows, edges by kind, pan and zoom, dragging nodes,
//! drawing edges, adding and deleting.

use gpui::{Context, Entity, IntoElement, Render, Window, div};

use crate::Draft;

/// The canvas of one draft.
pub struct CanvasView {
    draft: Entity<Draft>,
}

impl CanvasView {
    /// A canvas over `draft`; it observes the draft and edits it through [`Draft::edit`].
    pub fn new(draft: Entity<Draft>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { draft }
    }

    /// The draft it shows.
    pub fn draft(&self) -> &Entity<Draft> {
        &self.draft
    }

    /// Fits the whole graph into view.
    pub fn zoom_to_fit(&mut self, _cx: &mut Context<Self>) {}

    /// The zoom factor, 1.0 for natural size.
    pub fn zoom(&self) -> f32 {
        1.0
    }

    /// Sets the zoom factor, keeping the centre of the view in place.
    pub fn set_zoom(&mut self, _zoom: f32, _cx: &mut Context<Self>) {}
}

impl Render for CanvasView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
