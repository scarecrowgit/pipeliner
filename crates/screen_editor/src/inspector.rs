//! The inspector: the fields of the selected node or edge, or of the pipeline when nothing is
//! selected.

use gpui::{Context, Entity, IntoElement, Render, Window, div};

use crate::Draft;

/// The inspector of one draft.
pub struct InspectorView {
    draft: Entity<Draft>,
}

impl InspectorView {
    /// An inspector over `draft`; it follows the selection and edits through [`Draft::edit`].
    pub fn new(draft: Entity<Draft>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { draft }
    }

    /// The draft it shows.
    pub fn draft(&self) -> &Entity<Draft> {
        &self.draft
    }
}

impl Render for InspectorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
