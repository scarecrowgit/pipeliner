//! Canvas primitives for the pipeline editor: dot grid, zoomed layout, ports and edges.
//!
//! This module draws and measures; it holds no pipeline logic. The editor screen owns the
//! graph and composes these in one clipped, relative container, in this order:
//!
//! 1. [`DotGrid`]: the background dots, moving and scaling with the [`Viewport`].
//! 2. [`Edges`]: every edge in one painted layer, with their [`EdgeLabel`] pills.
//! 3. A [`NodeFrame`] per node, absolutely positioned.
//! 4. A [`Legend`] in a corner.
//!
//! GPUI has no element transforms, so nothing is scaled by the framework: [`Viewport::z`]
//! turns canvas units into pixels at the current zoom, and every drawn size goes through it.
//!
//! # Pointer interaction
//!
//! Dragging nodes, connecting ports and panning belong to the editor. What it needs from
//! here, all in canvas units:
//!
//! - [`Viewport::pointer_to_canvas`] converts a pointer position from the window;
//! - [`hit_port`], [`hit_node_header`], [`hit_node`] and [`hit_edge`] (or
//!   [`Edges::hit_test`]) say what is under the pointer; test ports first, then the header,
//!   then edges;
//! - [`Viewport::zoom_at`], [`Viewport::pan`] and [`Viewport::fit`] change the view.

mod edges;
mod geometry;
mod grid;
mod legend;
mod node;
pub(crate) mod paint;
mod viewport;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;

pub use edges::{Edge, EdgeKind, EdgeLabel, EdgeShape, Edges, hit_edge};
pub use geometry::{
    BODY_PADDING, Cubic, Curve, GRID_UNIT, HEADER_HEIGHT, HEADER_PORT_Y, NODE_WIDTH, NodePorts,
    NodeShape, Outputs, PORT_HIT_RADIUS, PORT_SIZE, PortHit, PortSide, ROW_HEIGHT, back_curve,
    bounds_of, control_offset, distance, distance_to_segment, hit_node, hit_node_header, hit_port,
    node_height, route_curve, row_center_y,
};
pub use grid::{DotGrid, MIN_DOT_SPACING, dot_range, dot_spacing};
pub use legend::Legend;
pub use node::{NodeFrame, NodeState, Port, PortTone, SPAWNED_GROUND};
pub use viewport::{MAX_ZOOM, MIN_ZOOM, Viewport, ZOOM_STEP, z};
