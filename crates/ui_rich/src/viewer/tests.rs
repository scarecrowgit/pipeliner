//! The viewer in a test window: opened from a thumbnail, driven by simulated keys, clicks,
//! drags and wheel events through the window's dispatch.

use std::cell::RefCell;
use std::sync::Arc;

use gpui::{
    Context, Entity, FocusHandle, Image, ImageFormat, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Point, Render, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
    VisualTestContext, WeakEntity, Window, div, point, prelude::*, px, size,
};
use ui::OverlayHost;

use super::geometry::{MARGIN, MAX_SCALE, Mode, Viewport};
use super::view::{ImageViewer, TOOLBAR_H};
use super::{ImageThumb, ViewerImage, is_image_viewer_open};

thread_local! {
    /// The viewer the last `open_image_viewer` made.
    pub(super) static LAST: RefCell<Option<WeakEntity<ImageViewer>>> = const { RefCell::new(None) };
}

const WINDOW_W: f32 = 1000.;
const WINDOW_H: f32 = 748.;

struct Page {
    images: Vec<ViewerImage>,
    focus: FocusHandle,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .relative()
            .child(div().id("page-field").track_focus(&self.focus).size(px(20.)))
            .child(
                ImageThumb::new("thumb", self.images.clone(), 0)
                    .absolute()
                    .left(px(100.))
                    .top(px(100.))
                    .w(px(120.))
                    .h(px(80.)),
            )
    }
}

// A tiny PNG encoder (stored deflate blocks), so the tests need no image crate.
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

/// A `w` × `h` grey PNG with a gradient.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((w + 1) * h) as usize);
    for y in 0..h {
        raw.push(0);
        raw.extend((0..w).map(|x| ((x + y) % 256) as u8));
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
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn memory(name: &str, w: u32, h: u32) -> ViewerImage {
    ViewerImage::memory(Arc::new(Image::from_bytes(ImageFormat::Png, png(w, h))), name.to_string())
}

/// A tall page render, and two more.
fn pages() -> Vec<ViewerImage> {
    vec![memory("page.png", 800, 2400), memory("wide.png", 1200, 300), memory("small.png", 40, 30)]
}

fn setup(cx: &mut TestAppContext, images: Vec<ViewerImage>) -> (Entity<Page>, &mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        crate::bind_keys(cx);
    });
    let slot: RefCell<Option<Entity<Page>>> = RefCell::default();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|cx| Page { images, focus: cx.focus_handle() });
        *slot.borrow_mut() = Some(page.clone());
        OverlayHost::new(window, cx).with_content(page)
    });
    let Some(page) = slot.borrow_mut().take() else { panic!("page not built") };
    cx.simulate_resize(size(px(WINDOW_W), px(WINDOW_H)));
    cx.update(|window, cx| {
        window.activate_window();
        let focus = page.read(cx).focus.clone();
        window.focus(&focus);
    });
    cx.run_until_parked();
    (page, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

fn click_at(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_click(point(px(x), px(y)), Modifiers::none());
    cx.run_until_parked();
}

fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

fn open(cx: &mut VisualTestContext) -> Entity<ImageViewer> {
    click(cx, "thumb");
    // One more frame: the decoded image arrives, then the viewport is made.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(is_open(cx), "the viewer opened");
    let viewer = LAST.with(|l| l.borrow().clone()).and_then(|w| w.upgrade());
    viewer.unwrap_or_else(|| panic!("no viewer"))
}

fn is_open(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| is_image_viewer_open(window, cx))
}

fn vp(viewer: &Entity<ImageViewer>, cx: &mut VisualTestContext) -> Viewport {
    cx.update(|_, cx| viewer.read(cx).viewport).unwrap_or_else(|| panic!("the image did not load"))
}

/// A window point for a stage point.
fn at(x: f32, y: f32) -> Point<gpui::Pixels> {
    point(px(x), px(y + TOOLBAR_H))
}

fn wheel(cx: &mut VisualTestContext, x: f32, y: f32, dy: f32, modifiers: Modifiers) {
    cx.simulate_event(ScrollWheelEvent {
        position: at(x, y),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers,
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
}

fn page_focused(page: &Entity<Page>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| page.read(cx).focus.is_focused(window))
}

#[gpui::test]
fn a_thumbnail_opens_the_viewer_fitted_and_focused(cx: &mut TestAppContext) {
    let (page, cx) = setup(cx, pages());
    let viewer = open(cx);
    let v = vp(&viewer, cx);
    assert_eq!(v.mode, Mode::Fit);
    assert_eq!(v.stage.w, WINDOW_W);
    assert_eq!(v.stage.h, WINDOW_H - TOOLBAR_H);
    assert!((v.drawn().h - (v.stage.h - 2. * MARGIN)).abs() < 0.5, "the tall page fits its height");
    assert!(v.scale < 1.);
    assert!(cx.update(|window, cx| viewer.read(cx).focus.is_focused(window)), "the viewer has the keys");
    assert!(!page_focused(&page, cx));
}

#[gpui::test]
fn escape_closes_and_focus_returns(cx: &mut TestAppContext) {
    let (page, cx) = setup(cx, pages());
    open(cx);
    keys(cx, "escape");
    assert!(!is_open(cx));
    assert!(page_focused(&page, cx), "focus went back to the page");
}

#[gpui::test]
fn the_close_button_closes(cx: &mut TestAppContext) {
    let (page, cx) = setup(cx, pages());
    open(cx);
    click(cx, "viewer-close");
    assert!(!is_open(cx));
    assert!(page_focused(&page, cx));
}

#[gpui::test]
fn a_click_beside_the_image_closes_and_a_click_on_it_does_not(cx: &mut TestAppContext) {
    let (page, cx) = setup(cx, pages());
    let viewer = open(cx);
    let v = vp(&viewer, cx);
    let (mx, my) = (v.x + v.drawn().w / 2., v.y + v.drawn().h / 2.);
    let p = at(mx, my);
    click_at(cx, p.x.into(), p.y.into());
    assert!(is_open(cx), "a click on the image keeps it open");
    let p = at(20., 300.);
    click_at(cx, p.x.into(), p.y.into());
    assert!(!is_open(cx), "a click on the dark area closes");
    assert!(page_focused(&page, cx));
}

#[gpui::test]
fn keys_change_the_zoom(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, pages());
    let viewer = open(cx);
    keys(cx, "1");
    assert_eq!(vp(&viewer, cx).percent(), 100);
    keys(cx, "-");
    assert_eq!(vp(&viewer, cx).percent(), 75);
    keys(cx, "+");
    assert_eq!(vp(&viewer, cx).percent(), 100);
    // Shift-= as a keyboard sends it.
    keys(cx, "shift-=->+");
    assert_eq!(vp(&viewer, cx).percent(), 125);
    keys(cx, "=");
    assert_eq!(vp(&viewer, cx).percent(), 150);
    keys(cx, "0");
    assert_eq!(vp(&viewer, cx).mode, Mode::Fit);
    click(cx, "viewer-fit-width");
    let v = vp(&viewer, cx);
    assert_eq!(v.mode, Mode::FitWidth);
    assert!((v.drawn().w - (WINDOW_W - 2. * MARGIN)).abs() < 0.5);
    assert_eq!(v.y, MARGIN, "fit width shows the top of the page");
    click(cx, "viewer-actual");
    assert_eq!(vp(&viewer, cx).percent(), 100);
    click(cx, "viewer-zoom-in");
    assert_eq!(vp(&viewer, cx).percent(), 125);
    click(cx, "viewer-zoom-out");
    assert_eq!(vp(&viewer, cx).percent(), 100);
    click(cx, "viewer-fit");
    assert_eq!(vp(&viewer, cx).mode, Mode::Fit);
}

#[gpui::test]
fn zoom_is_clamped(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, pages());
    let viewer = open(cx);
    for _ in 0..30 {
        cx.simulate_keystrokes("+");
    }
    cx.run_until_parked();
    assert_eq!(vp(&viewer, cx).scale, MAX_SCALE);
    for _ in 0..40 {
        cx.simulate_keystrokes("-");
    }
    cx.run_until_parked();
    let v = vp(&viewer, cx);
    assert!((v.scale - v.min_scale()).abs() < 1e-5, "stops at the smallest zoom");
    for _ in 0..20 {
        wheel(cx, 500., 300., 400., Modifiers::control());
    }
    assert_eq!(vp(&viewer, cx).scale, MAX_SCALE, "Ctrl+wheel stops at the largest zoom too");
}

#[gpui::test]
fn ctrl_wheel_zooms_around_the_pointer_and_the_wheel_pans(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, pages());
    let viewer = open(cx);
    // 300%: wider and taller than the stage, so nothing is centred.
    keys(cx, "1 + + + +");
    let before = vp(&viewer, cx);
    assert_eq!(before.percent(), 300);
    let (ax, ay) = (300., 200.);
    let (ix, iy) = ((ax - before.x) / before.scale, (ay - before.y) / before.scale);
    wheel(cx, ax, ay, 100., Modifiers::control());
    let after = vp(&viewer, cx);
    assert!(after.scale > before.scale, "wheel up with Ctrl zooms in");
    assert!(((ay - after.y) / after.scale - iy).abs() < 0.5, "the point under the pointer stays");
    assert!(((ax - after.x) / after.scale - ix).abs() < 0.5);

    keys(cx, "1");
    let y0 = vp(&viewer, cx).y;
    wheel(cx, 500., 300., -120., Modifiers::none());
    assert_eq!(vp(&viewer, cx).y, y0 - 120., "a plain wheel scrolls the page");
    keys(cx, "down");
    assert!(vp(&viewer, cx).y < y0 - 120.);
}

#[gpui::test]
fn dragging_pans_within_limits_and_does_not_close(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, pages());
    let viewer = open(cx);
    keys(cx, "1");
    let v = vp(&viewer, cx);
    let x0 = v.x;
    // Start beside the image (it is 800 wide in a 1000 stage): a drag, not a click.
    cx.simulate_mouse_down(at(40., 300.), MouseButton::Left, Modifiers::none());
    for i in 1..=10 {
        cx.simulate_mouse_move(at(40., 300. - 500. * i as f32), MouseButton::Left, Modifiers::none());
    }
    cx.simulate_mouse_up(at(40., -4700.), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(is_open(cx), "a drag does not close the viewer");
    let v = vp(&viewer, cx);
    assert_eq!(v.y, v.stage.h - MARGIN - v.drawn().h, "the bottom edge stops inside the stage");
    assert_eq!(v.x, x0, "an axis the image fits on stays centred");
    // And back past the top.
    cx.simulate_mouse_down(at(500., 300.), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(at(500., 9000.), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(at(500., 9000.), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(vp(&viewer, cx).y, MARGIN);
}

#[gpui::test]
fn double_click_toggles_fit_and_actual(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, pages());
    let viewer = open(cx);
    let v = vp(&viewer, cx);
    let p = at(v.x + v.drawn().w / 2., v.y + 40.);
    let double = |cx: &mut VisualTestContext| {
        for click_count in [1, 2] {
            cx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: p,
                modifiers: Modifiers::none(),
                click_count,
                first_mouse: false,
            });
            cx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: p, modifiers: Modifiers::none(), click_count });
        }
        cx.run_until_parked();
    };
    double(cx);
    assert_eq!(vp(&viewer, cx).percent(), 100);
    assert!(is_open(cx));
    double(cx);
    assert_eq!(vp(&viewer, cx).mode, Mode::Fit);
}

#[gpui::test]
fn left_and_right_switch_images(cx: &mut TestAppContext) {
    let (_, cx) = setup(cx, pages());
    let viewer = open(cx);
    let index = |cx: &mut VisualTestContext| cx.update(|_, cx| viewer.read(cx).index);
    keys(cx, "left");
    assert_eq!(index(cx), 0, "no image before the first");
    keys(cx, "1");
    keys(cx, "right");
    assert_eq!(index(cx), 1);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(vp(&viewer, cx).mode, Mode::Fit, "a new image starts fitted");
    click(cx, "viewer-next");
    assert_eq!(index(cx), 2);
    keys(cx, "right");
    assert_eq!(index(cx), 2, "no image after the last");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(vp(&viewer, cx).scale, 1., "a small image is not enlarged by the fit");
    click(cx, "viewer-prev");
    assert_eq!(index(cx), 1);
}

#[gpui::test]
fn a_missing_file_says_so_with_its_path(cx: &mut TestAppContext) {
    let missing = std::env::temp_dir().join("ui-rich-viewer-tests").join("missing.png");
    let (page, cx) = setup(cx, vec![ViewerImage::file(&missing)]);
    let viewer = open(cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("viewer-failed").is_some(), "the failure is shown");
    let text = cx.update(|_, cx| viewer.read(cx).error_text(cx));
    assert!(text.is_some_and(|t| t.contains(&missing.display().to_string())), "with the path");
    assert!(cx.update(|_, cx| viewer.read(cx).viewport.is_none()));
    keys(cx, "escape");
    assert!(!is_open(cx));
    assert!(page_focused(&page, cx));
}

#[gpui::test]
fn undecodable_bytes_say_so(cx: &mut TestAppContext) {
    let broken = ViewerImage::memory(Arc::new(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3])), "Pasted image");
    let (_, cx) = setup(cx, vec![broken]);
    let viewer = open(cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let text = cx.update(|_, cx| viewer.read(cx).error_text(cx));
    assert!(text.is_some_and(|t| t.starts_with("Pasted image")));
}

#[gpui::test]
fn a_file_on_disk_opens(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("ui-rich-viewer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("shot.png");
    std::fs::write(&path, png(300, 200)).ok();
    let (_, cx) = setup(cx, vec![ViewerImage::file(&path).caption("After")]);
    let viewer = open(cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let v = vp(&viewer, cx);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(v.scale, 1., "it fits at 100%");
    assert!(cx.update(|_, cx| viewer.read(cx).images[0].name == "shot.png"));
}
