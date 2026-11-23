use super::*;

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|i| i.to_string()).collect())
}

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

fn fields(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
    entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn simple_header_and_body() {
    let doc = parse("---\nname: reviewer\ndescription: Reviews a diff\n---\n# Title\n\nText\n");
    assert_eq!(doc.str("name"), Some("reviewer"));
    assert_eq!(doc.str("description"), Some("Reviews a diff"));
    assert_eq!(doc.body, "# Title\n\nText\n");
    assert_eq!(doc.str("missing"), None);
}

#[test]
fn no_header_means_whole_text_is_body() {
    for text in ["", "just text\n", "# Title\n---\nnot a header\n", "\n---\na: b\n---\n"] {
        let doc = parse(text);
        assert!(doc.fields.is_empty(), "{text:?}");
        assert_eq!(doc.body, text);
    }
}

#[test]
fn unclosed_header_is_no_header() {
    let text = "---\nname: x\nbody that never closes\n";
    let doc = parse(text);
    assert!(doc.fields.is_empty());
    assert_eq!(doc.body, text);
    let doc = parse("---");
    assert!(doc.fields.is_empty());
    assert_eq!(doc.body, "---");
}

#[test]
fn empty_header_and_empty_body() {
    let doc = parse("---\n---\n");
    assert!(doc.fields.is_empty());
    assert_eq!(doc.body, "");
    let doc = parse("---\na: b\n---");
    assert_eq!(doc.str("a"), Some("b"));
    assert_eq!(doc.body, "");
}

#[test]
fn crlf_and_bom() {
    let doc = parse("\u{feff}---\r\nname: x\r\nlist: [a, b]\r\n---\r\nbody\r\n");
    assert_eq!(doc.str("name"), Some("x"));
    assert_eq!(doc.list("list"), vec!["a", "b"]);
    assert_eq!(doc.body, "body\r\n");
}

#[test]
fn closing_fence_inside_body_is_part_of_body() {
    let doc = parse("---\na: b\n---\ntext\n---\nmore\n");
    assert_eq!(doc.body, "text\n---\nmore\n");
}

#[test]
fn quotes_comments_and_colons() {
    let doc = parse(
        "---\n# a comment\nname: \"quoted: name\"\nother: 'it''s'\nurl: http://x.dev/a:b\nempty:\nesc: \"a\\nb \\\"c\\\"\"\n\n  # indented comment\nlast: v\n---\n",
    );
    assert_eq!(doc.str("name"), Some("quoted: name"));
    assert_eq!(doc.str("other"), Some("it's"));
    assert_eq!(doc.str("url"), Some("http://x.dev/a:b"));
    assert_eq!(doc.str("empty"), Some(""));
    assert_eq!(doc.str("esc"), Some("a\nb \"c\""));
    assert_eq!(doc.str("last"), Some("v"));
    assert_eq!(doc.fields.len(), 6);
}

#[test]
fn quoted_keys_and_odd_lines() {
    let doc = parse("---\n\"quoted key\": 1\nno colon here\n: no key\nk:v\nok: yes\n- stray\n---\n");
    assert_eq!(doc.str("quoted key"), Some("1"));
    assert_eq!(doc.str("ok"), Some("yes"));
    assert_eq!(doc.str("k"), None);
    assert_eq!(doc.fields.len(), 2);
}

#[test]
fn inline_lists() {
    let doc = parse("---\ntools: [Read, Grep,  Bash ]\nempty: []\nq: [\"a, b\", 'c', d]\none: [x]\n---\n");
    assert_eq!(doc.list("tools"), vec!["Read", "Grep", "Bash"]);
    assert_eq!(doc.list("empty"), Vec::<String>::new());
    assert_eq!(doc.list("q"), vec!["a, b", "c", "d"]);
    assert_eq!(doc.list("one"), vec!["x"]);
    assert!(matches!(doc.fields.get("empty"), Some(Value::List(v)) if v.is_empty()));
}

#[test]
fn inline_list_over_several_lines() {
    let doc = parse("---\ntools: [Read,\n  Grep,\n  Bash]\nnext: v\n---\n");
    assert_eq!(doc.list("tools"), vec!["Read", "Grep", "Bash"]);
    assert_eq!(doc.str("next"), Some("v"));
}

#[test]
fn unterminated_inline_list_stays_a_string() {
    let doc = parse("---\ntools: [Read, Grep\nnext: v\n---\n");
    assert_eq!(doc.str("tools"), Some("[Read, Grep"));
    assert_eq!(doc.str("next"), Some("v"));
}

#[test]
fn block_lists() {
    let doc = parse("---\ntools:\n  - Read\n  - \"Grep\"\n  # skipped\n  - 'Bash'\n  -\nsame:\n- a\n- b\nafter: x\n---\n");
    assert_eq!(doc.list("tools"), vec!["Read", "Grep", "Bash"]);
    assert_eq!(doc.list("same"), vec!["a", "b"]);
    assert_eq!(doc.str("after"), Some("x"));
}

#[test]
fn comma_string_reads_as_list() {
    let doc = parse("---\ntools: Read, Grep ,Bash,\nnone:\nmapped:\n  a: b\n---\n");
    assert_eq!(doc.list("tools"), vec!["Read", "Grep", "Bash"]);
    assert!(doc.list("none").is_empty());
    assert!(doc.list("mapped").is_empty());
    assert!(doc.list("missing").is_empty());
}

#[test]
fn literal_block_scalar() {
    let doc = parse("---\nprompt: |\n  line one\n    indented\n\n  line three\n\nname: x\n---\nbody");
    assert_eq!(doc.str("prompt"), Some("line one\n  indented\n\nline three"));
    assert_eq!(doc.str("name"), Some("x"));
    assert_eq!(doc.body, "body");
}

#[test]
fn folded_block_scalar() {
    let doc = parse("---\ndescription: >\n  one long\n  sentence here\n\n  second paragraph\nname: x\n---\n");
    assert_eq!(doc.str("description"), Some("one long sentence here\nsecond paragraph"));
    assert_eq!(doc.str("name"), Some("x"));
}

#[test]
fn block_scalar_modifiers_and_edges() {
    let doc = parse("---\na: |-\n  x\nb: >+\n  y\n  z\nc: |\nd: |2\n    keep\n---\n");
    assert_eq!(doc.str("a"), Some("x"));
    assert_eq!(doc.str("b"), Some("y z"));
    assert_eq!(doc.str("c"), Some(""));
    assert_eq!(doc.str("d"), Some("  keep"));
}

#[test]
fn block_scalar_may_contain_hash_and_colon_lines() {
    let doc = parse("---\nprompt: |\n  # heading\n  key: value\n  - item\nafter: 1\n---\n");
    assert_eq!(doc.str("prompt"), Some("# heading\nkey: value\n- item"));
    assert_eq!(doc.str("after"), Some("1"));
}

#[test]
fn plain_scalar_continues_on_deeper_lines() {
    let doc = parse("---\ndescription: Reviews code and\n  reports problems\nname: x\nnext:\n  more text\n  and more\n---\n");
    assert_eq!(doc.str("description"), Some("Reviews code and reports problems"));
    assert_eq!(doc.str("name"), Some("x"));
    assert_eq!(doc.str("next"), Some("more text and more"));
}

#[test]
fn quoted_scalar_continues_on_deeper_lines() {
    let doc = parse("---\ndescription: \"first part\n  second part\"\nname: x\n---\n");
    assert_eq!(doc.str("description"), Some("first part second part"));
}

#[test]
fn harness_block_nested_maps() {
    let doc = parse(
        "---\nname: pipeline\nharness:\n  kind: stage\n  uses: [lint, test]\n  spawn:\n    - reviewer\n    - fixer\n  limits:\n    turns: 12\n    cost: '1.5'\nmodel: fast\n---\n",
    );
    let harness = doc.map("harness").expect("harness map");
    assert_eq!(harness.get("kind"), Some(&s("stage")));
    assert_eq!(harness.get("uses"), Some(&list(&["lint", "test"])));
    assert_eq!(harness.get("spawn"), Some(&list(&["reviewer", "fixer"])));
    assert_eq!(harness.get("limits"), Some(&map(&[("turns", s("12")), ("cost", s("1.5"))])));
    assert_eq!(doc.str("model"), Some("fast"));
    assert_eq!(doc.str("harness"), None);
    assert!(doc.map("model").is_none());
}

#[test]
fn deeply_nested_input_is_bounded() {
    let mut text = String::from("---\n");
    for depth in 0..40 {
        text.push_str(&format!("{}k{depth}:\n", "  ".repeat(depth)));
    }
    text.push_str("top: 1\n---\n");
    let doc = parse(&text);
    assert!(doc.fields.contains_key("k0"));
}

#[test]
fn unknown_syntax_is_kept_as_string() {
    let doc = parse("---\nflow: {a: 1, b: 2}\nanchor: &x value\nalias: *x\ntilde: ~\nnum: 3.5\n---\n");
    assert_eq!(doc.str("flow"), Some("{a: 1, b: 2}"));
    assert_eq!(doc.str("anchor"), Some("&x value"));
    assert_eq!(doc.str("alias"), Some("*x"));
    assert_eq!(doc.str("tilde"), Some("~"));
    assert_eq!(doc.str("num"), Some("3.5"));
}

#[test]
fn garbage_does_not_panic() {
    let cases = [
        "---\n:\n---\n",
        "---\n: :\n---\n",
        "---\n[\n---\n",
        "---\nk: [\n---\n",
        "---\nk: \"unclosed\n---\n",
        "---\nk: |\n---\n",
        "---\n\t\tk: v\n  j: w\n---\n",
        "---\n- a\n- b\n---\n",
        "---\nk:\n\t- a\n---\n",
        "---\nк: значение\nключ:\n  - один\n---\nтекст",
        "---\nk: \"\\",
        "---\n---\n---\n",
        "---\r\n",
        "---\n\u{0}\u{1}: \u{2}\n---\n",
        "---\nk: 'a\n---\n",
        "---\nk: >9\n  x\n---\n",
        "---\n  a: 1\nb: 2\n   c: 3\n---\n",
    ];
    for text in cases {
        let doc = parse(text);
        let _ = render(&doc.fields, doc.body);
    }
}

#[test]
fn non_ascii_values() {
    let doc = parse("---\nname: ревьюер\ntools: [чтение, 日本語]\n---\nтекст\n");
    assert_eq!(doc.str("name"), Some("ревьюер"));
    assert_eq!(doc.list("tools"), vec!["чтение", "日本語"]);
    assert_eq!(doc.body, "текст\n");
}

#[test]
fn duplicate_key_keeps_last() {
    let doc = parse("---\na: 1\na: 2\n---\n");
    assert_eq!(doc.str("a"), Some("2"));
}

#[test]
fn render_is_stable_and_sorted() {
    let f = fields(&[
        ("name", s("x")),
        ("description", s("Does a thing")),
        ("tools", list(&["Read", "Grep"])),
        ("harness", map(&[("kind", s("stage")), ("uses", list(&["lint"]))])),
    ]);
    let text = render(&f, "Body\n");
    assert_eq!(
        text,
        "---\ndescription: Does a thing\nharness:\n  kind: stage\n  uses: [lint]\nname: x\ntools: [Read, Grep]\n---\nBody\n"
    );
    assert_eq!(render(&f, "Body\n"), text);
}

fn round_trip(f: &BTreeMap<String, Value>, body: &str) {
    let text = render(f, body);
    let doc = parse(&text);
    assert_eq!(&doc.fields, f, "fields changed through:\n{text}");
    assert_eq!(doc.body, body, "body changed through:\n{text}");
    assert_eq!(render(&doc.fields, doc.body), text, "second render differs");
}

#[test]
fn round_trip_plain_values() {
    round_trip(
        &fields(&[
            ("name", s("reviewer")),
            ("description", s("Reviews: a diff # carefully")),
            ("tools", list(&["Read", "Grep"])),
            ("harness", map(&[("kind", s("stage")), ("limits", map(&[("turns", s("12"))]))])),
        ]),
        "# Body\n\nSome text.\n",
    );
}

#[test]
fn round_trip_awkward_values() {
    round_trip(
        &fields(&[
            ("empty", s("")),
            ("spaces", s("  padded  ")),
            ("quote", s("say \"hi\"")),
            ("single", s("'single'")),
            ("backslash", s("C:\\dir\\file")),
            ("bracket", s("[not a list]")),
            ("brace", s("{}")),
            ("dash", s("- not an item")),
            ("hash", s("# not a comment")),
            ("pipe", s("| not a block")),
            ("colon", s("ends with:")),
            ("tab", s("a\tb")),
            ("unicode", s("привет, мир")),
            ("empty_list", list(&[])),
            ("empty_map", map(&[])),
            ("odd_list", list(&["a, b", "[x]", " lead", "q\"uote", "- dash", "multi\nline"])),
            (
                "long_list",
                list(&["alpha-alpha-alpha", "beta-beta-beta", "gamma-gamma-gamma", "delta-delta-delta", "epsilon"]),
            ),
        ]),
        "body",
    );
}

#[test]
fn round_trip_multiline_scalars() {
    round_trip(
        &fields(&[
            ("prompt", s("line one\n  indented\n\nline four: with colon\n# not a comment\n- not a list")),
            ("leading", s("  starts with spaces\nsecond")),
            ("inner", map(&[("text", s("a\nb"))])),
        ]),
        "after\n",
    );
}

#[test]
fn round_trip_without_header() {
    round_trip(&BTreeMap::new(), "plain text\n");
    round_trip(&BTreeMap::new(), "");
    // A body that looks like a fence still survives.
    round_trip(&BTreeMap::new(), "---\nnot: header\n---\nreally body\n");
    round_trip(&fields(&[("a", s("b"))]), "---\nbody that starts with a fence\n");
}

#[test]
fn render_without_fields_writes_no_header() {
    assert_eq!(render(&BTreeMap::new(), "text"), "text");
}

#[test]
fn render_then_parse_real_agent_file() {
    let source = "---\nname: code-reviewer\ndescription: >\n  Reviews a diff for bugs.\n  Use after a change.\ntools: Read, Grep, Bash\nmodel: sonnet\nharness:\n  kind: agent\n  spawn: [fixer]\n---\nYou are a reviewer.\n";
    let doc = parse(source);
    assert_eq!(doc.str("description"), Some("Reviews a diff for bugs. Use after a change."));
    assert_eq!(doc.list("tools"), vec!["Read", "Grep", "Bash"]);
    round_trip(&doc.fields, doc.body);
}
