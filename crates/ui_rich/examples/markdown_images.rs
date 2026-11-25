//! Pictures in Markdown: one text with a picture in running text, two in one paragraph, one
//! in a list item, a missing file, a remote image, a link to a picture and one in a table
//! cell, shown three times: in a wide column, a 400px panel and a 300px cell. The pictures
//! are written to a temp folder at start (a wide page, a small shot and a page much taller
//! than a thumbnail, with a red band at its top and a blue one at its bottom).
//!
//! Dev-driver probes: `md-wide`, `md-panel`, `md-cell` (the three columns), `md-image-<n>`
//! (thumbnails, numbered per view), and the viewer's own (`viewer-name`, `viewer-close`…).
//!
//! `cargo run -p ui_rich --example markdown_images`

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::markdown::{self, Markdown};

const TEXT: &str = r#"## Renders

Text and one picture: the home screen after the change ![Home after the change, rendered at 1440 × 900](renders/home.png) is what the reviewer sees first.

Two pictures in one paragraph: ![Before](renders/before.png) ![After, a very tall page](renders/tall.png)

- A list item with a picture ![Item shot](renders/before.png)
- A missing file stays a link: ![Missing render](renders/missing.png)

> A quote with a link to [the tall page](renders/tall.png) and a remote image ![Remote badge](https://example.com/badge.png).

| Screen | Picture |
|:--|:--|
| Home | ![Home](renders/home.png) |
| Tall | ![](renders/tall.png) |
"#;

struct Page {
    views: Vec<Entity<Markdown>>,
}

impl Page {
    fn new(dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let views = (0..3)
            .map(|_| {
                cx.new(|cx| {
                    Markdown::new(TEXT, cx)
                        .image_base(dir.clone())
                        .on_link(|url, _, _| eprintln!("on_link: {url}"))
                })
            })
            .collect();
        Self { views }
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let column = |label: &'static str, probe: &'static str, view: &Entity<Markdown>| {
            col()
                .gap(px(SPACE_3))
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(label))
                .child(view.clone())
                .probe(probe)
        };
        ui::root().child(
            div()
                .id("page")
                .size_full()
                .overflow_y_scroll()
                .child(
                    row()
                        .items_start()
                        .gap(px(SPACE_6))
                        .p(px(SPACE_6))
                        .child(div().flex_1().min_w_0().child(column("Wide", "md-wide", &self.views[0])))
                        .child(div().flex_none().w(px(400.)).child(column("400px panel", "md-panel", &self.views[1])))
                        .child(
                            div()
                                .flex_none()
                                .w(px(300.))
                                .p(px(SPACE_3))
                                .border_1()
                                .border_color(LINE)
                                .rounded(px(RADIUS_LG))
                                .child(column("300px cell", "md-cell", &self.views[2])),
                        ),
                ),
        )
    }
}

/// Write the pictures the text refers to into `dir/renders`.
fn write_pictures(dir: &Path) -> std::io::Result<()> {
    let renders = dir.join("renders");
    std::fs::create_dir_all(&renders)?;
    std::fs::write(renders.join("home.png"), png(1440, 900, [40, 90, 160]))?;
    std::fs::write(renders.join("before.png"), png(320, 200, [150, 110, 40]))?;
    std::fs::write(renders.join("tall.png"), png(360, 1600, [60, 140, 90]))?;
    Ok(())
}

// A tiny PNG encoder (RGB, stored deflate blocks), so the example needs no image crate.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A `w` × `h` picture in `tint` with a diagonal pattern, a red band at the top and a blue
/// one at the bottom (to see which part a cut thumbnail shows).
fn png(w: u32, h: u32, tint: [u8; 3]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let px = if y < 24 {
                [220, 60, 60]
            } else if y + 24 >= h {
                [60, 90, 220]
            } else if (x + y) / 24 % 2 == 0 {
                tint
            } else {
                tint.map(|c| c / 2)
            };
            raw.extend_from_slice(&px);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn main() {
    let dir = std::env::temp_dir().join("ui-rich-markdown-images");
    if let Err(e) = write_pictures(&dir) {
        eprintln!("could not write the pictures to {}: {e}", dir.display());
    }
    ui::run_app("Markdown images", 1440., 960., move |_, cx: &mut App| {
        markdown::bind_keys(cx);
        ui_rich::bind_keys(cx);
        cx.new(|cx| Page::new(dir, cx))
    });
}
