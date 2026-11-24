//! The toolbar over the canvas: "Add" and a chip per kind of node, the gesture hint, the zoom
//! and "Fit". It drives the canvas through its public methods; the canvas hides its own
//! controls while the frame shows these.
//!
//! In a narrow editor the hint gives way first (it ends in `…`, then is gone); when even the
//! labelled chips do not fit, the chips show only their icons, with the label as a tooltip.
//! The widths this needs are measured from the last frame ([`super::Measured`]).
//!
//! Probes: `toolbar-add-<kind>` (`toolbar-add-agent`), `editor-zoom-out`, `editor-zoom`,
//! `editor-zoom-in`, `editor-fit`.

use gpui::{AnyElement, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui::tooltip::Tooltip;

use super::{EditorView, measure_width, store_width};
use crate::canvas::NewNode;

/// Height of the toolbar.
const TOOLBAR_HEIGHT: f32 = 48.0;
/// Width of the zoom figure between the two zoom buttons.
const ZOOM_LABEL_WIDTH: f32 = 40.0;
/// Size of the icon in an add chip.
const CHIP_ICON: f32 = 12.0;

impl EditorView {
    /// The chips show only their icons: the editor is too narrow for their labels.
    pub(crate) fn toolbar_compact(&self) -> bool {
        let m = &self.measured;
        let Some(add) = m.labelled_add.get() else { return false };
        // Padding, the gaps around the hint and its margin, with the hint at nothing.
        let fixed = SPACE_5 + SPACE_1_5 + SPACE_3 + SPACE_1_5 + SPACE_3;
        m.width.get() + 0.5 < add + m.view_controls.get() + fixed
    }

    pub(super) fn render_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let compact = self.toolbar_compact();
        let chips: Vec<AnyElement> =
            NewNode::ALL.iter().map(|&kind| if compact { self.icon_chip(kind, window, cx) } else { self.add_chip(kind) }).collect();
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
        let measured = self.measured.clone();
        let add = row()
            .relative()
            .flex_none()
            .gap(px(SPACE_1_5))
            .child(div().flex_none().mr(px(SPACE_1)).type_style(LABEL).text_color(TEXT_MUTED).child("Add"))
            .children(chips)
            .child(measure_width(move |width| {
                // Only the labelled chips tell how much room the labels want.
                if compact || measured.labelled_add.get().is_some_and(|known| (known - width).abs() < 0.5) {
                    return false;
                }
                measured.labelled_add.set(Some(width));
                true
            }));
        let measured = self.measured.clone();
        let view_controls = row()
            .relative()
            .flex_none()
            .gap(px(SPACE_1_5))
            .child(zoom_group)
            .child(
                Button::new("editor-fit", "Fit")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.parts.canvas.update(cx, |c, cx| c.zoom_to_fit(cx))))
                    .probe("editor-fit"),
            )
            .child(measure_width(move |width| store_width(&measured.view_controls, width)));
        row()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .min_w_0()
            .overflow_hidden()
            .pl(px(SPACE_5))
            .pr(px(SPACE_3))
            .gap(px(SPACE_1_5))
            .border_b_1()
            .border_color(LINE)
            .child(add)
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
            .child(view_controls)
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

    /// The add chip of a narrow toolbar: the icon in the chip's frame, the label as a tooltip.
    fn icon_chip(&self, kind: NewNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let name: SharedString = format!("toolbar-add-{}", kind.key()).into();
        let spawn = kind == NewNode::Subagent;
        let fg = if spawn { SPAWN_TEXT } else { TEXT_SECONDARY };
        let id = ElementId::Name(name.clone());
        let faded = motion::hover_fade_state(
            id.clone(),
            Look::bg(TRANSPARENT).fg(fg),
            Look::bg(SURFACE_RAISED).fg(if spawn { SPAWN_TEXT } else { TEXT }),
            window,
            cx,
        );
        let color = faded.fg.unwrap_or(fg);
        let available = kind.available();
        let tip = if available { kind.label().to_string() } else { format!("{}: coming soon", kind.label()) };
        let selector = name.to_string();
        let chip = faded
            .apply(div().id(id).debug_selector(move || selector))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(CONTROL_HEIGHT_SM))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .tooltip(Tooltip::text(tip))
            .child(icon(kind.icon(), CHIP_ICON, color));
        if !available {
            return chip.opacity(0.45).probe(name).into_any_element();
        }
        let (canvas, inspector) = (self.parts.canvas.clone(), self.parts.inspector.clone());
        chip.cursor_pointer()
            .on_click(move |_, window, cx| {
                if canvas.update(cx, |canvas, cx| canvas.add_node(kind, None, cx)).is_some() {
                    inspector.update(cx, |inspector, cx| inspector.focus_first_field(window, cx));
                }
            })
            .probe(name)
            .into_any_element()
    }
}
