//! The toolbar over the canvas: "Add" and a chip per kind of node, the gesture hint, the zoom
//! and "Fit". It drives the canvas through its public methods; the canvas hides its own
//! controls while the frame shows these.
//!
//! Probes: `toolbar-add-<kind>` (`toolbar-add-agent`), `editor-zoom-out`, `editor-zoom`,
//! `editor-zoom-in`, `editor-fit`.

use gpui::{AnyElement, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use ui::dev::Probe;
use ui::prelude::*;
use ui::tooltip::Tooltip;

use super::EditorView;
use crate::canvas::NewNode;

/// Height of the toolbar.
const TOOLBAR_HEIGHT: f32 = 48.0;
/// Width of the zoom figure between the two zoom buttons.
const ZOOM_LABEL_WIDTH: f32 = 40.0;

impl EditorView {
    pub(super) fn render_toolbar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let chips: Vec<AnyElement> = NewNode::ALL.iter().map(|&kind| self.add_chip(kind)).collect();
        let zoom = (self.parts.canvas.read(cx).zoom() * 100.0).round() as u32;
        let zoom_group = row()
            .flex_none()
            .ml(px(SPACE_3))
            .h(px(CONTROL_HEIGHT_SM))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .child(
                IconButton::new("editor-zoom-out", IconName::Minus)
                    .small()
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| this.parts.canvas.update(cx, |c, cx| c.zoom_out(cx))))
                    .probe("editor-zoom-out"),
            )
            .child(
                div()
                    .w(px(ZOOM_LABEL_WIDTH))
                    .flex()
                    .justify_center()
                    .type_style(MONO_SM)
                    .tabular()
                    .child(format!("{zoom}%"))
                    .probe_value("editor-zoom", format!("{zoom}%")),
            )
            .child(
                IconButton::new("editor-zoom-in", IconName::Plus)
                    .small()
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.parts.canvas.update(cx, |c, cx| c.zoom_in(cx))))
                    .probe("editor-zoom-in"),
            );
        row()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .pl(px(SPACE_5))
            .pr(px(SPACE_3))
            .gap(px(SPACE_1_5))
            .border_b_1()
            .border_color(LINE)
            .child(div().flex_none().mr(px(SPACE_1)).type_style(LABEL).text_color(TEXT_MUTED).child("Add"))
            .children(chips)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .ml(px(SPACE_3))
                    .flex()
                    .justify_end()
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .text_right()
                    .child(truncate_lh("Drag a node by its title · drag from a port to connect", LABEL.line)),
            )
            .child(zoom_group)
            .child(
                Button::new("editor-fit", "Fit")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.parts.canvas.update(cx, |c, cx| c.zoom_to_fit(cx))))
                    .probe("editor-fit"),
            )
            .into_any_element()
    }

    /// The chip that adds a node of `kind` in the middle of the view.
    fn add_chip(&self, kind: NewNode) -> AnyElement {
        let name: SharedString = format!("toolbar-add-{}", kind.key()).into();
        let mut chip = Chip::new(ElementId::Name(name.clone()), kind.label()).icon(kind.icon());
        if kind == NewNode::Subagent {
            chip = chip.spawn();
        }
        if !kind.available() {
            return div()
                .id(ElementId::Name(format!("{name}-soon").into()))
                .flex_none()
                .opacity(0.45)
                .tooltip(Tooltip::text("Coming soon"))
                .child(chip)
                .probe(name)
                .into_any_element();
        }
        let (canvas, inspector) = (self.parts.canvas.clone(), self.parts.inspector.clone());
        div()
            .flex_none()
            .child(chip.on_click(move |_, window, cx| {
                if canvas.update(cx, |canvas, cx| canvas.add_node(kind, None, cx)).is_some() {
                    inspector.update(cx, |inspector, cx| inspector.focus_first_field(window, cx));
                }
            }))
            .probe(name)
            .into_any_element()
    }
}
