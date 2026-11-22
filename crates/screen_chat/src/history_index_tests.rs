use super::*;
use ui::time::{long_date, when_short};

fn chat(id: &str, title: &str, agent: &str, wd: &str, updated: u64) -> ChatMeta {
    let v = json!({
        "id": id,
        "title": title,
        "agent": agent,
        "workdir": wd,
        "cost_usd": 0.25,
        "messages": [
            {"role": "user", "content": [{"Text": "Fix the retry bug"}]},
            {"role": "assistant", "content": [
                {"Text": "Looking at retry.ts"},
                {"ToolUse": {"id": "t1", "name": "edit_file", "input": {"path": "src/retry.ts"}}}
            ]},
            {"role": "user", "content": [{"ToolResult": {"tool_use_id": "t1", "content": "ok", "is_error": false}}]},
        ],
    });
    meta_from_json(&v, updated).unwrap()
}

#[test]
fn meta_collects_snippets_files_and_counts() {
    let c = chat("a", "Retry fix", "", "/nonexistent/proj", 10);
    assert_eq!(c.agent, "Build");
    assert_eq!(c.messages, 2);
    assert_eq!(c.files, vec!["src/retry.ts"]);
    assert_eq!(c.snippets.len(), 2);
    assert_eq!(c.snippets[0].who, "You");
    assert_eq!(c.snippets[1].who, "Build");
    assert_eq!(c.wd_label, "proj");
    assert!(c.matches("retry.ts"));
    assert!(c.matches("fix the"));
    assert!(!c.matches("nothing like this"));
}

#[test]
fn meta_skips_attachment_blocks() {
    let v = json!({
        "id": "b",
        "messages": [{"role": "user", "content": [
            {"Text": "look"},
            {"Text": session::encode_file("a.rs", "secret body")}
        ]}],
    });
    let c = meta_from_json(&v, 0).unwrap();
    assert_eq!(c.title, "Untitled session");
    assert_eq!(c.snippets.len(), 1);
    assert!(!c.matches("secret body"));
}

#[test]
fn trailing_tokens() {
    assert_eq!(
        split_trailing_token("fix in:pipe "),
        Some(("fix".into(), "in:pipe".into()))
    );
    assert_eq!(split_trailing_token("agent:plan "), Some((String::new(), "agent:plan".into())));
    assert_eq!(split_trailing_token("in:pipe"), None);
    assert_eq!(split_trailing_token("foo:bar "), None);
    assert_eq!(split_trailing_token("in: "), None);
}

#[test]
fn filter_by_tokens_query_flags() {
    let chats = vec![
        chat("a", "Retry fix", "Plan", "/x/alpha", 30),
        chat("b", "Docs", "", "/x/beta", 20),
        chat("c", "Retry other", "", "/x/alpha", 10),
    ];
    let mut flags = Flags::default();
    flags.archived.insert("c".into());
    flags.pinned.insert("b".into());
    let st = |c: &ChatMeta| {
        if c.id == "a" {
            LiveStatus::Running
        } else {
            LiveStatus::Done
        }
    };
    let ids = |f: &Filter| filter(&chats, &flags, f, &st).iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&Filter::default()), vec!["a", "b"]);
    let f = Filter { tokens: vec!["in:alpha".into()], ..Default::default() };
    assert_eq!(ids(&f), vec!["a"]);
    let f = Filter { tokens: vec!["agent:build".into()], ..Default::default() };
    assert_eq!(ids(&f), vec!["b"]);
    let f = Filter { tokens: vec!["is:pinned".into()], ..Default::default() };
    assert_eq!(ids(&f), vec!["b"]);
    let f = Filter { running: true, ..Default::default() };
    assert_eq!(ids(&f), vec!["a"]);
    let f = Filter { query: "RETRY FIX".into(), ..Default::default() };
    assert_eq!(ids(&f), vec!["a"]);
    let f = Filter { scope: Scope::Agents, ..Default::default() };
    assert!(ids(&f).is_empty());
}

#[test]
fn flags_roundtrip() {
    let dir = std::env::temp_dir().join(format!("harness-flags-{}", std::process::id()));
    let path = dir.join("chat_flags.json");
    let mut f = Flags::default();
    f.pinned.insert("p".into());
    f.archived.insert("a".into());
    f.save_to(&path).unwrap();
    assert_eq!(Flags::load_from(&path), f);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn dates_and_groups() {
    // 2026-11-22 12:00 UTC is a Sunday.
    let now = 1_795_348_800;
    let d = date_of(now, 0);
    assert_eq!((d.year, d.month, d.day, d.weekday), (2026, 11, 22, 6));
    assert_eq!(group_of(now - 3600, now, 0), Group::Today);
    assert_eq!(group_of(now - 86_400, now, 0), Group::Yesterday);
    assert_eq!(group_of(now - 4 * 86_400, now, 0), Group::ThisWeek);
    assert_eq!(group_of(now - 10 * 86_400, now, 0), Group::ThisMonth);
    assert_eq!(group_of(now - 40 * 86_400, now, 0), Group::Month(2026, 10));
    assert_eq!(Group::Month(2026, 10).name(), "October 2026");
    assert_eq!(when_label(now - 12 * 60, now, 0), "12m");
    assert_eq!(when_label(now - 3 * 3600, now, 0), "3h");
    assert_eq!(when_label(now - 86_400, now, 0), "Sat 12:00");
    assert_eq!(when_short(now - 86_400, now, 0), "Sat");
    assert_eq!(when_label(now - 13 * 86_400, now, 0), "Nov 9");
    assert_eq!(long_date(now, 0), "Sunday, 22 November");
}

#[test]
fn case_insensitive_find() {
    assert_eq!(find_ci("Fix Retry now", "retry"), Some((4, 9)));
    assert_eq!(find_ci("abc", ""), None);
}

#[test]
fn head_branch_reads_git_head() {
    let dir = std::env::temp_dir().join(format!("harness-head-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feat/x\n").unwrap();
    assert_eq!(head_branch(&dir.join("sub")).as_deref(), Some("feat/x"));
    std::fs::remove_dir_all(dir).ok();
}
