//! Image attachments shared by chat, issue descriptions and comments: loading / validation,
//! thumbnails, cards and the attach button.
//!
//! Screens own the `Vec<ImageAttachment>`; this module only builds elements and does I/O.
//! `load_image` blocks — run it on a background executor.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyView, App, AppContext as _, ClickEvent, Context, ElementId,
    FontWeight, ImageFormat, IntoElement, ObjectFit, Render, SharedString, Stateful, StyledImage,
    Window, div, img, prelude::*, px, relative,
};

use crate::components::icon;
use crate::motion;
use crate::theme::*;

pub const MAX_IMAGES: usize = 10;
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Border colour on hover for thumbs and cards.
const BORDER_HOVER: gpui::Hsla = hex(0x3a3d44);

/// An image ready to send / show: decoded lazily by gpui, bytes kept for the LLM.
#[derive(Clone, Debug)]
pub struct ImageAttachment {
    pub name: SharedString,
    /// Size in bytes.
    pub bytes: u64,
    pub image: Arc<gpui::Image>,
}

impl ImageAttachment {
    pub fn format(&self) -> ImageFormat {
        self.image.format
    }
    pub fn mime_type(&self) -> &'static str {
        self.image.format.mime_type()
    }
    /// "412 KB" / "1.2 MB".
    pub fn size_label(&self) -> String {
        format_size(self.bytes)
    }
}

/// 1024-based: "880 B", "412 KB" (integer), "1.2 MB" (one decimal).
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        let kb = (bytes as f64 / KB as f64).round() as u64;
        if kb >= 1024 {
            "1.0 MB".to_string()
        } else {
            format!("{kb} KB")
        }
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

/// Image format by file extension. SVG is excluded: models don't accept it.
pub fn format_from_path(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
    }
}

fn too_big(name: &str) -> String {
    format!("{name} is over {} MB", MAX_IMAGE_BYTES / (1024 * 1024))
}

/// Read an image file. Blocking — call from a background executor.
pub fn load_image(path: &Path) -> Result<ImageAttachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let Some(format) = format_from_path(path) else {
        return Err(format!("{name} is not an image"));
    };
    let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(too_big(&name));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(too_big(&name));
    }
    Ok(ImageAttachment {
        name: name.into(),
        bytes: bytes.len() as u64,
        image: Arc::new(gpui::Image::from_bytes(format, bytes)),
    })
}

/// Wrap an image pasted from the clipboard as `pasted-image-{ix}.{ext}`.
pub fn from_clipboard_image(image: gpui::Image, ix: usize) -> Result<ImageAttachment, String> {
    let name = format!("pasted-image-{ix}.{}", extension(image.format));
    if image.format == ImageFormat::Svg {
        return Err(format!("{name} is not an image"));
    }
    let bytes = image.bytes.len() as u64;
    if bytes > MAX_IMAGE_BYTES {
        return Err(too_big(&name));
    }
    Ok(ImageAttachment {
        name: name.into(),
        bytes,
        image: Arc::new(image),
    })
}

fn temp_path(a: &ImageAttachment) -> PathBuf {
    let name: String = a
        .name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    std::env::temp_dir()
        .join("harness-images")
        .join(format!("{}-{name}", a.image.id()))
}

/// Open the image in the system viewer (written to a temp file first).
pub fn open_image(a: &ImageAttachment, cx: &App) {
    let path = temp_path(a);
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&path, &a.image.bytes).is_err() {
            return;
        }
    }
    cx.open_with_system(&path);
}

// ── Caption ─────────────────────────────────────────────────────────────

/// Parts of "2 of 10 images · 1 uploading · 20 MB max each".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caption {
    /// "2 of 10 images"
    pub count: String,
    /// "1 uploading" — render in `INFO_TEXT`.
    pub loading: Option<String>,
    /// "20 MB max each"
    pub limit: String,
}

pub fn caption(count: usize, loading: usize) -> Caption {
    Caption {
        count: format!("{count} of {MAX_IMAGES} images"),
        loading: (loading > 0).then(|| format!("{loading} uploading")),
        limit: format!("{} MB max each", MAX_IMAGE_BYTES / (1024 * 1024)),
    }
}

impl Caption {
    /// 12px / 18px muted caption line with the "uploading" part in `INFO_TEXT`.
    pub fn element(&self) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(TEXT_MUTED)
            .child(self.count.clone())
            .when_some(self.loading.clone(), |d, l| {
                d.child("·").child(div().text_color(INFO_TEXT).child(l))
            })
            .child("·")
            .child(self.limit.clone())
    }
}

// ── Components ──────────────────────────────────────────────────────────

fn thumb_frame(id: impl Into<ElementId>, w: f32, h: f32) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_color(LINE_STRONG)
        .overflow_hidden()
        .hover(|s| s.border_color(BORDER_HOVER))
}

fn corner_button(
    id: impl Into<ElementId>,
    size: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .absolute()
        .top(px(2.))
        .right(px(2.))
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(LINE_STRONG)
        .bg(BG_SIDEBAR)
        .cursor_pointer()
        .child(icon("x", 12., TEXT_SECONDARY))
        .on_click(move |e, w, cx| {
            cx.stop_propagation();
            on_click(e, w, cx)
        })
}

/// Thumbnail of an attached image with a ✕ remove button (`remove_size`: 20, or 18 in comments).
pub fn image_thumb(
    id: impl Into<ElementId>,
    a: &ImageAttachment,
    w: f32,
    h: f32,
    remove_size: f32,
    on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id: ElementId = id.into();
    thumb_frame(id.clone(), w, h)
        .child(
            img(a.image.clone())
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
        .child(corner_button(
            crate::child_id(&id, "remove"),
            remove_size,
            on_remove,
        ))
}

/// Clickable thumbnail of a stored image (no remove button): opens it in the system viewer.
pub fn image_preview(
    id: impl Into<ElementId>,
    a: &ImageAttachment,
    w: f32,
    h: f32,
) -> Stateful<gpui::Div> {
    let open = a.clone();
    thumb_frame(id, w, h)
        .bg(SURFACE)
        .cursor_pointer()
        .child(
            img(a.image.clone())
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
        .on_click(move |_, _, cx| open_image(&open, cx))
}

/// Placeholder while an image loads: dimmed, "Loading", indeterminate bar, ✕ cancel.
pub fn loading_thumb(
    id: impl Into<ElementId>,
    _name: impl Into<SharedString>,
    w: f32,
    h: f32,
    cancel_size: f32,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id: ElementId = id.into();
    let bar = div().h(px(2.)).w(relative(0.4)).bg(INFO);
    let bar = if motion::reduced() {
        bar.into_any_element()
    } else {
        bar.with_animation(
            crate::child_id(&id, "bar"),
            Animation::new(motion::scaled(Duration::from_millis(1200))).repeat(),
            |el, t| el.ml(relative(t * 0.6)),
        )
        .into_any_element()
    };
    thumb_frame(id.clone(), w, h)
        .bg(SURFACE)
        .child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(INFO_TEXT)
                .child("Loading"),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(2.))
                .bg(LINE_STRONG)
                .child(bar),
        )
        .child(corner_button(
            crate::child_id(&id, "cancel"),
            cancel_size,
            on_cancel,
        ))
}

/// Image card in a sent message / comment: preview + "name  size" footer. Click opens it.
pub fn image_card(
    id: impl Into<ElementId>,
    a: &ImageAttachment,
    width: f32,
    preview_h: f32,
) -> Stateful<gpui::Div> {
    let open = a.clone();
    div()
        .id(id)
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_LG))
        .bg(SURFACE)
        .overflow_hidden()
        .cursor_pointer()
        .hover(|s| s.border_color(BORDER_HOVER))
        .child(
            div().w_full().h(px(preview_h)).flex_none().child(
                img(a.image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(4.))
                .border_t_1()
                .border_color(LINE_SUBTLE)
                .font_family(FONT_MONO)
                .text_size(px(TEXT_MONO_SM))
                .line_height(px(16.))
                .text_color(TEXT_MUTED)
                .child(div().flex_1().min_w_0().truncate().child(a.name.clone()))
                .child(div().flex_none().child(a.size_label())),
        )
        .on_click(move |_, _, cx| open_image(&open, cx))
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .font_family(FONT_SANS)
            .text_size(px(TEXT_LABEL))
            .line_height(px(16.))
            .text_color(TEXT)
            .px(px(8.))
            .py(px(4.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .whitespace_nowrap()
            .child(self.0.clone())
    }
}

fn tooltip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| Tooltip(text.clone())).into()
}

/// 28×28 "attach images" button. `tip` e.g. "Attach images · or paste Ctrl+V, drop onto the chat".
pub fn attach_button(
    id: impl Into<ElementId>,
    tip: impl Into<SharedString>,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_color(LINE_STRONG)
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE_HOVER))
        .child(icon("image-plus", 14., TEXT_SECONDARY))
        .tooltip(tooltip(tip.into()))
}

/// 64×64 dashed "Add" tile at the end of a thumbnail row.
pub fn add_image_tile(id: impl Into<ElementId>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(64.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(2.))
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_dashed()
        .border_color(LINE_STRONG)
        .text_color(TEXT_MUTED)
        .text_size(px(11.))
        .cursor_pointer()
        .hover(|s| s.border_color(BORDER_HOVER).text_color(TEXT_SECONDARY))
        .child(icon("image-plus", 14., TEXT_MUTED))
        .child("Add")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(880), "880 B");
        assert_eq!(format_size(412 * 1024), "412 KB");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(1024 * 1024 + 1024 * 1024 / 5), "1.2 MB");
        assert_eq!(format_size(20 * 1024 * 1024), "20.0 MB");
        assert_eq!(format_size(1024 * 1024 - 1), "1.0 MB");
    }

    #[test]
    fn formats() {
        let f = |p: &str| format_from_path(Path::new(p));
        assert_eq!(f("a/b.PNG"), Some(ImageFormat::Png));
        assert_eq!(f("x.jpg"), Some(ImageFormat::Jpeg));
        assert_eq!(f("x.jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(f("x.webp"), Some(ImageFormat::Webp));
        assert_eq!(f("x.gif"), Some(ImageFormat::Gif));
        assert_eq!(f("x.bmp"), Some(ImageFormat::Bmp));
        assert_eq!(f("x.tif"), Some(ImageFormat::Tiff));
        assert_eq!(f("x.tiff"), Some(ImageFormat::Tiff));
        assert_eq!(f("x.svg"), None);
        assert_eq!(f("x.txt"), None);
        assert_eq!(f("noext"), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-images-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn load_errors_and_ok() {
        let txt = scratch("notes.txt");
        std::fs::write(&txt, "hi").unwrap();
        assert_eq!(load_image(&txt).unwrap_err(), "notes.txt is not an image");

        let big = scratch("big.png");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_IMAGE_BYTES + 1)
            .unwrap();
        assert_eq!(load_image(&big).unwrap_err(), "big.png is over 20 MB");

        let missing = scratch("missing.png");
        assert!(
            load_image(&missing)
                .unwrap_err()
                .starts_with("missing.png:")
        );

        let ok = scratch("ok.jpeg");
        std::fs::write(&ok, [1u8, 2, 3]).unwrap();
        let a = load_image(&ok).unwrap();
        assert_eq!(
            (a.name.as_ref(), a.bytes, a.mime_type()),
            ("ok.jpeg", 3, "image/jpeg")
        );
        assert_eq!(a.size_label(), "3 B");
    }

    #[test]
    fn clipboard() {
        let a =
            from_clipboard_image(gpui::Image::from_bytes(ImageFormat::Png, vec![0; 5]), 2).unwrap();
        assert_eq!(a.name.as_ref(), "pasted-image-2.png");
        let big = gpui::Image::from_bytes(ImageFormat::Jpeg, vec![0; MAX_IMAGE_BYTES as usize + 1]);
        assert_eq!(
            from_clipboard_image(big, 1).unwrap_err(),
            "pasted-image-1.jpg is over 20 MB"
        );
    }

    #[test]
    fn captions() {
        let c = caption(2, 1);
        assert_eq!(c.count, "2 of 10 images");
        assert_eq!(c.loading.as_deref(), Some("1 uploading"));
        assert_eq!(c.limit, "20 MB max each");
        assert_eq!(caption(0, 0).loading, None);
    }
}
