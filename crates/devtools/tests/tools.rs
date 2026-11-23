//! The window-free tools: mockup sizes, image comparison, niri JSON and driver scripts.

use std::path::PathBuf;
use std::time::Duration;

use devtools::compare::{self, DIVIDER};
use devtools::mockup::{render_size, resolve};
use devtools::niri::parse_windows;
use devtools::script::{self, Step};
use image::{Rgba, RgbaImage};
use ui::dev::proto::{Command, Target};

#[test]
fn render_size_comment() {
    let head = "<head><meta charset=\"utf-8\"><!-- render size: 1440x960 -->\n<title>";
    assert_eq!(render_size(head), Some((1440, 960)));
    assert_eq!(render_size("<!-- render size: 560x640 -->"), Some((560, 640)));
    assert_eq!(render_size("<!--render size:800x600-->"), Some((800, 600)));
    assert_eq!(render_size("<head>"), None);
    assert_eq!(render_size("<!-- render size: 10x10 -->"), None);
    assert_eq!(render_size("<!-- render size: wide -->"), None);
}

#[test]
fn mockup_names_resolve_to_the_design_folder() {
    assert!(resolve("home").ends_with("screens-v2/home.html"));
    assert_eq!(resolve("./x.html"), PathBuf::from("./x.html"));
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(c))
}

#[test]
fn identical_images_are_fully_similar() {
    let a = solid(20, 10, [10, 20, 30, 255]);
    let result = compare::compare(&a, &a);
    assert_eq!(result.similarity.score, 1.0);
    assert_eq!(result.similarity.mean_diff, 0.0);
    assert_eq!(result.side_by_side.dimensions(), (40 + DIVIDER, 10));
    assert_eq!(result.heat.dimensions(), (20, 10));
    // No difference: the heat map is the dimmed image, not red.
    let p = result.heat.get_pixel(5, 5);
    assert!(p[0] < 30 && p[0] == p[1]);
}

#[test]
fn differences_show_up_red_and_lower_the_score() {
    let a = solid(10, 10, [0, 0, 0, 255]);
    let mut b = a.clone();
    for x in 0..5 {
        for y in 0..10 {
            b.put_pixel(x, y, Rgba([255, 255, 255, 255]));
        }
    }
    let result = compare::compare(&a, &b);
    assert!((result.similarity.score - 0.5).abs() < 1e-9);
    assert!((result.similarity.mean_diff - 127.5).abs() < 1e-9);
    let hot = result.heat.get_pixel(0, 0);
    assert!(hot[0] > 200 && hot[1] < 100, "{hot:?}");
    // Small antialiasing-sized differences stay within tolerance.
    let c = solid(10, 10, [10, 10, 10, 255]);
    assert_eq!(compare::compare(&a, &c).similarity.score, 1.0);
}

#[test]
fn size_mismatch_counts_as_difference() {
    let a = solid(10, 10, [50, 50, 50, 255]);
    let b = solid(10, 5, [50, 50, 50, 255]);
    let result = compare::compare(&a, &b);
    assert_eq!(result.similarity.score, 0.5);
    assert_eq!((result.similarity.width, result.similarity.height), (10, 10));
    // Side by side: mockup left, divider, window right, padded to the taller one.
    let side = &result.side_by_side;
    assert_eq!(side.dimensions(), (20 + DIVIDER, 10));
    assert_eq!(side.get_pixel(10, 0), &Rgba([255, 0, 170, 255]));
    assert_eq!(side.get_pixel(10 + DIVIDER, 0), &Rgba([50, 50, 50, 255]));
    assert_ne!(side.get_pixel(10 + DIVIDER, 8), &Rgba([50, 50, 50, 255]));
}

#[test]
fn compare_files_writes_both_pngs() {
    let dir = std::env::temp_dir().join(format!("devtools-compare-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (left, right) = (dir.join("a.png"), dir.join("b.png"));
    solid(8, 8, [0, 0, 0, 255]).save(&left).expect("save");
    solid(8, 8, [0, 0, 255, 255]).save(&right).expect("save");
    let (sim, side, heat) =
        compare::compare_files(&left, &right, &dir.join("out")).expect("compare");
    assert_eq!(sim.score, 0.0);
    assert!(side.ends_with("out-side.png") && heat.ends_with("out-heat.png"));
    assert_eq!(image::open(&side).expect("side").width(), 16 + DIVIDER);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

const NIRI_SAMPLE: &str = r#"[
 {"id":216,"title":"zsh","app_id":"kitty","pid":1676879,"workspace_id":6,"is_focused":false,
  "is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[4,1],
  "tile_size":[910.0,965.0],"window_size":[908,935],"tile_pos_in_workspace_view":null,
  "window_offset_in_tile":[1.0,29.0],"pos_in_canvas":null,"is_pinned":false},
  "focus_timestamp":{"secs":453759,"nanos":717656716},"hidden_from_workspace_id":null},
 {"id":301,"title":"Driver demo","app_id":"harness-demo","pid":4242,"workspace_id":6,
  "is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"window_size":[1440,935]}},
 {"id":302,"title":null,"app_id":null,"pid":null,"is_focused":false}
]"#;

#[test]
fn niri_windows_parse() {
    let windows = parse_windows(NIRI_SAMPLE).expect("parse");
    assert_eq!(windows.len(), 3);
    let demo = windows.iter().find(|w| w.pid == Some(4242)).expect("demo window");
    assert_eq!(demo.id, 301);
    assert_eq!(demo.app_id.as_deref(), Some("harness-demo"));
    assert_eq!(demo.size, Some((1440, 935)));
    assert!(demo.focused);
    assert_eq!(windows[0].size, Some((908, 935)));
    assert_eq!((windows[2].pid, windows[2].size), (None, None));
    assert!(parse_windows("{}").is_err());
    assert!(parse_windows("[{\"title\":\"no id\"}]").is_err());
}

#[test]
fn scripts_parse_with_shots_and_comments() {
    let text = "\
# open the counter
click increment
wait

type \"hello world\"
shot /tmp/x.png
sleep 250
key cmd-k escape
";
    let steps = script::parse(text).expect("script");
    assert_eq!(
        steps,
        vec![
            Step::Command(Command::Click(Target::Probe("increment".into()))),
            Step::Command(Command::Wait),
            Step::Command(Command::Type("hello world".into())),
            Step::Shot(PathBuf::from("/tmp/x.png")),
            Step::Sleep(Duration::from_millis(250)),
            Step::Command(Command::Key(vec!["cmd-k".into(), "escape".into()])),
        ]
    );
}

#[test]
fn script_errors_name_the_line() {
    let err = script::parse("wait\nfly away\n").expect_err("bad line");
    assert!(err.to_string().starts_with("line 2:"), "{err}");
    assert!(script::parse("shot").is_err());
    assert!(script::parse("sleep soon").is_err());
}
