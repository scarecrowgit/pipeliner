//! The image viewer: any picture in the app opened over the whole window, to zoom and pan.
//!
//! Open it from a click handler with the images of the place the click came from and the
//! one that was clicked; make thumbnails open it with [`ImageThumb`]:
//!
//! ```ignore
//! use ui_rich::viewer::{ImageThumb, ViewerImage, open_image_viewer};
//!
//! let images: Vec<ViewerImage> = shots.iter().map(|s| ViewerImage::file(&s.path).caption("After")).collect();
//! // A thumbnail that opens the viewer at its own image (pointer, hover border, 150ms fade):
//! ImageThumb::new(("shot", ix), images.clone(), ix).w(px(240.)).h(px(160.)).rounded(px(RADIUS_LG))
//!     .child(img(path).size_full().object_fit(ObjectFit::Contain))
//! // Or from any handler that has the window:
//! open_image_viewer(images, ix, window, cx);
//! ```
//!
//! What the viewer does:
//!
//! - It opens in the overlay host as a full-window layer ([`ui::Overlays::open_full_view`])
//!   over a dark scrim, fading in and out like a dialog, takes keyboard focus and gives it
//!   back when it closes.
//! - The image starts fitted to the window, never enlarged past 100% (one image pixel on one
//!   screen pixel). Ctrl+wheel zooms around the pointer, the plain wheel pans (Shift+wheel
//!   sideways), dragging pans, `+`/`-` zoom, `0` fits, `1` is 100%, `w` fits the width,
//!   double-click toggles fit and 100%, Up/Down scroll. Zoom stays between the fit (or 10%) and 800%; the image cannot
//!   be pushed out of view.
//! - The toolbar shows the name, "2 of 5" with Previous/Next when there are several, zoom out
//!   / percent / zoom in, "Fit", "Fit width", "100%" and Close. Left/Right switch images.
//! - Esc, the close button or a click on the dark area around the image closes it.
//! - A missing or undecodable image shows "Could not load the image" with its path and the
//!   reason, as selectable text.
//!
//! Images are files ([`ViewerImage::file`]; decoded through GPUI's image cache, so a
//! thumbnail drawn with `img(path)` is already loaded) or in-memory [`gpui::Image`]s
//! ([`ViewerImage::memory`]), decoded off the UI thread.
//!
//! The keys are bound by [`crate::bind_keys`]; opening the viewer also binds them once if
//! that was not called (previews, tests).

mod geometry;
mod load;
mod thumb;
mod view;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext as _, Global, Image, KeyBinding, SharedString, Window, actions};
use ui::overlays;

pub use thumb::ImageThumb;

use view::ImageViewer;

/// The overlay id of the viewer's layer: `overlays(window, cx).is_open(VIEWER_ID, cx)`.
pub const VIEWER_ID: &str = "image-viewer";

/// Key context of the open viewer.
pub const KEY_CONTEXT: &str = "ImageViewer";

actions!(
    image_viewer,
    [
        /// One zoom step in (`+`, `=`).
        ZoomIn,
        /// One zoom step out (`-`).
        ZoomOut,
        /// Fit the whole image (`0`).
        ZoomFit,
        /// Make the image as wide as the window and show its top.
        ZoomFitWidth,
        /// One image pixel on one screen pixel (`1`).
        ZoomActual,
        /// The next image (Right).
        NextImage,
        /// The previous image (Left).
        PreviousImage,
        /// Scroll the image up a step (Up).
        PanUp,
        /// Scroll the image down a step (Down).
        PanDown,
    ]
);

/// Where a viewer image comes from.
#[derive(Clone, Debug)]
pub enum ViewerSource {
    /// A file on disk (PNG, JPEG, GIF, WebP…).
    File(Arc<Path>),
    /// An image in memory: pasted from the clipboard, or handed in by a screen.
    Memory(Arc<Image>),
}

impl From<PathBuf> for ViewerSource {
    fn from(path: PathBuf) -> Self {
        ViewerSource::File(path.into())
    }
}

impl From<&Path> for ViewerSource {
    fn from(path: &Path) -> Self {
        ViewerSource::File(path.into())
    }
}

impl From<Arc<Path>> for ViewerSource {
    fn from(path: Arc<Path>) -> Self {
        ViewerSource::File(path)
    }
}

impl From<Arc<Image>> for ViewerSource {
    fn from(image: Arc<Image>) -> Self {
        ViewerSource::Memory(image)
    }
}

/// One image of the viewer: where it comes from, the name the toolbar shows and an optional
/// caption drawn under the image.
#[derive(Clone, Debug)]
pub struct ViewerImage {
    /// The pixels.
    pub source: ViewerSource,
    /// Shown in the toolbar (a file name, "Before", "Pasted image").
    pub name: SharedString,
    /// One line under the image, when set.
    pub caption: Option<SharedString>,
}

impl ViewerImage {
    /// The image file at `path`, named after its file name.
    pub fn file(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self { source: ViewerSource::File(path.into()), name: name.into(), caption: None }
    }

    /// An image in memory, shown as `name`.
    pub fn memory(image: Arc<Image>, name: impl Into<SharedString>) -> Self {
        Self { source: ViewerSource::Memory(image), name: name.into(), caption: None }
    }

    /// Any source, shown as `name`.
    pub fn new(source: impl Into<ViewerSource>, name: impl Into<SharedString>) -> Self {
        Self { source: source.into(), name: name.into(), caption: None }
    }

    /// Show `name` in the toolbar instead.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    /// A line under the image ("After", "Rendered at 1440 × 900").
    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    /// The file it is read from, for a file image.
    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            ViewerSource::File(p) => Some(p),
            ViewerSource::Memory(_) => None,
        }
    }
}

/// Open the viewer over the whole window on `images`, showing `images[start]` first (`start`
/// is clamped). Replaces a viewer that is already open; does nothing for an empty list or in a
/// window without an overlay host. Call it from a click or key handler, not while rendering.
pub fn open_image_viewer(images: Vec<ViewerImage>, start: usize, window: &mut Window, cx: &mut App) {
    if images.is_empty() {
        return;
    }
    bind_keys(cx);
    let view = cx.new(|cx| ImageViewer::new(images, start, cx));
    #[cfg(test)]
    tests::LAST.with(|last| *last.borrow_mut() = Some(view.downgrade()));
    overlays(window, cx).open_full_view(VIEWER_ID, view, |_, _| {}, window, cx);
}

/// Close the viewer, if it is open in this window.
pub fn close_image_viewer(window: &mut Window, cx: &mut App) {
    overlays(window, cx).close(VIEWER_ID, window, cx);
}

/// The viewer is open in this window (not closed or leaving).
pub fn is_image_viewer_open(window: &Window, cx: &App) -> bool {
    overlays(window, cx).is_open(VIEWER_ID, cx)
}

/// Every key binding of the viewer, in [`KEY_CONTEXT`].
pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(KEY_CONTEXT);
    vec![
        KeyBinding::new("+", ZoomIn, c),
        KeyBinding::new("=", ZoomIn, c),
        KeyBinding::new("ctrl-=", ZoomIn, c),
        KeyBinding::new("ctrl-+", ZoomIn, c),
        KeyBinding::new("cmd-=", ZoomIn, c),
        KeyBinding::new("-", ZoomOut, c),
        KeyBinding::new("ctrl--", ZoomOut, c),
        KeyBinding::new("cmd--", ZoomOut, c),
        KeyBinding::new("0", ZoomFit, c),
        KeyBinding::new("ctrl-0", ZoomFit, c),
        KeyBinding::new("cmd-0", ZoomFit, c),
        KeyBinding::new("1", ZoomActual, c),
        KeyBinding::new("w", ZoomFitWidth, c),
        KeyBinding::new("right", NextImage, c),
        KeyBinding::new("left", PreviousImage, c),
        KeyBinding::new("up", PanUp, c),
        KeyBinding::new("down", PanDown, c),
    ]
}

/// Set once the viewer's keys are bound in this app.
struct KeysBound;

impl Global for KeysBound {}

/// Bind the viewer's keys once per app. [`crate::bind_keys`] calls it.
pub fn bind_keys(cx: &mut App) {
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    cx.bind_keys(bindings());
}

