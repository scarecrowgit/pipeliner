//! Images in a test window: thumbnails drawn from real PNG files in a temp folder, the viewer
//! they open, and the text around them.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyView, Bounds, Context, Entity, IntoElement, Modifiers, MouseButton, Pixels, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use ui::OverlayHost;

use crate::markdown::element::unit_point;
use crate::markdown::image::Probe;
use crate::markdown::{Markdown, bind_keys};
use crate::viewer::tests::png;
use crate::viewer::{close_image_viewer, image_viewer_current, is_image_viewer_open};

/// A folder under the system temp dir, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ui-rich-md-images-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("renders")).unwrap_or_else(|e| panic!("{e}"));
        Self(dir)
    }

    /// Write a `w` × `h` PNG at `rel`.
    fn png(&self, rel: &str, w: u32, h: u32) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::write(&path, png(w, h)).unwrap_or_else(|e| panic!("{e}"));
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Host {
    view: AnyView,
    width: f32,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().child(
            div()
                .debug_selector(|| "md-host".into())
                .w(px(self.width))
                .child(self.view.clone()),
        )
    }
}

type Log = Rc<RefCell<Vec<String>>>;

/// A window with an overlay host showing `text` in a `width` px column, relative images
/// resolved against `base`; logs the links `on_link` gets.
fn open<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    base: Option<&Path>,
    width: f32,
) -> (Entity<Markdown>, Log, &'a mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        bind_keys(cx);
    });
    let links: Log = Rc::default();
    let l = links.clone();
    let slot: Rc<RefCell<Option<Entity<Markdown>>>> = Rc::default();
    let s = slot.clone();
    let (text, base) = (text.to_string(), base.map(Path::to_path_buf));
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let md = cx.new(|cx| {
            let md = Markdown::new(text, cx).on_link(move |url, _, _| l.borrow_mut().push(url.to_string()));
            match base {
                Some(b) => md.image_base(b),
                None => md,
            }
        });
        *s.borrow_mut() = Some(md.clone());
        let host = cx.new(|_| Host {
            view: md.into(),
            width,
        });
        OverlayHost::new(window, cx).with_content(host)
    });
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    cx.run_until_parked();
    // The files are read off the UI thread; draw once more with what they gave.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let Some(md) = slot.borrow_mut().take() else {
        panic!("view not built")
    };
    (md, links, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("`{selector}` was not laid out"))
}

fn click(cx: &mut VisualTestContext, at: gpui::Point<Pixels>) {
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

fn viewer(cx: &mut VisualTestContext) -> Option<(usize, String)> {
    cx.update(|window, cx| image_viewer_current(window, cx))
        .map(|c| (c.index, c.name.to_string()))
}

fn close(cx: &mut VisualTestContext) {
    cx.update(close_image_viewer);
    cx.run_until_parked();
}

fn unit_texts(md: &Entity<Markdown>, cx: &mut VisualTestContext) -> Vec<String> {
    md.read_with(cx, |m, _| m.units.iter().map(|u| u.text.text.clone()).collect())
}

fn point_of(md: &Entity<Markdown>, cx: &mut VisualTestContext, unit: usize, offset: usize) -> gpui::Point<Pixels> {
    md.read_with(cx, |m, _| unit_point(&m.shared.borrow(), unit, offset))
        .unwrap_or_else(|| panic!("unit {unit} offset {offset} not laid out"))
}

#[gpui::test]
fn a_relative_image_with_a_base_is_a_thumbnail_that_opens_the_viewer(cx: &mut TestAppContext) {
    let dir = TempDir::new("one");
    dir.png("renders/before.png", 320, 200);
    let (md, links, cx) = open(
        cx,
        "The page before the change ![Before](renders/before.png) and after it.",
        Some(&dir.0),
        640.,
    );
    assert_eq!(unit_texts(&md, cx), ["The page before the change and after it."]);
    let thumb = bounds(cx, "md-image-0");
    // Natural size plus the border, under the text.
    assert_eq!((thumb.size.width, thumb.size.height), (px(322.), px(202.)));
    let text = md.read_with(cx, |m, _| m.shared.borrow().frame[0].bounds);
    assert!(thumb.top() >= text.bottom(), "{thumb:?} under {text:?}");
    assert!(bounds(cx, "md-image-caption-0").top() >= thumb.bottom());
    click(cx, thumb.center());
    assert!(cx.update(|window, cx| is_image_viewer_open(window, cx)));
    assert_eq!(viewer(cx), Some((0, "before.png".to_string())));
    assert!(links.borrow().is_empty());
}

#[gpui::test]
fn the_viewer_has_every_local_picture_and_starts_at_the_clicked_one(cx: &mut TestAppContext) {
    let dir = TempDir::new("three");
    dir.png("one.png", 120, 80);
    dir.png("two.png", 80, 120);
    dir.png("three.png", 60, 60);
    let (md, links, cx) = open(
        cx,
        "![One](one.png) ![Two](two.png)\n\nThe third is [this shot](three.png), the docs are [here](https://example.com).",
        Some(&dir.0),
        640.,
    );
    let names: Vec<String> = md.read_with(cx, |m, _| {
        m.gallery.images.iter().map(|i| i.name.to_string()).collect()
    });
    assert_eq!(names, ["one.png", "two.png", "three.png"]);
    // A paragraph of images only has no text: the link paragraph is unit 0.
    assert_eq!(
        unit_texts(&md, cx),
        ["The third is this shot, the docs are here."]
    );
    // Side by side while they fit.
    let (a, b) = (bounds(cx, "md-image-0"), bounds(cx, "md-image-1"));
    assert_eq!(a.top(), b.top());
    assert!(b.left() > a.right());
    click(cx, b.center());
    assert_eq!(viewer(cx), Some((1, "two.png".to_string())));
    close(cx);
    let link = point_of(&md, cx, 0, "The third is th".len());
    click(cx, link);
    assert_eq!(viewer(cx), Some((2, "three.png".to_string())));
    close(cx);
    assert!(links.borrow().is_empty(), "{:?}", links.borrow());
    let web = point_of(&md, cx, 0, "The third is this shot, the docs are he".len());
    click(cx, web);
    assert_eq!(links.borrow().as_slice(), ["https://example.com"]);
    assert_eq!(viewer(cx), None);
}

#[gpui::test]
fn what_is_not_a_readable_local_picture_stays_link_text(cx: &mut TestAppContext) {
    let dir = TempDir::new("missing");
    std::fs::write(dir.0.join("broken.png"), b"not a png").unwrap_or_else(|e| panic!("{e}"));
    let (md, links, cx) = open(
        cx,
        "A ![Missing](missing.png) B ![Broken](broken.png) C ![Remote](https://example.com/a.png) D",
        Some(&dir.0),
        640.,
    );
    assert_eq!(unit_texts(&md, cx), ["A Missing B Broken C Remote D"]);
    md.read_with(cx, |m, _| {
        assert!(m.gallery.images.is_empty());
        assert_eq!(m.images.files.get(&dir.0.join("missing.png")), Some(&Probe::Failed));
        assert_eq!(m.images.files.get(&dir.0.join("broken.png")), Some(&Probe::Failed));
    });
    for (offset, url) in [(3, "missing.png"), (13, "broken.png"), (23, "https://example.com/a.png")] {
        let at = point_of(&md, cx, 0, offset);
        click(cx, at);
        assert_eq!(links.borrow().last().map(String::as_str), Some(url));
    }
    assert_eq!(viewer(cx), None);
}

#[gpui::test]
fn a_relative_image_without_a_base_is_link_text(cx: &mut TestAppContext) {
    let dir = TempDir::new("nobase");
    dir.png("renders/before.png", 40, 40);
    let (md, links, cx) = open(cx, "See ![Before](renders/before.png).", None, 640.);
    assert_eq!(unit_texts(&md, cx), ["See Before."]);
    let at = point_of(&md, cx, 0, 5);
    click(cx, at);
    assert_eq!(links.borrow().as_slice(), ["renders/before.png"]);
    // Given the folder later, it turns into a thumbnail.
    md.update(cx, |m, cx| m.set_image_base(Some(dir.0.clone()), cx));
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(unit_texts(&md, cx), ["See ."]);
    assert!(md.read_with(cx, |m, _| m.gallery.images.len()) == 1);
}

#[gpui::test]
fn nothing_leaves_a_narrow_view(cx: &mut TestAppContext) {
    let dir = TempDir::new("narrow");
    dir.png("wide.png", 800, 300);
    dir.png("tall.png", 100, 600);
    let text = "A long paragraph with words that wrap in a narrow column ![A very long caption that will never fit on one line of a narrow panel](wide.png) ![Tall](tall.png)\n\n- In a list ![Item](wide.png)\n\n> In a quote ![Quote](tall.png)";
    let (md, _, cx) = open(cx, text, Some(&dir.0), 300.);
    let host = bounds(cx, "md-host");
    for sel in [
        "md-image-0",
        "md-image-1",
        "md-image-2",
        "md-image-3",
        "md-image-caption-0",
        "md-image-caption-1",
        "md-image-caption-2",
        "md-image-caption-3",
    ] {
        let b = bounds(cx, sel);
        assert!(
            b.left() >= host.left() && b.right() <= host.right(),
            "{sel} {b:?} outside {host:?}"
        );
    }
    // The wide picture is as wide as the column, at its proportions.
    let wide = bounds(cx, "md-image-0");
    assert_eq!(wide.size.width, px(300.));
    let inner = 298. * 300. / 800.;
    assert!((f32::from(wide.size.height) - (inner + 2.)).abs() < 1., "{wide:?}");
    // The tall one is cut at 220 and keeps its width; it goes under the wide one.
    let tall = bounds(cx, "md-image-1");
    assert_eq!((tall.size.width, tall.size.height), (px(102.), px(220.)));
    assert!(tall.top() > wide.bottom());
    // The text above is measured at the column's width, not at a sliver.
    md.read_with(cx, |m, _| {
        let s = m.shared.borrow();
        let first = &s.frame[0];
        assert_eq!(first.bounds.size.width, px(300.));
        assert!(first.layout.height() < 100., "{:?}", first.bounds);
    });
}

#[gpui::test]
fn growing_text_keeps_its_thumbnails(cx: &mut TestAppContext) {
    let dir = TempDir::new("stream");
    dir.png("a.png", 50, 40);
    dir.png("b.png", 50, 40);
    let start = "Shot ![A](a.png) taken";
    let (md, _, cx) = open(cx, start, Some(&dir.0), 640.);
    assert_eq!(unit_texts(&md, cx), ["Shot taken"]);
    // At once, before anything is read again: no flash of link text.
    md.update(cx, |m, cx| m.set_text(format!("{start} today.\n\nAnd ![B](b.pn"), cx));
    assert_eq!(unit_texts(&md, cx), ["Shot taken today.", "And ![B](b.pn"]);
    md.update(cx, |m, cx| m.set_text(format!("{start} today.\n\nAnd ![B](b.png)"), cx));
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(unit_texts(&md, cx), ["Shot taken today.", "And"]);
    let (a, b) = (bounds(cx, "md-image-0"), bounds(cx, "md-image-1"));
    assert!(b.top() > a.bottom());
}

#[gpui::test]
fn a_selection_across_a_paragraph_with_a_picture_copies_its_text(cx: &mut TestAppContext) {
    let dir = TempDir::new("select");
    dir.png("a.png", 200, 100);
    let (md, _, cx) = open(cx, "Alpha ![A](a.png) beta\n\nGamma delta", Some(&dir.0), 640.);
    let from = point_of(&md, cx, 0, 0);
    let to = point_of(&md, cx, 1, "Gamma".len());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    assert_eq!(
        md.read_with(cx, |m, _| m.selected_text()).as_deref(),
        Some("Alpha beta\n\nGamma")
    );
    cx.simulate_keystrokes("ctrl-c");
    let clip = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("Alpha beta\n\nGamma"));
}

#[gpui::test]
fn headings_and_cells_show_link_text_that_opens_the_viewer(cx: &mut TestAppContext) {
    let dir = TempDir::new("cells");
    dir.png("a.png", 30, 30);
    dir.png("b.png", 30, 30);
    let (md, links, cx) = open(
        cx,
        "## Result ![Shot](a.png)\n\n| Picture |\n|---|\n| ![](b.png) |",
        Some(&dir.0),
        640.,
    );
    assert_eq!(unit_texts(&md, cx), ["Result Shot", "Picture", "b.png"]);
    let at = point_of(&md, cx, 2, 1);
    click(cx, at);
    assert_eq!(viewer(cx), Some((1, "b.png".to_string())));
    close(cx);
    let at = point_of(&md, cx, 0, "Result S".len());
    click(cx, at);
    assert_eq!(viewer(cx), Some((0, "a.png".to_string())));
    assert!(links.borrow().is_empty());
}
