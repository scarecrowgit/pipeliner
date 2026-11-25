//! Images without a window: how the parser keeps them, which targets are local pictures, and
//! what the text units show once some of them are thumbnails.

use std::path::{Path, PathBuf};

use super::assert_well_formed;
use crate::markdown::image::{Gallery, Images, Probe, text_runs, thumbs};
use crate::markdown::parse::{Block, Inline, Style, parse};
use crate::markdown::rich::{first_paragraph, plain_text, units_with};
use crate::markdown::{Document, local_image};

fn image(alt: &str, target: &str) -> Inline {
    Inline {
        text: alt.into(),
        style: Style::default(),
        link: Some(target.into()),
        image: true,
    }
}

fn plain(s: &str) -> Inline {
    Inline {
        text: s.into(),
        ..Inline::default()
    }
}

fn paragraph(doc: &Document, ix: usize) -> &[Inline] {
    match &doc.blocks[ix] {
        Block::Paragraph(runs) => runs,
        other => panic!("block {ix} is {other:?}"),
    }
}

/// `/work` as the image folder, with these files read and decoded.
fn ready(files: &[&str]) -> Images {
    let mut images = Images {
        base: Some(PathBuf::from("/work")),
        ..Images::default()
    };
    for f in files {
        images.files.insert(
            Path::new("/work").join(f),
            Probe::Ready {
                width: 400.,
                height: 300.,
            },
        );
    }
    images
}

#[test]
fn an_image_is_a_run_of_its_own_with_its_alt_text() {
    let src = "See ![Before](renders/before.png) here.";
    let doc = parse(src);
    assert_well_formed(&doc, src);
    assert_eq!(
        paragraph(&doc, 0),
        [
            plain("See "),
            image("Before", "renders/before.png"),
            plain(" here.")
        ]
    );
}

#[test]
fn images_without_alt_and_side_by_side_stay_apart() {
    let src = "![](a.png)![b](a.png) ![c *d*\ne](b.png)";
    let doc = parse(src);
    assert_well_formed(&doc, src);
    assert_eq!(
        paragraph(&doc, 0),
        [
            image("", "a.png"),
            image("b", "a.png"),
            plain(" "),
            image("c d e", "b.png")
        ]
    );
    // An image alone, even without alt text, is a paragraph.
    assert_eq!(parse("![](x.png)").blocks, [Block::Paragraph(vec![image("", "x.png")])]);
}

#[test]
fn images_in_headings_cells_lists_and_bold_text() {
    let src = "# Title ![Shot](a.png)\n\n| A |\n|---|\n| ![](b.png) |\n\n- **![Bold](c.png)**\n";
    let doc = parse(src);
    assert_well_formed(&doc, src);
    let Block::Heading { text, .. } = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    assert_eq!(text[1], image("Shot", "a.png"));
    let Block::Table(t) = &doc.blocks[1] else {
        panic!("{doc:?}")
    };
    assert_eq!(t.rows[0][0], [image("", "b.png")]);
    let Block::List(l) = &doc.blocks[2] else {
        panic!("{doc:?}")
    };
    let Block::Paragraph(runs) = &l.items[0].blocks[0] else {
        panic!("{l:?}")
    };
    assert!(runs[0].image && runs[0].style.bold, "{runs:?}");
}

#[test]
fn plain_text_and_first_paragraph_keep_the_alt_text() {
    let src = "Look ![Before](a.png) and ![](b.png) here.";
    assert_eq!(plain_text(src), "Look Before and  here.");
    assert_eq!(first_paragraph(src), "Look Before and here.");
}

#[test]
fn every_prefix_of_a_text_with_images_parses() {
    let src = "Intro ![Before](renders/before.png) and\n\n- ![A](a.png)\n\n> ![Q](q.png) quote\n\n| x |\n|---|\n| ![](c.png) |\n";
    for end in (0..=src.len()).filter(|e| src.is_char_boundary(*e)) {
        assert_well_formed(&parse(&src[..end]), &src[..end]);
    }
}

#[test]
fn local_image_targets() {
    let base = Path::new("/work/tree");
    let some = |t: &str, b: Option<&Path>| local_image(t, b);
    assert_eq!(some("renders/before.png", Some(base)), Some("/work/tree/renders/before.png".into()));
    assert_eq!(some("./renders/a.PNG", Some(base)), Some("/work/tree/renders/a.PNG".into()));
    assert_eq!(some("../out/./a.jpeg", Some(base)), Some("/work/out/a.jpeg".into()));
    assert_eq!(some("renders/my%20shot.webp", Some(base)), Some("/work/tree/renders/my shot.webp".into()));
    assert_eq!(some("/abs/x.gif", None), Some("/abs/x.gif".into()));
    assert_eq!(some("file:///abs/my%20x.bmp", None), Some("/abs/my x.bmp".into()));
    assert_eq!(some("file://localhost/abs/x.jpg", None), Some("/abs/x.jpg".into()));
    assert_eq!(some("FILE:///abs/x.png", None), Some("/abs/x.png".into()));
    let home = base::paths::Paths::from_env().home;
    assert_eq!(some("~/shots/x.png", None), Some(home.join("shots/x.png")));
    // Not local pictures.
    for t in [
        "renders/before.png",
        "https://example.com/a.png",
        "http://example.com/a.png",
        "data:image/png;base64,iVBORw0KGgo=",
        "mailto:a@b.png",
        "file://host/abs/x.png",
        "",
    ] {
        assert_eq!(some(t, None), None, "{t:?}");
    }
    for t in ["notes.txt", "x.svg", "folder/", "noext", "https://x/a.png", "a.png.zip"] {
        assert_eq!(some(t, Some(base)), None, "{t:?}");
    }
}

#[test]
fn a_thumbnail_leaves_the_running_text() {
    let doc = parse("Look ![Before](a.png) here.\n\n![A](a.png) ![B](b.png)\n\nText\n![Missing](m.png)\n![After](b.png)");
    let images = ready(&["a.png", "b.png"]);
    let us = units_with(&doc, &[], &images);
    let texts: Vec<&str> = us.iter().map(|u| u.text.text.as_str()).collect();
    // The second paragraph is thumbnails only: no unit.
    assert_eq!(texts, ["Look here.", "Text\nMissing"]);
    assert_eq!(us[1].joiner, "\n\n");
    // The missing file stays a link.
    assert_eq!(us[1].text.links, [(5..12, "m.png".to_string())]);
    let t = thumbs(paragraph(&doc, 0), &images);
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].path.as_path(), t[0].alt.as_str()), (Path::new("/work/a.png"), "Before"));
    assert_eq!(thumbs(paragraph(&doc, 1), &images).len(), 2);
    // Nothing known yet: every image is link text, as before.
    let none = units_with(&doc, &[], &Images::default());
    assert_eq!(none[0].text.text, "Look Before here.");
    assert_eq!(none[1].text.text, "A B");
}

#[test]
fn headings_and_cells_keep_images_as_link_text() {
    let doc = parse("# Title ![Shot](a.png)\n\n| A |\n|---|\n| ![](b.png) |");
    let us = units_with(&doc, &[], &ready(&["a.png", "b.png"]));
    let texts: Vec<&str> = us.iter().map(|u| u.text.text.as_str()).collect();
    assert_eq!(texts, ["Title Shot", "A", "b.png"]);
    assert_eq!(us[2].text.links, [(0..5, "b.png".to_string())]);
}

#[test]
fn a_list_item_of_thumbnails_passes_its_marker_on() {
    let doc = parse("- ![A](a.png)\n\n  after\n- next");
    let us = units_with(&doc, &[], &ready(&["a.png"]));
    let got: Vec<(&str, &str)> = us
        .iter()
        .map(|u| (u.joiner.as_str(), u.text.text.as_str()))
        .collect();
    assert_eq!(got, [("\n\n- ", "after"), ("\n- ", "next")]);
}

#[test]
fn text_runs_borrow_a_paragraph_without_images() {
    let doc = parse("Plain **text**.");
    assert!(matches!(
        text_runs(paragraph(&doc, 0), &ready(&[]), true),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn the_gallery_lists_each_readable_picture_once_in_source_order() {
    let doc = parse(
        "# ![H](c.png)\n\n![A](a.png) [the b shot](b.png) ![again](a.png) ![gone](m.png) [web](https://x/a.png)",
    );
    let g = Gallery::build(&doc, &ready(&["a.png", "b.png", "c.png"]));
    let names: Vec<(&str, Option<&str>)> = g
        .images
        .iter()
        .map(|i| (i.name.as_ref(), i.caption.as_ref().map(|c| c.as_ref())))
        .collect();
    assert_eq!(
        names,
        [
            ("c.png", Some("H")),
            ("a.png", Some("A")),
            ("b.png", Some("the b shot"))
        ]
    );
    assert_eq!(g.index_of_target("b.png"), Some(2));
    assert_eq!(g.index_of_target("m.png"), None);
    assert_eq!(g.index_of_target("https://x/a.png"), None);
}
