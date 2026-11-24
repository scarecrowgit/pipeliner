//! The renders card: thumbnails sized for tall pages, and the image viewer they open.

use gpui::{Focusable as _, TestAppContext, px, size};
use hub::InboxFocus;
use hub::api::{BlobId, Render, ReviewId};
use ui_rich::viewer::is_image_viewer_open;

use super::{REVIEW, click, keys, open_with, shown};
use crate::shots::{Picture, SHOT_HEIGHT, Screen, pictures, thumb_size, viewer_images};

/// A 4 × 6 PNG.
const PNG: [u8; 73] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 4, 0, 0, 0, 6, 8, 2, 0,
    0, 0, 107, 91, 168, 34, 0, 0, 0, 16, 73, 68, 65, 84, 120, 156, 99, 168, 232, 57, 1, 71, 12,
    148, 114, 0, 139, 8, 43, 33, 28, 224, 180, 57, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn shot(path: &str, before: Option<&str>, after: &str, width: u32, height: u32) -> Render {
    Render {
        path: path.to_string(),
        before: before.map(BlobId::new),
        after: BlobId::new(after),
        width,
        height,
    }
}

#[test]
fn a_tall_page_shows_its_top_at_the_thumbnail_width() {
    let tall = shot("home.html", None, "a", 1440, 4800);
    let size = thumb_size(&tall, 400.);
    assert_eq!(size.width, 400.);
    assert_eq!(size.height, SHOT_HEIGHT, "cut at the thumbnail height");
    let drawn = size.image_height.unwrap_or_default();
    assert!(
        (drawn - 398. * 4800. / 1440.).abs() < 0.01,
        "the image keeps its ratio: {drawn}"
    );

    let wide = shot("home.html", None, "a", 1440, 900);
    let size = thumb_size(&wide, 300.);
    assert_eq!(
        (size.width, size.height),
        (300., 298. * 900. / 1440. + 2.),
        "a short page whole"
    );

    let small = shot("icon.html", None, "a", 64, 64);
    assert_eq!(thumb_size(&small, 400.).width, 66., "never enlarged");

    let unknown = shot("x.html", None, "a", 0, 0);
    assert_eq!(thumb_size(&unknown, 400.).image_height, None);
}

#[test]
fn the_viewer_walks_every_picture_with_names_and_captions() {
    let screens = vec![
        Screen {
            shot: shot("mockups/home.html", Some("h0"), "h1", 1440, 4800),
            variant: Some(1),
        },
        Screen {
            shot: shot("mockups/cart.html", None, "c1", 0, 0),
            variant: None,
        },
    ];
    let pics = pictures(&screens);
    assert_eq!(
        pics,
        [
            Picture {
                screen: 0,
                before: true,
                blob: BlobId::new("h0")
            },
            Picture {
                screen: 0,
                before: false,
                blob: BlobId::new("h1")
            },
            Picture {
                screen: 1,
                before: false,
                blob: BlobId::new("c1")
            },
        ]
    );
    let images = viewer_images(&screens, &pics, |b| {
        std::path::PathBuf::from("/blobs").join(b.as_str())
    });
    let shown: Vec<(String, String, String)> = images
        .iter()
        .map(|i| {
            (
                i.name.to_string(),
                i.caption
                    .as_ref()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                i.path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                "mockups/home.html".into(),
                "Before · Variant A · 1440 × 4800".into(),
                "/blobs/h0".into()
            ),
            (
                "mockups/home.html".into(),
                "After · Variant A · 1440 × 4800".into(),
                "/blobs/h1".into()
            ),
            (
                "mockups/cart.html".into(),
                "After".into(),
                "/blobs/c1".into()
            ),
        ]
    );
}

#[gpui::test]
async fn a_thumbnail_opens_the_viewer_at_its_picture_and_esc_comes_back(cx: &mut TestAppContext) {
    // The "after" picture exists; the "before" one and the second screen do not.
    let dir = std::env::temp_dir().join("harness-fixture-blobs");
    std::fs::create_dir_all(&dir).ok();
    let name = format!("screen-inbox-test-{}.png", std::process::id());
    std::fs::write(dir.join(&name), PNG).ok();
    let (t, cx) = open_with(
        cx,
        Some(InboxFocus::Review(ReviewId::new(REVIEW))),
        |world| {
            world.reviews[0].variants[0].renders = vec![
                shot("mockups/home.html", Some("missing-before.png"), &name, 4, 6),
                shot("mockups/cart.html", None, "missing-cart.png", 4, 6),
            ];
        },
    );
    cx.simulate_resize(size(px(1440.), px(2600.)));
    cx.run_until_parked();
    assert!(shown(cx, "review-renders"));
    let focused = |cx: &mut gpui::VisualTestContext| {
        t.view
            .update_in(cx, |v, window, cx| v.focus_handle(cx).is_focused(window))
    };
    assert!(focused(cx));

    click(cx, "review-shot-1");
    assert!(cx.update(|window, cx| is_image_viewer_open(window, cx)));
    assert!(!focused(cx), "the viewer has the keys");
    assert!(
        !shown(cx, "viewer-failed"),
        "opened at the clicked picture, which loads"
    );
    keys(cx, "left");
    assert!(
        shown(cx, "viewer-failed"),
        "Left goes to the missing \"before\" picture"
    );

    keys(cx, "escape");
    assert!(!cx.update(|window, cx| is_image_viewer_open(window, cx)));
    assert!(focused(cx), "the keys are back on the inbox");
    keys(cx, "2");
    let variant = t
        .view
        .read_with(cx, |v, _| v.review.as_ref().and_then(|r| r.variant));
    assert_eq!(variant, Some(2), "the inbox's keys work again");
    std::fs::remove_file(dir.join(&name)).ok();
}

#[test]
fn local_image_links_are_found_in_markdown_and_open_as_one_set() {
    use crate::links::{image_set, local_image, markdown_links};
    let text = "See ![home](/tmp/a/home.png) and [the run](https://x.dev/r) and \
                ![cart](<file:///tmp/a/cart.JPG> \"Cart\") or [notes](notes.png).";
    assert_eq!(
        markdown_links(text),
        [
            "/tmp/a/home.png",
            "https://x.dev/r",
            "file:///tmp/a/cart.JPG",
            "notes.png"
        ]
    );
    assert!(local_image("/tmp/a/home.png").is_some());
    assert!(local_image("file:///tmp/a/cart.JPG").is_some());
    assert!(
        local_image("https://x.dev/i.png").is_none(),
        "not on this machine"
    );
    assert!(
        local_image("notes.png").is_none(),
        "relative to an unknown folder"
    );
    assert!(local_image("/tmp/a/report.pdf").is_none());
    let links: Vec<(String, String)> = markdown_links(text)
        .into_iter()
        .map(|u| (String::new(), u))
        .collect();
    let (images, start) = image_set(&links, "file:///tmp/a/cart.JPG");
    assert_eq!(images.len(), 2);
    assert_eq!(start, 1);
    assert_eq!(images[1].name.as_ref(), "cart.JPG");
}

#[gpui::test]
async fn an_image_link_of_an_item_opens_the_viewer(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(
        cx,
        Some(InboxFocus::Item(hub::api::InboxId::new("inbox-1419"))),
        |world| {
            if let Some(item) = world
                .inbox
                .iter_mut()
                .find(|i| i.id.as_str() == "inbox-1419")
            {
                item.links.push(hub::api::Link {
                    label: "home.png".to_string(),
                    url: "/nonexistent/shots/home.png".to_string(),
                });
            }
        },
    );
    click(cx, "item-link-1");
    assert!(cx.update(|window, cx| is_image_viewer_open(window, cx)));
    keys(cx, "escape");
    assert!(!cx.update(|window, cx| is_image_viewer_open(window, cx)));
}

#[gpui::test]
async fn the_end_of_a_long_review_can_be_scrolled_to(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(
        cx,
        Some(InboxFocus::Review(ReviewId::new(REVIEW))),
        |world| {
            world.reviews[0].variants[0].renders = (0..4)
                .map(|ix| {
                    shot(
                        &format!("mockups/page-{ix}.html"),
                        None,
                        "missing.png",
                        1440,
                        900,
                    )
                })
                .collect();
        },
    );
    let scroll = cx
        .debug_bounds("review-scroll")
        .expect("the column is drawn");
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-20_000.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    let scroll = cx
        .debug_bounds("review-scroll")
        .expect("the column is drawn");
    let renders = cx
        .debug_bounds("review-renders")
        .expect("the renders are drawn");
    assert!(
        renders.bottom() <= scroll.bottom() - px(23.),
        "the card ends at {:?}, the column at {:?}: its padding shows under it",
        renders.bottom(),
        scroll.bottom()
    );
}
