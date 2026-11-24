use model::{Issue, IssueKey, IssueStatus, IssueType, Priority, ProjectId, SourceKind, StartTriggers};

use super::*;

fn issue(kind: IssueType, priority: Priority, status: IssueStatus) -> Issue {
    Issue {
        key: IssueKey::new("PAY-1423"),
        project: ProjectId::new("pay"),
        title: "Checkout crashes on submit".into(),
        body: "Panic in the card form".into(),
        status,
        priority,
        kind,
        owner: "Ivan".into(),
        repo: Some("payments-api".into()),
        created: 0,
        updated: 0,
        filed_by: None,
        runner: None,
        triggers: StartTriggers::default(),
        run: None,
        run_count: 0,
        pr_url: None,
        images: Vec::new(),
    }
}

fn item(issue: Option<Issue>) -> Item {
    Item {
        title: "Checkout crashes on submit".into(),
        text: "Panic in the card form".into(),
        issue,
        source: SourceKind::Issue,
    }
}

fn bug_high() -> Item {
    item(Some(issue(IssueType::Bug, Priority::High, IssueStatus::Todo)))
}

#[test]
fn conditions_evaluate_over_an_issue() {
    let cases: &[(&str, bool)] = &[
        (r#"type == "bug""#, true),
        (r#"type == "Bug""#, true),
        (r#"type != "bug""#, false),
        (r#"type == "story""#, false),
        (r#"priority >= "high""#, true),
        (r#"priority > "medium""#, true),
        (r#"priority < "high""#, false),
        (r#"priority <= "low""#, false),
        (r#"type == "bug" && priority >= "high""#, true),
        (r#"type == "story" || priority == "high""#, true),
        (r#"type == "story" or priority == "low""#, false),
        (r#"!(status == "done")"#, true),
        (r#"not status == "todo""#, false),
        (r#"status < "in progress""#, true),
        (r#"status >= "in_review""#, false),
        (r#"title contains "CRASH""#, true),
        (r#"body contains "panic""#, true),
        (r#"text matches "*card*""#, true),
        (r#"title matches "checkout*""#, true),
        (r#"title matches "crash""#, false),
        (r#"owner == "ivan""#, true),
        (r#"assignee == "someone""#, false),
        (r#"key == "pay-1423""#, true),
        (r#"repo == "payments-api""#, true),
        (r#"source == "issue""#, true),
        (r#"source == "manual""#, false),
        (r#"(type == "story" || type == "bug") and (priority == "high")"#, true),
        (r#"type == "story" || type == "bug" && priority == "low""#, false),
        (r#"!!(type == "bug")"#, true),
        (r#"title == 'Checkout crashes on submit'"#, true),
        (r#"title contains "\"""#, false),
    ];
    let item = bug_high();
    for (text, want) in cases {
        let expr = parse(text).unwrap_or_else(|err| panic!("{text}: {err}"));
        assert_eq!(expr.eval(&item), *want, "{text}");
    }
}

#[test]
fn and_binds_tighter_than_or() {
    let expr = parse(r#"type == "story" || type == "bug" && priority == "low""#).unwrap();
    assert!(matches!(expr, Expr::Or(_, _)));
}

#[test]
fn without_an_issue_issue_fields_are_empty() {
    let item = item(None);
    let cases: &[(&str, bool)] = &[
        (r#"type == "bug""#, false),
        (r#"type != "bug""#, true),
        (r#"priority >= "low""#, false),
        (r#"owner == """#, false),
        (r#"owner contains "i""#, false),
        (r#"title contains "crash""#, true),
        (r#"source == "issue""#, true),
    ];
    for (text, want) in cases {
        assert_eq!(parse(text).unwrap().eval(&item), *want, "{text}");
    }
}

#[test]
fn errors_name_a_position_and_a_reason() {
    let cases: &[(&str, usize, &str)] = &[
        ("", 0, "empty"),
        ("   ", 0, "empty"),
        (r#"labels contains "ui""#, 0, "Unknown field “labels”; known fields: type, priority"),
        (r#"type = "bug""#, 5, "Use =="),
        (r#"type == bug"#, 8, "Put the value in quotes"),
        (r#"type == "bug"#, 8, "not closed"),
        (r#"type == "feature""#, 8, "Unknown type “feature”; one of bug, story, chore, docs"),
        (r#"priority >= "urgent""#, 12, "one of low, medium, high"),
        (r#"title >= "a""#, 9, "Only priority and status can use >="),
        (r#"type == "bug" &&"#, 16, "ends too early"),
        (r#"(type == "bug""#, 0, "not closed"),
        (r#"type == "bug")"#, 13, "Expected && or ||"),
        (r#"type == "bug" priority == "high""#, 14, "Expected && or ||"),
        (r#"type"#, 4, "Expected a comparison after type"),
        (r#"type contains"#, 13, "Expected a quoted value"),
        (r#"== "bug""#, 0, "Expected a field"),
        (r#"type == "bug" & x"#, 14, "Use &&"),
        (r#"type == "bug" # x"#, 14, "Unexpected “#”"),
        (r#"title matches "[a""#, 14, "Not a valid pattern"),
        (r#"type == "bug" && () "#, 18, "Expected a field"),
    ];
    for (text, pos, needle) in cases {
        let err = parse(text).expect_err(text);
        assert_eq!(err.pos, *pos, "{text}: {err}");
        assert!(err.message.contains(needle), "{text}: {err}");
    }
}

#[test]
fn errors_display_a_one_based_column() {
    let err = parse(r#"type = "bug""#).unwrap_err();
    assert_eq!(err.to_string(), "Use == to compare (at 6)");
}
