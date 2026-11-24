//! Where a link of an item leads: a local image opens in the image viewer, together with the
//! other local images of the same text or item; anything else opens in the browser.

use std::path::PathBuf;

use gpui::{App, Window};
use ui_rich::viewer::{ViewerImage, open_image_viewer};

/// File types the viewer shows.
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// The file of a link to an image on this machine (`/tmp/shot.png`, `file:///tmp/shot.png`).
pub(crate) fn local_image(url: &str) -> Option<PathBuf> {
    let path = PathBuf::from(url.strip_prefix("file://").unwrap_or(url));
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    (path.is_absolute() && IMAGE_EXTENSIONS.contains(&ext.as_str())).then_some(path)
}

/// The targets of the Markdown links and images of `text`, in order: `[a](url)`, `![a](url)`.
pub(crate) fn markdown_links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        rest = &rest[at + 2..];
        let Some(end) = rest.find(')') else { break };
        let target = rest[..end].trim();
        // `(url "title")` and `(<url>)`.
        let target = target.split_whitespace().next().unwrap_or("");
        let target = target.trim_start_matches('<').trim_end_matches('>');
        if !target.is_empty() {
            out.push(target.to_string());
        }
        rest = &rest[end..];
    }
    out
}

/// The viewer's images of the local image links among `links` (label, url), and the place
/// of `url` among them.
pub(crate) fn image_set(links: &[(String, String)], url: &str) -> (Vec<ViewerImage>, usize) {
    let mut images = Vec::new();
    let mut start = None;
    for (label, target) in links {
        let Some(path) = local_image(target) else {
            continue;
        };
        if target == url && start.is_none() {
            start = Some(images.len());
        }
        let image = ViewerImage::file(&path);
        images.push(if label.is_empty() {
            image
        } else {
            image.caption(label.clone())
        });
    }
    (images, start.unwrap_or(0))
}

/// Opens `url`: in the viewer when it is a local image (with the other images of `links`),
/// in the browser otherwise.
pub(crate) fn open(url: &str, links: &[(String, String)], window: &mut Window, cx: &mut App) {
    if local_image(url).is_some() {
        let (images, start) = image_set(links, url);
        open_image_viewer(images, start, window, cx);
    } else {
        cx.open_url(url);
    }
}
