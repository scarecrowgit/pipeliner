//! The editor's frame: loading and saving the draft, the layout of canvas, inspector and dock,
//! the toolbar, problems, test runs and deploys.

use gpui::{AnyElement, App, Context, IntoElement, Render, SharedString, Window, div};
use hub::model::{NodeId, PipelineId};

/// The whole content of `Route::Editor`.
pub struct EditorView {
    pipeline: PipelineId,
}

impl EditorView {
    /// The editor of `pipeline`, with `node` selected when given.
    pub fn new(pipeline: PipelineId, _node: Option<NodeId>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { pipeline }
    }

    /// Follows the route: another pipeline, or another node of the same one.
    pub fn show(&mut self, pipeline: PipelineId, _node: Option<NodeId>, _window: &mut Window, cx: &mut Context<Self>) {
        self.pipeline = pipeline;
        cx.notify();
    }

    /// The pipeline it edits.
    pub fn pipeline(&self) -> &PipelineId {
        &self.pipeline
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &App) -> SharedString {
        self.pipeline.to_string().into()
    }

    /// What follows the title in the shell's header: version and state marks.
    pub fn header_badges(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }

    /// The controls on the right of the shell's header, left to right.
    pub fn header_actions(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
