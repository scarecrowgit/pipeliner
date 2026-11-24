//! Image viewer preview: a row of thumbnails ([`ImageThumb`]) for the files given on the
//! command line, plus one missing file, each opening the viewer on the whole set.
//!
//! Dev-driver probes: `thumb-<n>` on the page; in the viewer `viewer-stage`, `viewer-image`,
//! `viewer-name`, `viewer-position`, `viewer-prev`, `viewer-next`, `viewer-zoom-out`,
//! `viewer-zoom` (its value is the percent), `viewer-zoom-in`, `viewer-fit`,
//! `viewer-fit-width`, `viewer-actual`, `viewer-close`, `viewer-caption`, `viewer-failed`.
//!
//! `cargo run -p ui_rich --example viewer -- page.png wide.png`

use std::path::PathBuf;

use gpui::{Context, IntoElement, ObjectFit, Render, StyledImage, Window, div, img, prelude::*, px};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::viewer::{ImageThumb, ViewerImage};

struct Page {
    paths: Vec<PathBuf>,
}

impl Page {
    fn images(&self) -> Vec<ViewerImage> {
        self.paths
            .iter()
            .enumerate()
            .map(|(ix, p)| ViewerImage::file(p).caption(format!("Screenshot {} · {}", ix + 1, p.display())))
            .collect()
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let images = self.images();
        let thumbs = self.paths.iter().enumerate().map(|(ix, path)| {
            col()
                .flex_none()
                .gap(px(SPACE_1_5))
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(images[ix].name.clone()))
                .child(
                    ImageThumb::new(("thumb", ix), images.clone(), ix)
                        .tooltip("Open image")
                        .w(px(240.))
                        .h(px(160.))
                        .rounded(px(RADIUS_LG))
                        .bg(BG_SIDEBAR)
                        .overflow_hidden()
                        .child(img(path.clone()).size_full().object_fit(ObjectFit::Contain))
                        .probe(format!("thumb-{ix}")),
                )
        });
        ui::root().flex().flex_col().gap(px(SPACE_4)).p(px(SPACE_6)).child(
            div().type_style(TITLE_SM).child("Image viewer"),
        )
        .child(
            div()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Click a thumbnail. Ctrl+wheel zooms, the wheel and dragging pan, + − 0 1, Left and Right, Esc."),
        )
        .child(row().gap(px(SPACE_4)).items_start().children(thumbs))
    }
}

fn main() {
    let mut paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    paths.push(PathBuf::from("/nonexistent/folder/missing-render.png"));
    ui::run_app("Image viewer", 1440., 900., move |_, cx| {
        ui_rich::bind_keys(cx);
        cx.new(|_| Page { paths })
    });
}
