//! Attachment chips (`.thumb`): a thumbnail or file icon, a caption with the name and size
//! (or upload progress), and a remove button; with the limits note at the end of the row.

use gpui::{
    AnyElement, App, ElementId, Entity, IntoElement, ObjectFit, StyledImage, Window, div, img,
    prelude::*, px,
};
use ui::components::{Progress, Tooltip};
use ui::dev::Probe;
use ui::motion::Look;
use ui::prelude::*;

use super::types::{Attachment, AttachmentContent};
use super::view::part;
use super::{Composer, ComposerKind, spans};

/// Chip size, from the composer mockups.
const CHIP_W: f32 = 112.;
const CHIP_H: f32 = 72.;
/// Height of the caption strip at the bottom of a chip.
const CAPTION_H: f32 = 22.;

/// The chips row above the text, or nothing when there are no attachments.
pub(super) fn chips_row(
    entity: &Entity<Composer>,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let c = entity.read(cx);
    if c.attachments.is_empty() {
        return None;
    }
    let (name, kind, off) = (c.name.clone(), c.kind, c.disabled.is_some());
    let attachments = c.attachments.clone();
    let lines = spans::summary(&attachments, c.limits);
    let chips: Vec<AnyElement> = attachments
        .iter()
        .enumerate()
        .map(|(ix, a)| chip(entity, &name, ix, a, off, window, cx))
        .collect();
    let note = match kind {
        ComposerKind::Chat => div().child(lines.join(" · ")),
        ComposerKind::Issue => {
            let (first, rest) = lines.split_first().map_or((String::new(), Vec::new()), |(f, r)| {
                (f.clone(), r.to_vec())
            });
            col().child(first).child(rest.join(" · "))
        }
    };
    Some(
        row()
            .id(part(&name, "chips"))
            .items_end()
            .gap(px(SPACE_2))
            .px(px(SPACE_2_5))
            .pt(px(SPACE_2_5))
            .overflow_x_scroll()
            .children(chips)
            .child(note.flex_none().type_style(LABEL).text_color(TEXT_MUTED))
            .into_any_element(),
    )
}

/// One chip.
fn chip(
    entity: &Entity<Composer>,
    name: &str,
    ix: usize,
    a: &Attachment,
    off: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let uploading = a.progress;
    let preview: AnyElement = match &a.content {
        AttachmentContent::Image(image) => {
            img(image.clone()).size_full().object_fit(ObjectFit::Cover).into_any_element()
        }
        AttachmentContent::ImageFile(path) => {
            img(path.clone()).size_full().object_fit(ObjectFit::Cover).into_any_element()
        }
        AttachmentContent::File(_) => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(IconName::File, 16., TEXT_MUTED))
            .into_any_element(),
    };
    let shot = div()
        .absolute()
        .top(px(SPACE_1_5))
        .left(px(SPACE_1_5))
        .right(px(SPACE_1_5))
        .bottom(px(CAPTION_H + SPACE_1))
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(LINE)
        .bg(SURFACE)
        .overflow_hidden()
        .when(uploading.is_some(), |d| d.opacity(0.4))
        .child(preview);
    let meta = match uploading {
        Some(p) => format!("{}%", base::fmt::percent(f64::from(p), 1.0)),
        None => base::fmt::bytes(a.size),
    };
    let caption = row()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(CAPTION_H))
        .gap(px(SPACE_1_5))
        .px(px(SPACE_2))
        .type_style(CAPTION)
        .text_color(TEXT_SECONDARY)
        .bg(SURFACE_RAISED)
        .border_t_1()
        .border_color(LINE)
        .child(truncate_lh(a.name.clone(), CAPTION.line))
        .child(div().flex_none().text_color(TEXT_MUTED).child(meta));
    let progress = uploading.map(|p| {
        div().absolute().left(px(SPACE_2)).right(px(SPACE_2)).top(px(34.)).child(Progress::new(p))
    });
    let remove = (!off).then(|| {
        let label = if uploading.is_some() {
            "Cancel upload".to_string()
        } else {
            format!("Remove {}", a.name)
        };
        remove_button(entity, name, ix, a, label, window, cx)
    });
    div()
        .id(ElementId::NamedInteger(format!("{name}-chip").into(), a.id.0))
        .relative()
        .flex_none()
        .w(px(CHIP_W))
        .h(px(CHIP_H))
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_LG))
        .bg(BG_SIDEBAR)
        .overflow_hidden()
        .child(shot)
        .children(progress)
        .child(caption)
        .children(remove)
        .probe_value(format!("{name}-chip-{ix}"), a.name.clone())
        .into_any_element()
}

/// The × in a chip's corner: removes it, or cancels its upload.
fn remove_button(
    entity: &Entity<Composer>,
    name: &str,
    ix: usize,
    a: &Attachment,
    label: String,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let id = ElementId::NamedInteger(format!("{name}-chip-x").into(), a.id.0);
    let rest = Look::bg(SURFACE_RAISED).fg(TEXT_MUTED).border(LINE_STRONG);
    let hover = Look::bg(SURFACE_HOVER).fg(TEXT).border(LINE_STRONG);
    let fade = motion::hover_fade_state(id.clone(), rest, hover, window, cx);
    let fg = fade.fg.unwrap_or(TEXT_MUTED);
    let (e, att) = (entity.clone(), a.id);
    fade.apply(
        div()
            .id(id)
            .absolute()
            .top(px(SPACE_1))
            .right(px(SPACE_1))
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_SM))
            .border_1()
            .cursor_pointer(),
    )
    .child(icon(IconName::X, 10., fg))
    .tooltip(Tooltip::text(label))
    .on_click(move |_, _, cx| e.update(cx, |c, cx| c.remove_by_user(att, cx)))
    .probe(format!("{name}-chip-{ix}-remove"))
}
