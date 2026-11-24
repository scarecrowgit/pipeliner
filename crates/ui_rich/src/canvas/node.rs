//! The node card and its ports, drawn at a zoom.

use gpui::{
    App, BoxShadow, Div, ElementId, Hsla, IntoElement, ParentElement, Point, RenderOnce,
    SharedString, Stateful, Window, div, point, prelude::*, px,
};
use ui::assets::{IconName, icon};
use ui::components::TileTone;
use ui::layout::truncate_block;
use ui::theme::*;

use super::geometry::{
    HEADER_HEIGHT, NODE_WIDTH, NodePorts, NodeShape, Outputs, PORT_SIZE, ROW_HEIGHT, node_height,
};
use super::viewport::Viewport;

/// The ground of a spawned node, darker than a card (`.node.spawned{background:#0f0d18}` in
/// the pipeline editor mockup). Not a token.
pub const SPAWNED_GROUND: Hsla = hex(0x0f0d18);
/// How much a "coming soon" node is dimmed.
const SOON_OPACITY: f32 = 0.5;

/// Border of a node, in canvas units.
const BORDER: f32 = 1.0;
/// Border of a port.
const PORT_BORDER: f32 = 2.0;
/// Space either side of a node's header content.
const PADDING_X: f32 = 10.0;
/// Header icon tile, its icon and radius.
const TILE: f32 = 24.0;
const TILE_ICON: f32 = 14.0;
/// The glow around a selected node (`--selected-glow`: a 3px ring).
const GLOW: f32 = 3.0;

/// How a node looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NodeState {
    /// An ordinary node.
    #[default]
    Default,
    /// The selected node: accent border and glow, a check at the end of the header.
    Selected,
    /// An agent created by another agent: dashed violet border on a darker ground.
    Spawned,
    /// A kind of node that is not available yet: dimmed, marked "Soon".
    Soon,
}

/// The colour family of a [`Port`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PortTone {
    /// `line-strong`.
    #[default]
    Default,
    /// Accent, on a selected node.
    Selected,
    /// Violet, on a spawned node.
    Spawn,
}

impl PortTone {
    fn color(self) -> Hsla {
        match self {
            PortTone::Default => LINE_STRONG,
            PortTone::Selected => ACCENT,
            PortTone::Spawn => SPAWN_LINE,
        }
    }
}

/// A port: a 10 unit circle with a 2 unit ring on the canvas ground.
///
/// Sized at the zoom it was given; position it where the node's
/// [`ports`](NodeFrame::ports) say.
#[derive(IntoElement)]
pub struct Port {
    viewport: Viewport,
    tone: PortTone,
}

impl Port {
    /// A port of `tone` at `viewport`'s zoom.
    pub fn new(viewport: Viewport, tone: PortTone) -> Self {
        Self { viewport: viewport.sanitized(), tone }
    }
}

impl RenderOnce for Port {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let vp = self.viewport;
        div()
            .flex_none()
            .size(vp.z(PORT_SIZE))
            .rounded_full()
            .bg(BG)
            .border(vp.z(PORT_BORDER))
            .border_color(self.tone.color())
    }
}

/// The icon tile colours of a tone: ground and glyph.
fn tile_colors(tone: TileTone) -> (Hsla, Hsla) {
    match tone {
        TileTone::Accent => (ACCENT_SOFT, ACCENT_TEXT),
        TileTone::Info => (INFO_SOFT, INFO_TEXT),
        TileTone::Spawn => (SPAWN_SOFT, SPAWN_TEXT),
        TileTone::Warn => (WARNING_SOFT, WARNING),
        _ => (SURFACE_HOVER, TEXT_SECONDARY),
    }
}

/// A pipeline node at a zoom: 168 units wide, a 49 unit header with an icon tile, a title and
/// a mono sub-line, and a body of 24 unit key/value rows.
///
/// GPUI has no transforms, so the frame is laid out in pixels: every size, radius, border and
/// font size is its value in units times the zoom. It is absolutely positioned where
/// [`Viewport::to_screen`] puts `origin`: put it in a `relative` container that clips.
///
/// [`ports`](Self::ports) gives the port positions in canvas units, which is what
/// [`Edge`](super::Edge)s connect.
#[derive(IntoElement)]
pub struct NodeFrame {
    id: ElementId,
    viewport: Viewport,
    origin: Point<f32>,
    icon: IconName,
    tone: TileTone,
    title: SharedString,
    subtitle: SharedString,
    rows: Vec<(SharedString, SharedString)>,
    state: NodeState,
    input: bool,
    outputs: Outputs,
}

impl NodeFrame {
    /// A node whose top-left corner is at `origin` (canvas units), seen through `viewport`.
    pub fn new(id: impl Into<ElementId>, viewport: Viewport, origin: Point<f32>) -> Self {
        Self {
            id: id.into(),
            viewport: viewport.sanitized(),
            origin,
            icon: IconName::Agent,
            tone: TileTone::Neutral,
            title: SharedString::default(),
            subtitle: SharedString::default(),
            rows: Vec::new(),
            state: NodeState::Default,
            input: true,
            outputs: Outputs::One,
        }
    }

    /// The icon of the header tile.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    /// The colour family of the header tile.
    pub fn tone(mut self, tone: TileTone) -> Self {
        self.tone = tone;
        self
    }

    /// The title.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    /// The mono sub-line under the title.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    /// Add a body row: a muted key at the left, a mono value at the right.
    pub fn row(mut self, key: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        self.rows.push((key.into(), value.into()));
        self
    }

    /// How the node looks.
    pub fn state(mut self, state: NodeState) -> Self {
        self.state = state;
        self
    }

    /// Whether the node has an input port (default: yes).
    pub fn input(mut self, input: bool) -> Self {
        self.input = input;
        self
    }

    /// Which outputs the node has (default: one at the header).
    pub fn outputs(mut self, outputs: Outputs) -> Self {
        self.outputs = outputs;
        self
    }

    /// The box and ports of this node, for placement and hit tests.
    pub fn shape(&self) -> NodeShape {
        NodeShape { origin: self.origin, rows: self.rows.len(), input: self.input, outputs: self.outputs }
    }

    /// Where the ports are, in canvas units: the input at the left of the header, the output
    /// at the right of it, or one output per body row for [`Outputs::PerRow`].
    pub fn ports(&self) -> NodePorts {
        self.shape().ports()
    }

    /// The element, for adding handlers (`on_mouse_down`, `on_click`…) before it is added to
    /// the tree. The frame itself needs none: the canvas hit-tests the shapes.
    pub fn build(self) -> Stateful<Div> {
        let vp = self.viewport;
        let z = |units: f32| vp.z(units);
        let origin = if super::geometry::is_finite(self.origin) { self.origin } else { point(0.0, 0.0) };
        let at = vp.to_screen(origin);
        let selected = self.state == NodeState::Selected;
        let spawned = self.state == NodeState::Spawned;
        let port_tone = if selected {
            PortTone::Selected
        } else if spawned {
            PortTone::Spawn
        } else {
            PortTone::Default
        };
        let ports = self.shape().ports();
        let row_count = self.rows.len();
        let (tile_bg, tile_fg) = tile_colors(self.tone);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(z(SPACE_2))
            .h(z(HEADER_HEIGHT - BORDER))
            .px(z(PADDING_X))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(z(TILE))
                    .rounded(z(RADIUS_MD))
                    .bg(tile_bg)
                    .child(icon(self.icon, vp.scale(TILE_ICON), tile_fg)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    // Cut at prepaint against the shaped width at this zoom and clipped to
                    // the box, so the `…` lands before the check mark at every zoom.
                    .child(
                        truncate_block(self.title, vp.scale(18.0))
                            .text_size(z(BODY.size))
                            .line_height(z(18.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(TEXT),
                    )
                    .child(
                        truncate_block(self.subtitle, vp.scale(MONO_SM.line))
                            .font_family(FONT_MONO)
                            .text_size(z(MONO_SM.size))
                            .line_height(z(MONO_SM.line))
                            .text_color(TEXT_MUTED),
                    ),
            )
            .when(selected, |d| {
                d.child(icon(IconName::Check, vp.scale(12.0), ACCENT_TEXT))
            })
            .when(self.state == NodeState::Soon, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(z(CAPTION.size))
                        .line_height(z(CAPTION.line))
                        .text_color(TEXT_MUTED)
                        .child("Soon"),
                )
            });

        let body = (!self.rows.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .border_t(z(BORDER))
                .border_color(LINE)
                .px(z(PADDING_X))
                .pt(z(4.0))
                .children(self.rows.into_iter().map(|(key, value)| {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(z(SPACE_2))
                        .h(z(ROW_HEIGHT))
                        .text_size(z(LABEL.size))
                        .line_height(z(ROW_HEIGHT))
                        .child(div().flex_none().text_color(TEXT_MUTED).whitespace_nowrap().child(key))
                        .child(
                            div().relative().flex_1().min_w_0().h_full().child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .child(
                                        div()
                                            .max_w_full()
                                            .truncate()
                                            .font_family(FONT_MONO)
                                            .text_size(z(MONO_SM.size))
                                            .text_color(TEXT_SECONDARY)
                                            .child(value),
                                    ),
                            ),
                        )
                }))
        });

        // Ports sit on the node's edges; positions are from the padding box, hence the border.
        let port = |centre_x: f32, centre_y: f32| {
            div()
                .absolute()
                .left(z(centre_x - BORDER - PORT_SIZE / 2.0))
                .top(z(centre_y - BORDER - PORT_SIZE / 2.0))
                .child(Port::new(vp, port_tone))
        };
        let local = |p: Point<f32>| (p.x - origin.x, p.y - origin.y);
        let input = ports.input.map(|p| {
            let (x, y) = local(p);
            port(x, y)
        });
        let outputs = ports.outputs.iter().map(|p| {
            let (x, y) = local(*p);
            port(x, y)
        });

        let mut frame = div()
            .id(self.id)
            .absolute()
            .left(px(at.x))
            .top(px(at.y))
            .w(z(NODE_WIDTH))
            .h(z(node_height(row_count)))
            .flex()
            .flex_col()
            .rounded(z(RADIUS_XL))
            .border(z(BORDER))
            .border_color(if selected {
                ACCENT
            } else if spawned {
                SPAWN_LINE
            } else {
                LINE_STRONG
            })
            .bg(if spawned { SPAWNED_GROUND } else { SURFACE })
            .text_color(TEXT)
            .text_size(z(BODY.size))
            .line_height(z(BODY.line));
        if spawned {
            frame = frame.border_dashed();
        }
        if selected {
            frame = frame.shadow(vec![BoxShadow {
                color: ACCENT_GLOW,
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: z(GLOW),
            }]);
        }
        if self.state == NodeState::Soon {
            frame = frame.opacity(SOON_OPACITY);
        }
        frame.child(header).children(body).children(input).children(outputs)
    }
}

impl RenderOnce for NodeFrame {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.build()
    }
}
