//! Decoding viewer images off the UI thread, through GPUI's asset cache.

use std::sync::Arc;

use gpui::{App, Asset, Image, ImgResourceLoader, RenderImage, Resource, SharedString, Window};

use super::ViewerSource;

/// Where one image's decoding stands.
#[derive(Clone)]
pub(super) enum Loaded {
    /// Still decoding; the view is drawn again when it is done.
    Pending,
    /// Decoded; frame 0 is drawn.
    Ready(Arc<RenderImage>),
    /// Could not be read or decoded, with the reason.
    Failed(SharedString),
}

/// Start or poll the decoding of `source`. File images share GPUI's cache with `img(path)`.
pub(super) fn load(source: &ViewerSource, window: &mut Window, cx: &mut App) -> Loaded {
    let result = match source {
        ViewerSource::File(path) => window
            .use_asset::<ImgResourceLoader>(&Resource::Path(path.clone()), cx)
            .map(|r| r.map_err(|e| SharedString::from(e.to_string()))),
        ViewerSource::Memory(image) => window.use_asset::<MemoryImage>(image, cx),
    };
    match result {
        None => Loaded::Pending,
        Some(Ok(image)) if image.frame_count() > 0 => Loaded::Ready(image),
        Some(Ok(_)) => Loaded::Failed("The image has no frames".into()),
        Some(Err(reason)) => Loaded::Failed(reason),
    }
}

/// Forget the decoded copy of an in-memory image, once the viewer is gone.
pub(super) fn release(source: &ViewerSource, cx: &mut App) {
    if let ViewerSource::Memory(image) = source {
        cx.remove_asset::<MemoryImage>(image);
    }
}

/// Decodes an in-memory image on the background executor.
enum MemoryImage {}

impl Asset for MemoryImage {
    type Source = Arc<Image>;
    type Output = Result<Arc<RenderImage>, SharedString>;

    fn load(source: Self::Source, cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move { source.to_image_data(renderer).map_err(|e| SharedString::from(e.to_string())) }
    }
}
