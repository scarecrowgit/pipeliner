use model::{RunnableKind, RunnableRef};

use super::*;
use crate::testing::{agent, runnable};

fn library() -> Vec<model::Runnable> {
    vec![
        agent("Fixer"),
        agent("Reproducer"),
        runnable(RunnableKind::Orchestrator, "Release captain"),
        runnable(RunnableKind::Pipeline, "Release"),
        runnable(RunnableKind::Pipeline, "PAY triage"),
        agent("Дизайнер"),
    ]
}

fn names(text: &str) -> Vec<String> {
    find_mentions(text, &library()).into_iter().map(|m| m.name).collect()
}

#[test]
fn finds_a_mention_with_its_range() {
    let text = "Hey @Fixer, look";
    let found = find_mentions(text, &library());
    assert_eq!(found.len(), 1);
    assert_eq!(&text[found[0].range.clone()], "@Fixer");
    assert_eq!(found[0].target, RunnableRef::agent("fixer"));
}

#[test]
fn matching_ignores_case_and_reports_the_library_spelling() {
    assert_eq!(names("@fixer and @FIXER and @FiXeR"), ["Fixer", "Fixer", "Fixer"]);
}

#[test]
fn longest_name_wins() {
    let found = find_mentions("ping @Release captain now", &library());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Release captain");
    assert_eq!(names("ping @release notes"), ["Release"]);
    assert_eq!(names("@Release"), ["Release"]);
}

#[test]
fn names_with_spaces() {
    assert_eq!(names("@PAY triage please"), ["PAY triage"]);
}

#[test]
fn a_mention_must_start_a_word() {
    assert!(names("mail bob@Fixer.com").is_empty());
    assert!(names("a_@Fixer").is_empty());
    assert!(names("@@Fixer").is_empty());
    assert_eq!(names("(@Fixer)"), ["Fixer"]);
    assert_eq!(names("\"@Fixer\""), ["Fixer"]);
    assert_eq!(names("x:@Fixer"), ["Fixer"]);
    assert_eq!(names("line\n@Fixer"), ["Fixer"]);
}

#[test]
fn a_mention_must_end_at_a_word_boundary() {
    assert!(names("@Fixers").is_empty());
    assert!(names("@Fixer2").is_empty());
    assert!(names("@Fixer_x").is_empty());
    for tail in [",", ".", "!", "?", ":", ")", "'s", "-", "\n", " "] {
        assert_eq!(names(&format!("@Fixer{tail}")), ["Fixer"], "{tail:?}");
    }
}

#[test]
fn unknown_names_and_bare_ats_are_not_mentions() {
    assert!(names("@nobody @ @").is_empty());
    assert!(names("").is_empty());
    assert!(names("@").is_empty());
}

#[test]
fn code_spans_hide_mentions() {
    assert!(names("run `@Fixer` now").is_empty());
    assert!(names("``a ` @Fixer``").is_empty());
    assert!(names("```\n@Fixer\n```").is_empty());
    assert_eq!(names("`x` @Fixer `y`"), ["Fixer"]);
    assert_eq!(names("@Fixer `@Reproducer`"), ["Fixer"]);
    // An unclosed backtick is plain text.
    assert_eq!(names("it`s @Fixer"), ["Fixer"]);
}

#[test]
fn non_ascii_text_and_names() {
    let text = "Привет, @дизайнер! Ещё @Fixer — ок";
    let found = find_mentions(text, &library());
    assert_eq!(found.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Дизайнер", "Fixer"]);
    assert_eq!(&text[found[0].range.clone()], "@дизайнер");
    assert_eq!(&text[found[1].range.clone()], "@Fixer");
    assert_eq!(names("😀@Fixer😀"), ["Fixer"]);
    assert!(names("é@Fixer").is_empty());
    assert_eq!(names("«@Fixer»"), ["Fixer"]);
}

#[test]
fn several_mentions_and_distinct_targets() {
    let found = find_mentions("@Fixer then @Reproducer then @fixer", &library());
    assert_eq!(found.len(), 3);
    assert_eq!(targets(&found), [RunnableRef::agent("fixer"), RunnableRef::agent("reproducer")]);
}

#[test]
fn archived_and_paused_names_are_still_found() {
    let mut lib = library();
    lib[0].archived = true;
    lib[1].paused = true;
    assert_eq!(find_mentions("@Fixer @Reproducer", &lib).len(), 2);
}

// ---- completion

fn items(prefix: &str, attached: Option<&RunnableRef>, limit: usize) -> Vec<String> {
    complete(prefix, &library(), attached, limit).items.into_iter().map(|i| i.name).collect()
}

#[test]
fn prefix_matches_come_before_substring_matches() {
    // "re": Reproducer and both Release start with it; "Fixer" does not contain it.
    assert_eq!(items("re", None, 10), ["Release captain", "Reproducer", "Release"]);
    // "ix": only a substring match.
    assert_eq!(items("ix", None, 10), ["Fixer"]);
    // "er": substring matches only; "Дизайнер" has Cyrillic letters, not "er".
    assert_eq!(items("er", None, 10), ["Fixer", "Reproducer"]);
    assert_eq!(items("pro", None, 10), ["Reproducer"]);
    // Prefix class first, then substring class.
    assert_eq!(items("r", None, 10), ["Release captain", "Reproducer", "Release", "Fixer", "PAY triage"]);
}

#[test]
fn attached_runner_first_within_its_class() {
    let reproducer = RunnableRef::agent("reproducer");
    assert_eq!(items("re", Some(&reproducer), 10), ["Reproducer", "Release captain", "Release"]);
    assert_eq!(items("", Some(&reproducer), 3), ["Reproducer", "Release captain", "Fixer"]);
}

#[test]
fn then_by_kind_and_name() {
    assert_eq!(items("", None, 10), ["Release captain", "Fixer", "Reproducer", "Дизайнер", "PAY triage", "Release"]);
}

#[test]
fn limit_cuts_the_rows_and_total_counts_all() {
    let c = complete("re", &library(), None, 2);
    assert_eq!(c.items.len(), 2);
    assert_eq!(c.total, 3);
    assert_eq!(c.heading(), "Matching “re” · 2 of 3");
    assert_eq!(complete("zzz", &library(), None, 5).total, 0);
    assert!(complete("re", &library(), None, 0).items.is_empty());
    assert_eq!(complete("", &library(), None, 1).heading(), "Agents and pipelines · 1 of 6");
}

#[test]
fn highlight_covers_the_match() {
    let c = complete("re", &library(), None, 10);
    let rep = c.items.iter().find(|i| i.name == "Reproducer").unwrap();
    assert_eq!(&rep.name[rep.highlight.clone()], "Re");
    let c = complete("IX", &library(), None, 10);
    assert_eq!(&c.items[0].name[c.items[0].highlight.clone()], "ix");
    let c = complete("диз", &library(), None, 10);
    assert_eq!(&c.items[0].name[c.items[0].highlight.clone()], "Диз");
    let c = complete("", &library(), None, 10);
    assert!(c.items.iter().all(|i| i.highlight.is_empty()));
}

#[test]
fn descriptions_follow_the_mockup() {
    let reproducer = RunnableRef::agent("reproducer");
    let c = complete("re", &library(), Some(&reproducer), 10);
    assert_eq!(c.items[0].description, "Agent · on this issue now · gets your comment as input");
    assert_eq!(c.items[0].standing, Standing::Attached);
    assert_eq!(c.items[1].description, "Orchestrator · starts a new run on this issue");
    assert_eq!(
        c.items[1].description_on(&model::IssueKey::new("PAY-1427")),
        "Orchestrator · starts a new run on PAY-1427"
    );
    assert_eq!(c.items[1].insert_text(), "@Release captain ");
}

#[test]
fn archived_paused_and_non_issue_runnables() {
    let mut lib = library();
    lib[0].archived = true;
    lib[1].paused = true;
    lib[4].uses.issues = false;
    let c = complete("", &lib, None, 10);
    let names: Vec<_> = c.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Release captain", "Reproducer", "Дизайнер", "Release"]);
    let rep = c.items.iter().find(|i| i.name == "Reproducer").unwrap();
    assert_eq!(rep.standing, Standing::Paused);
    assert_eq!(rep.description, "Agent · paused · takes no new work");
}

#[test]
fn completion_is_case_insensitive_and_unicode_aware() {
    assert_eq!(items("ДИЗ", None, 10), ["Дизайнер"]);
    assert_eq!(items("pay", None, 10), ["PAY triage"]);
    assert_eq!(items("release c", None, 10), ["Release captain"]);
}

// ---- cursor

#[test]
fn finds_the_mention_under_the_cursor() {
    assert_eq!(mention_at_cursor("Batching fixed. @re", 19), Some((16, "re".to_string())));
    assert_eq!(mention_at_cursor("@", 1), Some((0, String::new())));
    assert_eq!(mention_at_cursor("hi @Fix and more", 7), Some((3, "Fix".to_string())));
    assert_eq!(mention_at_cursor("(@fi", 4), Some((1, "fi".to_string())));
}

#[test]
fn multi_word_prefixes_keep_their_space() {
    assert_eq!(mention_at_cursor("@Release c", 10), Some((0, "Release c".to_string())));
}

#[test]
fn no_mention_in_emails_code_or_after_punctuation() {
    assert_eq!(mention_at_cursor("bob@exa", 7), None);
    assert_eq!(mention_at_cursor("`@fi`", 4), None);
    assert_eq!(mention_at_cursor("`x @fi`", 6), None);
    assert_eq!(mention_at_cursor("@Fixer, thanks", 14), None);
    assert_eq!(mention_at_cursor("@Fixer\nthanks", 13), None);
    assert_eq!(mention_at_cursor("no at sign", 5), None);
    assert_eq!(mention_at_cursor("", 0), None);
    assert_eq!(mention_at_cursor("@a", 0), None);
    let long = format!("@{}", "a".repeat(60));
    assert_eq!(mention_at_cursor(&long, long.len()), None);
}

#[test]
fn cursor_handles_non_ascii_and_bad_offsets() {
    let text = "Привет @диз";
    assert_eq!(mention_at_cursor(text, text.len()), Some((13, "диз".to_string())));
    // A cursor inside a multi-byte char falls back to the char before.
    assert_eq!(mention_at_cursor("@ди", 3), Some((0, "д".to_string())));
    // Past the end is clamped.
    assert_eq!(mention_at_cursor("@fi", 99), Some((0, "fi".to_string())));
    assert_eq!(mention_at_cursor("😀@x", 6), Some((4, "x".to_string())));
}
