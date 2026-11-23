//! The dev-driver line protocol: parsing, formatting, quoting and replies.

use devtools::driver::{parse_probes, parse_windows};
use ui::dev::proto::{Command, Target, format_reply, json_string, parse_reply, quote, split};

#[test]
fn parses_every_command() {
    let cases = [
        ("windows", Command::Windows),
        ("probes", Command::Probes),
        ("wait", Command::Wait),
        ("quit", Command::Quit),
        ("click increment", Command::Click(Target::Probe("increment".into()))),
        ("click 10 20.5", Command::Click(Target::Point(10., 20.5))),
        ("hover \"row 3\"", Command::Hover(Target::Probe("row 3".into()))),
        ("scroll list 0 -40", Command::Scroll(Target::Probe("list".into()), 0., -40.)),
        ("scroll 5 6 1 2", Command::Scroll(Target::Point(5., 6.), 1., 2.)),
        ("dblclick row", Command::DblClick(Target::Probe("row".into()))),
        ("drag knob track", Command::Drag(Target::Probe("knob".into()), Target::Probe("track".into()))),
        ("drag knob 300 20", Command::Drag(Target::Probe("knob".into()), Target::Point(300., 20.))),
        ("drag 1 2 track", Command::Drag(Target::Point(1., 2.), Target::Probe("track".into()))),
        ("drag 1 2 3 4", Command::Drag(Target::Point(1., 2.), Target::Point(3., 4.))),
        ("key cmd-k escape", Command::Key(vec!["cmd-k".into(), "escape".into()])),
        ("type \"hello world\"", Command::Type("hello world".into())),
        ("type x", Command::Type("x".into())),
        ("text count", Command::Text("count".into())),
        ("action ui::Quit", Command::Action("ui::Quit".into())),
        ("  click   a  ", Command::Click(Target::Probe("a".into()))),
    ];
    for (line, want) in cases {
        assert_eq!(Command::parse(line), Ok(want.clone()), "{line}");
        assert_eq!(Command::parse(&want.to_line()), Ok(want), "round trip of {line}");
    }
}

#[test]
fn rejects_bad_commands() {
    for line in [
        "",
        "dance",
        "windows now",
        "click",
        "click 1 2 3",
        "scroll list 1",
        "scroll list a b",
        "key",
        "type hello world",
        "type \"open",
        "text",
        "drag knob",
        "drag 1 2 3 4 5",
        "dblclick",
    ] {
        assert!(Command::parse(line).is_err(), "{line:?} should not parse");
    }
}

#[test]
fn quoting_round_trips_spaces_newlines_and_escapes() {
    for word in ["plain", "two words", "line\nbreak", "tab\there", "say \"hi\"", "back\\slash", "", " "] {
        let quoted = quote(word);
        assert!(!quoted.contains('\n'), "{quoted:?} must stay on one line");
        assert_eq!(split(&quoted), Ok(vec![word.to_string()]), "{word:?} via {quoted:?}");
    }
    assert_eq!(quote("plain"), "plain");
    assert_eq!(quote("a b"), "\"a b\"");
    let typed = Command::Type("first line\nsecond \"quoted\"".into());
    let line = typed.to_line();
    assert!(!line.contains('\n'));
    assert_eq!(Command::parse(&line), Ok(typed));
}

#[test]
fn split_reports_bad_escapes() {
    assert!(split("\"a\\q\"").is_err());
    assert!(split("\"unterminated").is_err());
    assert_eq!(split("a \"b c\" d"), Ok(vec!["a".into(), "b c".into(), "d".into()]));
}

#[test]
fn json_strings_are_valid_json() {
    for s in ["plain", "quote \" and \\", "new\nline\ttab", "\u{1}control", "ünïcødé"] {
        let parsed: String = serde_json::from_str(&json_string(s)).expect("valid JSON");
        assert_eq!(parsed, s);
    }
}

#[test]
fn replies_format_and_parse() {
    assert_eq!(format_reply(&Ok("[1,2]".into())), "ok [1,2]");
    assert_eq!(format_reply(&Err("two\nlines".into())), "err two lines");
    assert_eq!(parse_reply("ok [1,2]\n"), Ok("[1,2]".into()));
    assert_eq!(parse_reply("ok"), Ok(String::new()));
    assert_eq!(parse_reply("err no probe named x"), Err("no probe named x".into()));
    assert!(parse_reply("hello").is_err());
}

#[test]
fn keystrokes_for_typed_characters() {
    let a = ui::dev::char_keystroke('A');
    assert_eq!((a.key.as_str(), a.modifiers.shift, a.key_char.as_deref()), ("a", true, Some("A")));
    let space = ui::dev::char_keystroke(' ');
    assert_eq!((space.key.as_str(), space.key_char.as_deref()), ("space", Some(" ")));
    let enter = ui::dev::char_keystroke('\n');
    assert_eq!(enter.key, "enter");
}

#[test]
fn reads_window_and_probe_payloads() {
    let windows = serde_json::json!([
        {"id": 1, "x": 0, "y": 0, "width": 1440, "height": 900, "viewport": [1440, 900],
         "scale": 1, "active": true}
    ]);
    let windows = parse_windows(&windows).expect("windows");
    assert_eq!((windows[0].id, windows[0].width, windows[0].active), (1, 1440., true));

    let probes = serde_json::json!([
        {"name": "count", "window": 1, "x": 24, "y": 70, "width": 100, "height": 20,
         "visible": true, "value": "3"},
        {"name": "increment", "window": 1, "x": 24, "y": 24, "width": 160, "height": 36,
         "visible": false, "value": null}
    ]);
    let probes = parse_probes(&probes).expect("probes");
    assert_eq!(probes[0].value.as_deref(), Some("3"));
    assert!(!probes[1].visible && probes[1].value.is_none());
}
