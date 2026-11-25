//! Pictures in Markdown text.
//!
//! - [`local_image`] says whether an image or link target names an image file on this
//!   machine (pure; no file is touched).
//! - [`Images`] is what a view knows about those files: still loading, readable with its
//!   pixel size, or unreadable. Only a readable file becomes a thumbnail or opens the viewer;
//!   anything else stays link text.
//! - [`text_runs`] and [`thumbs`] split a paragraph into the text it still shows and the
//!   thumbnails drawn under it; the unit walk and the renderer both use them, so they agree
//!   on which paragraphs have a text unit.
//! - [`Gallery`] is the viewer's list: every readable local image of the text, thumbnails and
//!   links alike, in source order, each file once.
//! - [`strip`] and [`thumb`] draw the thumbnails.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use gpui::{AnyElement, Div, ObjectFit, StyledImage, div, img, prelude::*, px};
use ui::dev::Probe as _;
use ui::prelude::*;

use super::parse::{Block, Document, Inline};
use crate::viewer::{ImageThumb, ViewerImage};

/// File extensions of the pictures a thumbnail and the viewer can show.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// The widest a thumbnail gets, border included.
pub(crate) const THUMB_MAX_WIDTH: f32 = 520.;
/// The tallest a thumbnail gets, border included; a taller picture shows its top.
pub(crate) const THUMB_MAX_HEIGHT: f32 = 220.;

/// The image file an image or link target names, when it names a local picture.
///
/// Accepted: an absolute path (`/home/me/shot.png`), a `file://` URL (`file:///…`,
/// `file://localhost/…`), a path under the home directory (`~/shot.png`), and a relative
/// path (`renders/before.png`, `./before.png`, `../out/before.png`) when `base` is given,
/// resolved against it. Percent escapes (`my%20shot.png`) are decoded and `.`/`..` are
/// folded. The extension must be one of [`IMAGE_EXTENSIONS`] (any case).
///
/// `None` for anything else: `http(s):`, `data:` and other URL schemes, a relative path
/// without `base`, another file type. The file system is not touched: whether the file
/// exists and decodes is for the caller to find out.
pub fn local_image(target: &str, base: Option<&Path>) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let path = if let Some(rest) = strip_prefix_ignore_case(target, "file://") {
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        if !rest.starts_with('/') {
            return None;
        }
        PathBuf::from(percent_decode(rest)?)
    } else if has_scheme(target) {
        return None;
    } else {
        let decoded = percent_decode(target)?;
        if decoded.starts_with("~/") {
            base::paths::Paths::from_env().expand(&decoded)
        } else if decoded.starts_with('/') {
            PathBuf::from(decoded)
        } else {
            base?.join(decoded)
        }
    };
    let path = fold(&path);
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str()).then_some(path)
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// `text` starts with a URL scheme (`https:`, `data:`, `mailto:`).
fn has_scheme(text: &str) -> bool {
    let Some(colon) = text.find(':') else {
        return false;
    };
    let scheme = &text[..colon];
    scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `%XX` escapes decoded; a `%` without two hex digits is kept. `None` when the bytes are
/// not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_string());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| hex(*b)),
                bytes.get(i + 2).and_then(|b| hex(*b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `.` dropped and `..` folded into its parent, without looking at the disk.
fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// What a view knows about one local image file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Probe {
    /// Being read and decoded off the UI thread.
    Loading,
    /// Decoded; its size in image pixels.
    Ready { width: f32, height: f32 },
    /// Missing, unreadable or not a picture.
    Failed,
}

/// The local image files of a view's text and what is known about each. Kept across
/// [`set_text`](super::Markdown::set_text), so a streaming message does not reload them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Images {
    /// The folder a relative target is resolved against.
    pub base: Option<PathBuf>,
    pub files: HashMap<PathBuf, Probe>,
}

impl Images {
    /// The local image file `target` names (see [`local_image`]).
    pub fn path(&self, target: &str) -> Option<PathBuf> {
        local_image(target, self.base.as_deref())
    }

    /// The file and pixel size of `target`, when it is a readable local image.
    pub fn ready(&self, target: &str) -> Option<(PathBuf, f32, f32)> {
        let path = self.path(target)?;
        match self.files.get(&path)? {
            Probe::Ready { width, height } => Some((path, *width, *height)),
            _ => None,
        }
    }

    fn is_thumb(&self, run: &Inline) -> bool {
        run.image && run.link.as_deref().is_some_and(|t| self.ready(t).is_some())
    }
}

/// One thumbnail of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Thumb {
    pub path: PathBuf,
    pub width: f32,
    pub height: f32,
    /// The caption; empty for none.
    pub alt: String,
}

/// The thumbnails of a paragraph's `runs`, in source order: its images that are readable
/// local files.
pub(crate) fn thumbs(runs: &[Inline], images: &Images) -> Vec<Thumb> {
    runs.iter()
        .filter(|r| r.image)
        .filter_map(|r| {
            let (path, width, height) = images.ready(r.link.as_deref()?)?;
            Some(Thumb {
                path,
                width,
                height,
                alt: r.text.clone(),
            })
        })
        .collect()
}

/// The runs a text unit shows for `runs`. An image that stays text shows its alt text, or
/// its file name when the alt is empty. With `cut` (a paragraph) the thumbnails are taken
/// out: the whitespace where one stood is not doubled, and the text is trimmed at both ends.
/// Empty when nothing but thumbnails is left.
pub(crate) fn text_runs<'a>(runs: &'a [Inline], images: &Images, cut: bool) -> Cow<'a, [Inline]> {
    if !runs.iter().any(|r| r.image) {
        return Cow::Borrowed(runs);
    }
    let mut out: Vec<Inline> = Vec::with_capacity(runs.len());
    let mut after_cut = false;
    for run in runs {
        if cut && images.is_thumb(run) {
            after_cut = true;
            continue;
        }
        let mut run = run.clone();
        if run.image && run.text.is_empty() {
            run.text = file_name(run.link.as_deref().unwrap_or_default());
        }
        // At the start, or after a space: the space after the thumbnail is not needed.
        let blank_before = out
            .last()
            .is_none_or(|r| r.text.ends_with(char::is_whitespace));
        if after_cut && blank_before {
            run.text = run.text.trim_start().to_string();
        }
        after_cut = false;
        if !run.text.is_empty() {
            out.push(run);
        }
    }
    if cut && runs.iter().any(|r| images.is_thumb(r)) {
        while let Some(last) = out.last_mut() {
            let keep = last.text.trim_end().len();
            last.text.truncate(keep);
            if last.text.is_empty() {
                out.pop();
            } else {
                break;
            }
        }
    }
    Cow::Owned(out)
}

/// The last path segment of an image target, for an image without alt text.
fn file_name(target: &str) -> String {
    let path = target.split(['?', '#']).next().unwrap_or(target);
    let name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path);
    percent_decode(name).unwrap_or_else(|| name.to_string())
}

/// Every link and image target of `doc`, in source order (repeats included).
pub(crate) fn targets(doc: &Document) -> Vec<&str> {
    let mut out = Vec::new();
    each_run(&doc.blocks, &mut |r| {
        if let Some(t) = r.link.as_deref() {
            out.push(t);
        }
    });
    out
}

/// Calls `f` on every inline run of `blocks` in reading order.
fn each_run<'a>(blocks: &'a [Block], f: &mut impl FnMut(&'a Inline)) {
    for block in blocks {
        match block {
            Block::Paragraph(runs) | Block::Heading { text: runs, .. } => runs.iter().for_each(&mut *f),
            Block::Quote(blocks) => each_run(blocks, f),
            Block::List(list) => list.items.iter().for_each(|i| each_run(&i.blocks, f)),
            Block::Table(t) => t
                .head
                .iter()
                .chain(t.rows.iter().flatten())
                .flatten()
                .for_each(&mut *f),
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

/// The images the viewer opens on: every readable local image of a text, as a thumbnail,
/// image link text or plain link, in source order, each file once.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gallery {
    pub images: Vec<ViewerImage>,
    by_path: HashMap<PathBuf, usize>,
    by_target: HashMap<String, usize>,
}

impl Gallery {
    /// The gallery of `doc`. An image's alt text (a link's text) is its caption.
    pub fn build(doc: &Document, images: &Images) -> Self {
        let mut g = Gallery::default();
        let mut prev_link: Option<&str> = None;
        each_run(&doc.blocks, &mut |r| {
            let Some(target) = r.link.as_deref() else {
                prev_link = None;
                return;
            };
            // A link of several runs (`[a **b**](x.png)`) is captioned by its first run.
            let continues = !r.image && prev_link == Some(target);
            prev_link = (!r.image).then_some(target);
            if continues || g.by_target.contains_key(target) {
                return;
            }
            let Some((path, _, _)) = images.ready(target) else {
                return;
            };
            let ix = match g.by_path.get(&path) {
                Some(ix) => *ix,
                None => {
                    let mut image = ViewerImage::file(&path);
                    if !r.text.trim().is_empty() {
                        image = image.caption(r.text.trim().to_string());
                    }
                    g.images.push(image);
                    g.by_path.insert(path, g.images.len() - 1);
                    g.images.len() - 1
                }
            };
            g.by_target.insert(target.to_string(), ix);
        });
        g
    }

    /// No readable local image in the text.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// The place of the image `target` names, when it is in the gallery.
    pub fn index_of_target(&self, target: &str) -> Option<usize> {
        self.by_target.get(target).copied()
    }

    /// The place of the image file `path`.
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.by_path.get(path).copied()
    }
}

/// The row of a paragraph's thumbnails: side by side while they fit, wrapping below each
/// other when they do not.
pub(crate) fn strip(thumbs: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_start()
        .w_full()
        .min_w_0()
        .flex_none()
        .gap(px(SPACE_3))
        .children(thumbs)
}

/// The `n`-th thumbnail of a view: the picture at its own proportions, at most
/// [`THUMB_MAX_WIDTH`] wide (never wider than its row) and [`THUMB_MAX_HEIGHT`] tall (a
/// taller one is cut, showing its top), its alt text as a one-line caption under it. A
/// click opens the viewer on `gallery` at this picture.
pub(crate) fn thumb(n: usize, t: &Thumb, gallery: &Gallery) -> AnyElement {
    let start = gallery.index_of(&t.path).unwrap_or(0);
    let border = 2.;
    let width = (t.width + border).min(THUMB_MAX_WIDTH);
    let mut picture = div().w_full().flex_none().overflow_hidden();
    // Height follows the width the row gives, at the picture's proportions.
    picture.style().aspect_ratio = Some(t.width / t.height);
    let picture = picture.child(img(t.path.clone()).size_full().object_fit(ObjectFit::Fill));
    let frame = ImageThumb::new(("md-image", n), gallery.images.clone(), start)
        .flex()
        .flex_col()
        .w_full()
        .max_h(px(THUMB_MAX_HEIGHT))
        .rounded(px(RADIUS_MD))
        .bg(BG_SIDEBAR)
        .overflow_hidden()
        .child(picture)
        .probe(format!("md-image-{n}"));
    let caption = (!t.alt.is_empty()).then(|| {
        truncate_block(t.alt.clone(), CAPTION.line)
            .type_style(CAPTION)
            .text_color(TEXT_MUTED)
            .debug_selector(move || format!("md-image-caption-{n}"))
    });
    col()
        .flex_none()
        .w(px(width))
        .max_w_full()
        .gap(px(SPACE_1))
        .child(frame)
        .children(caption)
        .into_any_element()
}
