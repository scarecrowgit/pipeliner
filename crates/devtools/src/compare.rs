//! Put a mockup render next to a window screenshot, for a human to look at: a side-by-side
//! PNG, a heat map of per-pixel differences and a coarse similarity number. Not a pass/fail
//! gate: Firefox and GPUI rasterise text differently.

use std::path::{Path, PathBuf};

use anyhow::Context;
use image::{Rgba, RgbaImage};

/// Width of the divider between the two images in the side-by-side PNG.
pub const DIVIDER: u32 = 4;

/// A channel difference at or below this counts as "the same pixel" (antialiasing noise).
pub const TOLERANCE: u8 = 24;

const DIVIDER_COLOR: Rgba<u8> = Rgba([255, 0, 170, 255]);
const EMPTY_COLOR: Rgba<u8> = Rgba([24, 24, 28, 255]);

/// How alike two images are, over the union of their areas (missing pixels differ fully).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    /// Share of pixels whose largest channel difference is within [`TOLERANCE`], 0–1.
    pub score: f64,
    /// Mean of the largest channel difference per pixel, 0–255.
    pub mean_diff: f64,
    pub width: u32,
    pub height: u32,
}

/// The three outputs of a comparison.
pub struct Comparison {
    pub side_by_side: RgbaImage,
    pub heat: RgbaImage,
    pub similarity: Similarity,
}

/// Mockup left, window right, both 1:1, a thin divider between them.
pub fn side_by_side(left: &RgbaImage, right: &RgbaImage) -> RgbaImage {
    let width = left.width() + DIVIDER + right.width();
    let height = left.height().max(right.height());
    let mut out = RgbaImage::from_pixel(width, height, EMPTY_COLOR);
    for y in 0..height {
        for x in left.width()..left.width() + DIVIDER {
            out.put_pixel(x, y, DIVIDER_COLOR);
        }
    }
    blit(&mut out, left, 0);
    blit(&mut out, right, left.width() + DIVIDER);
    out
}

fn blit(out: &mut RgbaImage, img: &RgbaImage, dx: u32) {
    for (x, y, p) in img.enumerate_pixels() {
        out.put_pixel(x + dx, y, opaque(*p));
    }
}

/// Composite over black, so transparent screenshot corners compare as dark.
fn opaque(p: Rgba<u8>) -> Rgba<u8> {
    let a = p[3] as u16;
    let c = |v: u8| ((v as u16 * a) / 255) as u8;
    Rgba([c(p[0]), c(p[1]), c(p[2]), 255])
}

/// The largest channel difference of two pixels; a missing pixel differs by 255.
fn diff(a: Option<&Rgba<u8>>, b: Option<&Rgba<u8>>) -> u8 {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (a, b) = (opaque(*a), opaque(*b));
            (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
        }
        _ => 255,
    }
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> Option<&Rgba<u8>> {
    (x < img.width() && y < img.height()).then(|| img.get_pixel(x, y))
}

/// Heat map over the union of both images: the window dimmed to grey, differences in red
/// (brighter = larger), and the [`Similarity`].
pub fn heat_map(mockup: &RgbaImage, window: &RgbaImage) -> (RgbaImage, Similarity) {
    let width = mockup.width().max(window.width());
    let height = mockup.height().max(window.height());
    let mut out = RgbaImage::new(width, height);
    let (mut same, mut total) = (0u64, 0u64);
    for y in 0..height {
        for x in 0..width {
            let (a, b) = (pixel(mockup, x, y), pixel(window, x, y));
            let d = diff(a, b);
            total += d as u64;
            if d <= TOLERANCE {
                same += 1;
            }
            let base = b.or(a).map_or(0., |p| {
                let p = opaque(*p);
                (0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32) * 0.35
            });
            let t = if d <= TOLERANCE { 0. } else { d as f32 / 255. };
            let red = base.max(255. * t.sqrt());
            let rest = base * (1. - t);
            out.put_pixel(x, y, Rgba([red as u8, rest as u8, rest as u8, 255]));
        }
    }
    let n = (width as u64 * height as u64).max(1) as f64;
    let similarity = Similarity {
        score: same as f64 / n,
        mean_diff: total as f64 / n,
        width,
        height,
    };
    (out, similarity)
}

/// Compare two images in memory.
pub fn compare(mockup: &RgbaImage, window: &RgbaImage) -> Comparison {
    let (heat, similarity) = heat_map(mockup, window);
    Comparison { side_by_side: side_by_side(mockup, window), heat, similarity }
}

/// Compare two PNG files; writes `<prefix>-side.png` and `<prefix>-heat.png` and returns
/// their paths with the similarity.
pub fn compare_files(
    mockup: &Path,
    window: &Path,
    prefix: &Path,
) -> anyhow::Result<(Similarity, PathBuf, PathBuf)> {
    let load = |p: &Path| -> anyhow::Result<RgbaImage> {
        Ok(image::open(p).with_context(|| format!("open {}", p.display()))?.to_rgba8())
    };
    let result = compare(&load(mockup)?, &load(window)?);
    let side = with_suffix(prefix, "-side.png");
    let heat = with_suffix(prefix, "-heat.png");
    if let Some(dir) = side.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    result.side_by_side.save(&side).with_context(|| format!("write {}", side.display()))?;
    result.heat.save(&heat).with_context(|| format!("write {}", heat.display()))?;
    Ok((result.similarity, side, heat))
}

fn with_suffix(prefix: &Path, suffix: &str) -> PathBuf {
    let mut s = prefix.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}
