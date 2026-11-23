use model::{
    ChatMode, ContentBlock, ModelRef, Role, RunnableRef, SessionId, SessionOrigin, SessionState, Usage, WorkMode,
};
use serde_json::json;

use super::{Fixture, assistant_text, old_session, user_text};
use crate::SessionFilter;

const LEGACY: &str = "1763560000123-4821";

fn id() -> SessionId {
    SessionId::new(format!("v1-{LEGACY}"))
}

/// A session that uses every field the old app wrote.
fn full_session(fx: &Fixture) -> serde_json::Value {
    let home = fx.paths.home.display().to_string();
    let file_block = "<file path=\"src/a.rs\">\nfn a() {}\n</file>";
    let shell_block = "<shell_output command=\"ls\" failed=\"true\">\nThe user ran this command in the workspace shell; the output is context for the next request.\nno such file\n</shell_output>";
    let note_block = "<workdir_note text=\"Moved to x\">\nThe working folder is now /x.\n</workdir_note>";
    let mut session = old_session(
        LEGACY,
        json!([
            user_text("look at @src/a.rs"),
            {"role": "user", "content": [{"Text": file_block}]},
            {"role": "assistant", "content": [
                {"Text": "Reading it."},
                {"ToolUse": {"id": "t1", "name": "read_file", "input": {"path": "src/a.rs"}}}
            ]},
            {"role": "user", "content": [{"ToolResult": {"tool_use_id": "t1", "content": "fn a() {}", "is_error": false}}]},
            {"role": "user", "content": [{"Text": shell_block}, {"Text": note_block}, {"Text": "   "}]},
            assistant_text("Done."),
        ]),
    );
    session["title"] = json!("Fix the thing");
    session["cost_usd"] = json!(0.75);
    session["usage"] =
        json!({"input_tokens": 1000, "output_tokens": 200, "cache_read_tokens": 30, "cache_write_tokens": 4});
    session["last_usage"] = json!({"model": "cc/claude-opus-5-5", "input_tokens": 500, "output_tokens": 50, "cache_read_tokens": 100, "cache_write_tokens": 7});
    session["mode"] = json!("plan");
    session["agent"] = json!("Fixer Bot");
    session["forced_skills"] = json!(["rust", "gpui"]);
    session["workdir"] = json!(format!("{home}/.harness/worktrees/payments/chat-session"));
    session["work_mode"] = json!("worktree");
    session
}

#[test]
fn a_session_is_mapped_field_by_field() {
    let fx = Fixture::new();
    let path = fx.write_json(&format!("sessions/{LEGACY}.json"), &full_session(&fx));
    fx.touch(&path, 1_763_560_100_123);
    fx.write_json("chat_flags.json", &json!({"pinned": [LEGACY], "archived": []}));

    let report = fx.import();
    assert_eq!((report.sessions.imported, report.messages.imported), (1, 6));
    assert!(report.problems.is_empty(), "{report}");

    let s = fx.store.session(&id()).unwrap().unwrap();
    assert_eq!(s.title, "Fix the thing");
    assert_eq!(s.model, ModelRef::parse("nr/cc/claude-opus-5-5"));
    assert_eq!(s.mode, ChatMode::Plan);
    assert_eq!(s.runnable, RunnableRef::agent("fixer-bot"));
    assert_eq!(
        (s.state, s.origin.clone()),
        (SessionState::Idle, SessionOrigin::Imported)
    );
    assert_eq!((s.parent.clone(), s.depth, s.issue.clone()), (None, 0, None));
    assert_eq!((s.pinned, s.archived), (true, false));
    assert_eq!((s.created, s.updated), (1_763_560_000_123, 1_763_560_100_123));
    assert_eq!(s.message_count, 6);
    assert_eq!(s.cost_usd, 0.75);
    assert_eq!(
        s.usage,
        Usage {
            input: 1000,
            output: 200,
            cache_read: 30,
            cache_write: 4
        }
    );
    assert_eq!(s.context_tokens, 607);
    assert_eq!(s.forced_skills, ["rust", "gpui"]);
    let workdir = s.workdir.unwrap();
    assert_eq!(workdir.path, fx.paths.worktrees_dir().join("payments/chat-session"));
    assert_eq!(
        (workdir.repo.as_str(), workdir.mode, workdir.branch),
        ("payments", WorkMode::Worktree, None)
    );
    assert_eq!(workdir.runtime, "local");
    assert_eq!(fx.store.session_by_legacy_id(LEGACY).unwrap().unwrap().id, id());
}

#[test]
fn messages_are_converted_and_spaced_between_created_and_updated() {
    let fx = Fixture::new();
    let path = fx.write_json(&format!("sessions/{LEGACY}.json"), &full_session(&fx));
    fx.touch(&path, 1_763_560_005_123);
    fx.import();

    let messages = fx.store.messages(&id(), 1..100).unwrap();
    assert_eq!(messages.len(), 6);
    // Six messages over five seconds: one per second, first at the creation, last at the update.
    let times: Vec<i64> = messages.iter().map(|m| m.at).collect();
    assert_eq!(times, [0, 1000, 2000, 3000, 4000, 5000].map(|d| 1_763_560_000_123 + d));
    assert_eq!(messages.iter().map(|m| m.seq).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);

    assert_eq!(messages[0].message.plain_text(), "look at @src/a.rs");
    assert_eq!(
        messages[1].message.plain_text(),
        "Attached file `src/a.rs`:\n```\nfn a() {}\n```"
    );
    assert_eq!(messages[2].message.role, Role::Assistant);
    assert_eq!(messages[2].message.content[0], ContentBlock::text("Reading it."));
    assert_eq!(
        messages[2].message.content[1],
        ContentBlock::ToolUse {
            id: "t1".into(),
            name: "read_file".into(),
            input: json!({"path": "src/a.rs"})
        }
    );
    assert_eq!(
        messages[3].message.content,
        vec![ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: "fn a() {}".into(),
            is_error: false
        }]
    );
    // The shell run and the divider are plain text; the blank block is gone.
    assert_eq!(
        messages[4].message.content,
        vec![
            ContentBlock::text("Shell command (failed):\n```\n$ ls\nno such file\n```"),
            ContentBlock::text("\u{2014} Moved to x \u{2014}\nThe working folder is now /x."),
        ]
    );
    assert_eq!(messages[5].message.plain_text(), "Done.");
    assert!(
        messages
            .iter()
            .all(|m| !m.compacted && m.model.is_none() && m.cost_usd == 0.0)
    );
}

#[test]
fn the_whole_cost_is_one_imported_ledger_entry_dated_at_the_last_change() {
    let fx = Fixture::new();
    let path = fx.write_json(&format!("sessions/{LEGACY}.json"), &full_session(&fx));
    fx.touch(&path, 1_763_560_100_123);
    fx.import();

    let spent = fx.store.spend_for_session(&id()).unwrap();
    assert_eq!(spent.calls, 1);
    assert_eq!(spent.cost_usd, 0.75);
    assert_eq!(
        spent.usage,
        Usage {
            input: 1000,
            output: 200,
            cache_read: 30,
            cache_write: 4
        }
    );
    let day = fx.store.spend_between(1_763_560_100_000, 1_763_560_101_000).unwrap();
    assert_eq!(day.calls, 1);
    let by_agent = fx.store.spend_by_runnable(0, i64::MAX, 5).unwrap();
    assert_eq!(
        (by_agent[0].runnable.clone(), by_agent[0].name.as_str()),
        (RunnableRef::agent("fixer-bot"), "Fixer Bot")
    );
}

#[test]
fn an_old_file_with_only_the_early_fields_still_imports() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000999-0001.json",
        &json!({"id": "1763560000999-0001", "title": null, "model": null, "cost_usd": 0.0,
                "messages": [user_text("\n  Make the build faster\nplease"), assistant_text("ok")]}),
    );
    let report = fx.import();
    assert_eq!(report.sessions.imported, 1, "{report}");

    let s = fx
        .store
        .session(&SessionId::new("v1-1763560000999-0001"))
        .unwrap()
        .unwrap();
    assert_eq!(s.title, "Make the build faster");
    assert_eq!(
        (s.mode, s.runnable.clone(), s.workdir.clone()),
        (ChatMode::Build, RunnableRef::agent("build"), None)
    );
    assert_eq!(s.model, ModelRef::default());
    assert_eq!((s.cost_usd, s.usage), (0.0, Usage::default()));
    // Nothing was spent, so there is nothing in the ledger.
    assert_eq!(fx.store.spend_for_session(&s.id).unwrap().calls, 0);
}

#[test]
fn a_plan_session_and_a_session_without_a_clock_in_its_id() {
    let fx = Fixture::new();
    let mut plan = old_session("old-style-id", json!([user_text("plan it")]));
    plan["mode"] = json!("plan");
    plan["work_mode"] = json!("read_only");
    plan["workdir"] = json!("~/code/repo");
    let path = fx.write_json("sessions/old-style-id.json", &plan);
    fx.touch(&path, 1_700_000_000_000);
    fx.import();

    let s = fx.store.session(&SessionId::new("v1-old-style-id")).unwrap().unwrap();
    assert_eq!(s.runnable, RunnableRef::agent("plan"));
    // No clock in the id: the file's time stands for both.
    assert_eq!((s.created, s.updated), (1_700_000_000_000, 1_700_000_000_000));
    let workdir = s.workdir.unwrap();
    assert_eq!(workdir.path, fx.paths.home.join("code/repo"));
    assert_eq!((workdir.repo.as_str(), workdir.mode), ("repo", WorkMode::ReadOnly));
}

#[test]
fn pinned_and_archived_come_from_the_flags_file() {
    let fx = Fixture::new();
    for id in ["1763560000001-0001", "1763560000002-0002", "1763560000003-0003"] {
        fx.write_json(
            &format!("sessions/{id}.json"),
            &old_session(id, json!([user_text("hi")])),
        );
    }
    fx.write_json(
        "chat_flags.json",
        &json!({"pinned": ["1763560000001-0001"], "archived": ["1763560000002-0002"]}),
    );
    fx.import();

    let pinned = fx
        .store
        .sessions(&SessionFilter {
            pinned: Some(true),
            ..SessionFilter::default()
        })
        .unwrap();
    assert_eq!(
        pinned.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["v1-1763560000001-0001"]
    );
    let archived = fx
        .store
        .sessions(&SessionFilter {
            archived: Some(true),
            ..SessionFilter::default()
        })
        .unwrap();
    assert_eq!(
        archived.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["v1-1763560000002-0002"]
    );
}

#[test]
fn a_broken_flags_file_is_reported_and_the_sessions_still_import() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("hi")])),
    );
    fx.write("chat_flags.json", "{not json");
    let report = fx.import();
    assert_eq!(report.sessions.imported, 1);
    assert_eq!(report.problems.len(), 1);
    assert_eq!(report.problems[0].item, "chat_flags.json");
    assert_eq!(report.failed_total(), 1);
}

#[test]
fn a_second_import_finds_the_sessions_again_and_imports_nothing() {
    let fx = Fixture::new();
    fx.write_json(&format!("sessions/{LEGACY}.json"), &full_session(&fx));
    let first = fx.import();
    assert_eq!((first.sessions.imported, first.messages.imported), (1, 6));

    let second = fx.import();
    assert_eq!((second.sessions.imported, second.sessions.skipped), (0, 1));
    assert_eq!((second.messages.imported, second.messages.skipped), (0, 6));
    assert_eq!(fx.store.sessions(&SessionFilter::default()).unwrap().len(), 1);
    assert_eq!(fx.store.messages(&id(), 1..100).unwrap().len(), 6);
    assert_eq!(fx.store.spend_for_session(&id()).unwrap().calls, 1);
}

#[test]
fn a_session_the_old_app_saved_later_is_imported_by_the_next_run() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("a")])),
    );
    fx.import();
    fx.write_json(
        "sessions/1763560000002-0002.json",
        &old_session("1763560000002-0002", json!([user_text("b")])),
    );
    let report = fx.import();
    assert_eq!((report.sessions.imported, report.sessions.skipped), (1, 1));
}

#[test]
fn a_dry_run_reports_the_same_counts_and_writes_nothing() {
    let fx = Fixture::new();
    fx.write_json(&format!("sessions/{LEGACY}.json"), &full_session(&fx));
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("a")])),
    );
    fx.write("sessions/broken.json", "nope");
    let before = fx.snapshot();

    let dry = fx.dry_import();
    assert!(dry.dry_run);
    assert!(fx.store.sessions(&SessionFilter::default()).unwrap().is_empty());
    assert_eq!(fx.store.spend_between(0, i64::MAX).unwrap().calls, 0);
    assert_eq!(fx.store.marker(crate::import_v1::IMPORT_MARKER).unwrap(), None);

    let real = fx.import();
    assert_eq!(
        (dry.sessions, dry.messages, dry.problems.len()),
        (real.sessions, real.messages, real.problems.len())
    );
    assert_eq!(dry.problems, real.problems);
    assert!(fx.store.marker(crate::import_v1::IMPORT_MARKER).unwrap().is_some());
    // The old directory is untouched by both.
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn a_corrupt_file_is_reported_and_the_others_still_import() {
    let fx = Fixture::new();
    fx.write(
        "sessions/1763560000001-0001.json",
        "{\"id\": \"1763560000001-0001\", \"messages\": [",
    );
    fx.write_json("sessions/1763560000002-0002.json", &json!({"id": "1763560000002-0002"}));
    fx.write_json(
        "sessions/1763560000003-0003.json",
        &json!({"id": "1763560000003-0003", "messages": [{"role": "user", "content": [{"Video": "x"}]}]}),
    );
    fx.write_json(
        "sessions/1763560000004-0004.json",
        &old_session("1763560000004-0004", json!([user_text("fine")])),
    );
    fx.write("sessions/.hidden.json.tmp", "ignored");
    fx.write("sessions/notes.txt", "ignored");

    let report = fx.import();
    assert_eq!((report.sessions.imported, report.sessions.failed), (1, 3));
    let items: Vec<&str> = report.problems.iter().map(|p| p.item.as_str()).collect();
    assert_eq!(
        items,
        [
            "sessions/1763560000001-0001.json",
            "sessions/1763560000002-0002.json",
            "sessions/1763560000003-0003.json"
        ]
    );
    assert!(
        report.problems[0].reason.contains("not valid JSON"),
        "{}",
        report.problems[0].reason
    );
    assert!(
        report.problems[1].reason.contains("messages"),
        "{}",
        report.problems[1].reason
    );
    assert!(fx.store.session_by_legacy_id("1763560000004-0004").unwrap().is_some());
    assert!(fx.store.session_by_legacy_id("1763560000003-0003").unwrap().is_none());
}

#[test]
fn a_session_left_half_written_is_replaced_not_duplicated() {
    let fx = Fixture::new();
    fx.write_json(&format!("sessions/{LEGACY}.json"), &full_session(&fx));
    // An earlier run died after the header and one message, before it marked the session.
    let mut leftover = crate::tests::session(id().as_str(), "Leftover", 5);
    leftover.origin = SessionOrigin::Imported;
    fx.store.upsert_session(&leftover).unwrap();
    fx.store
        .append_message(&leftover.id, crate::tests::stored("stale", 6))
        .unwrap();

    let report = fx.import();
    assert_eq!(report.sessions.imported, 1);
    let s = fx.store.session(&id()).unwrap().unwrap();
    assert_eq!((s.title.as_str(), s.message_count), ("Fix the thing", 6));
    assert_eq!(fx.store.messages(&id(), 1..100).unwrap().len(), 6);
}

#[test]
fn a_session_with_no_text_left_imports_with_no_messages() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("  ")])),
    );
    let report = fx.import();
    assert_eq!((report.sessions.imported, report.messages.imported), (1, 0));
    let s = fx
        .store
        .session(&SessionId::new("v1-1763560000001-0001"))
        .unwrap()
        .unwrap();
    assert_eq!((s.title.as_str(), s.message_count), ("Session 1763560000001-0001", 0));
}

#[test]
fn a_missing_sessions_directory_is_not_a_problem() {
    let fx = Fixture::new();
    let report = fx.import();
    assert!(report.problems.is_empty());
    assert_eq!(report.imported_total(), 0);
}
