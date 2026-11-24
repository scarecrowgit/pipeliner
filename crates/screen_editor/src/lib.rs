//! The pipeline editor: the canvas of nodes and edges, the inspector of what is selected, and
//! the dock with problems, test runs and deploys.
//!
//! The crate gives the app one view, [`EditorView`], for `Route::Editor`; the shell mounts it
//! and draws its header. The three parts share one [`Draft`]: the graph being edited, what is
//! selected, the undo history and the test run on show.
//!
//! | Part | Module | What it does |
//! | --- | --- | --- |
//! | Frame | [`frame`] | [`EditorView`]: loads the draft, lays out the parts, saves, toolbar, dock, deploy |
//! | Canvas | [`canvas`] | [`CanvasView`]: nodes, ports, edges, pan and zoom, drag, connect, add |
//! | Inspector | [`inspector`] | [`InspectorView`]: the fields of the selected node, edge or the pipeline |
//!
//! Everything shown comes from `hub` models; everything done goes through `hub::commands`.

pub mod canvas;
pub mod draft;
pub mod frame;
pub mod inspector;
mod selectable;

pub use canvas::CanvasView;
pub use draft::{Draft, DraftEvent, Selection};
pub use frame::EditorView;
pub use inspector::InspectorView;
