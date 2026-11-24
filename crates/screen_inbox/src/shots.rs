//! The renders card of a review: before and after pictures of rendered files. Each picture is
//! a thumbnail that opens the image viewer on every picture of the card, in reading order, so
//! Left/Right walk through one variant's screens (and flip between a screen's before and after).

use std::path::PathBuf;

use gpui::{AnyElement, Context, ElementId, IntoElement, ObjectFit, div, img, prelude::*, px};
use hub::api::{BlobId, Render as Shot, Review, Variant, variant_letter};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::viewer::{ImageThumb, ViewerImage};

use crate::texts::{Look, block};
use crate::view::InboxView;

/// The tallest a thumbnail gets; a taller page shows its top.
pub(crate) const SHOT_HEIGHT: f32 = 220.;
/// The widest a thumbnail gets.
const SHOT_MAX_WIDTH: f32 = 520.;
/// The narrowest a thumbnail gets, however little room there is.
const SHOT_MIN_WIDTH: f32 = 96.;

/// One rendered file of the card, with the variant it belongs to (`None`: the review's own).
#[derive(Clone)]
pub(crate) struct Screen {
    pub(crate) shot: Shot,
    pub(crate) variant: Option<u32>,
}

/// One picture of the card: which screen, before or after, and the blob it shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Picture {
    pub(crate) screen: usize,
    pub(crate) before: bool,
    pub(crate) blob: BlobId,
}

/// The screens the review shows: its own renders, then the chosen variant's.
pub(crate) fn screens(review: &Review, variant: Option<&Variant>) -> Vec<Screen> {
    review
        .renders
        .iter()
        .map(|shot| Screen {
            shot: shot.clone(),
            variant: None,
        })
        .chain(variant.into_iter().flat_map(|v| {
            v.renders.iter().map(|shot| Screen {
                shot: shot.clone(),
                variant: Some(v.index),
            })
        }))
        .collect()
}

/// Every picture in reading order: each screen's "Before" (when there is one), then its
/// "After". The viewer walks them in this order.
pub(crate) fn pictures(screens: &[Screen]) -> Vec<Picture> {
    let mut out = Vec::new();
    for (ix, screen) in screens.iter().enumerate() {
        if let Some(before) = &screen.shot.before {
            out.push(Picture {
                screen: ix,
                before: true,
                blob: before.clone(),
            });
        }
        out.push(Picture {
            screen: ix,
            before: false,
            blob: screen.shot.after.clone(),
        });
    }
    out
}

/// The viewer's images of `pictures`: named after the rendered file, captioned "After ·
/// Variant A · 1440 × 4800".
pub(crate) fn viewer_images(
    screens: &[Screen],
    pictures: &[Picture],
    path: impl Fn(&BlobId) -> PathBuf,
) -> Vec<ViewerImage> {
    pictures
        .iter()
        .map(|picture| {
            let screen = &screens[picture.screen];
            let mut caption = vec![side(picture.before).to_string()];
            if let Some(index) = screen.variant {
                caption.push(format!("Variant {}", variant_letter(index)));
            }
            if let Some(size) = size_label(&screen.shot) {
                caption.push(size);
            }
            ViewerImage::file(path(&picture.blob))
                .name(screen.shot.path.clone())
                .caption(caption.join(" · "))
        })
        .collect()
}

fn side(before: bool) -> &'static str {
    if before { "Before" } else { "After" }
}

/// "1440 × 4800", when the size is known.
fn size_label(shot: &Shot) -> Option<String> {
    // Pixel sizes read without thousands separators, as image tools print them.
    (shot.width > 0 && shot.height > 0).then(|| format!("{} × {}", shot.width, shot.height))
}

/// How a picture sits in its thumbnail: the thumbnail's outer size, and the height the image
/// is drawn at (its width is the thumbnail's inner width), or `None` to fit an image of
/// unknown size inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThumbSize {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) image_height: Option<f32>,
}

/// The thumbnail of `shot` in `slot` pixels of width: the image at the thumbnail's width
/// (never enlarged past its own), at most [`SHOT_HEIGHT`] tall, so a tall page shows its top
/// rather than a sliver of the whole page.
pub(crate) fn thumb_size(shot: &Shot, slot: f32) -> ThumbSize {
    let border = 2.;
    let room = slot.clamp(SHOT_MIN_WIDTH, SHOT_MAX_WIDTH);
    if shot.width == 0 || shot.height == 0 {
        return ThumbSize {
            width: room,
            height: SHOT_HEIGHT,
            image_height: None,
        };
    }
    let inner = (room - border).min(shot.width as f32).max(1.);
    let image_height = inner * shot.height as f32 / shot.width as f32;
    ThumbSize {
        width: inner + border,
        height: image_height.min(SHOT_HEIGHT - border) + border,
        image_height: Some(image_height),
    }
}

impl InboxView {
    /// The renders card, when the review or the chosen variant has renders.
    pub(crate) fn render_renders(
        &mut self,
        review: &Review,
        variant: Option<&Variant>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let screens = screens(review, variant);
        if screens.is_empty() {
            return None;
        }
        let pictures = pictures(&screens);
        let queries = hub::queries(cx);
        let images = viewer_images(&screens, &pictures, |blob| queries.blob_path(blob));
        // The card's padding and border.
        let inner = self.content_width() - 2. * SPACE_4 - 2.;
        let rows: Vec<AnyElement> = screens
            .iter()
            .enumerate()
            .map(|(ix, screen)| {
                let path = self.texts.get(
                    format!("shot-{ix}-path"),
                    screen.shot.path.clone(),
                    Look::plain(MONO, TEXT),
                    cx,
                );
                let own: Vec<usize> = (0..pictures.len())
                    .filter(|p| pictures[*p].screen == ix)
                    .collect();
                let n = own.len().max(1) as f32;
                let slot = (inner - SPACE_4 * (n - 1.)) / n;
                let size = thumb_size(&screen.shot, slot);
                let thumbs = own.into_iter().map(|p| {
                    let picture = &pictures[p];
                    let path = queries.blob_path(&picture.blob);
                    let picture_el = match size.image_height {
                        Some(h) => img(path)
                            .flex_none()
                            .w(px(size.width - 2.))
                            .h(px(h))
                            .object_fit(ObjectFit::Fill),
                        None => img(path).size_full().object_fit(ObjectFit::Contain),
                    };
                    col()
                        .flex_none()
                        .w(px(size.width))
                        .gap(px(SPACE_1_5))
                        .child(
                            row()
                                .gap(px(SPACE_2))
                                .type_style(LABEL)
                                .text_color(TEXT_MUTED)
                                .child(side(picture.before))
                                .child(grow())
                                .when_some(size_label(&screen.shot), |d, label| {
                                    d.child(div().flex_none().type_style(MONO_SM).child(label))
                                }),
                        )
                        .child(
                            ImageThumb::new(
                                ElementId::Name(format!("review-shot-{p}").into()),
                                images.clone(),
                                p,
                            )
                            .flex()
                            .flex_col()
                            .w(px(size.width))
                            .h(px(size.height))
                            .rounded(px(RADIUS_LG))
                            .bg(BG_SIDEBAR)
                            .overflow_hidden()
                            .child(picture_el)
                            .probe(format!("review-shot-{p}")),
                        )
                });
                col()
                    .gap(px(SPACE_2))
                    .p(px(SPACE_4))
                    .when(ix > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
                    .child(block("review-shot-path", path))
                    .child(row().items_start().gap(px(SPACE_4)).children(thumbs))
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .debug_selector(|| "review-renders".into())
                .child(
                    Card::new()
                        .flush()
                        .header(
                            CardHeader::new("Renders")
                                .count(base::fmt::count(screens.len() as u64)),
                        )
                        .child(col().children(rows)),
                )
                .into_any_element(),
        )
    }
}
