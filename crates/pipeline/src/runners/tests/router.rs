use model::{AskRule, IssueType, NodeKind, RouterNode, RouterRule};

use super::*;
use crate::runners::router::yes_no;

fn rule(id: &str, when: &str, ask: Option<&str>) -> RouterRule {
    RouterRule {
        id: id.into(),
        label: id.to_uppercase(),
        when: when.into(),
        ask: ask.map(|q| AskRule { question: q.into(), workdir: "docs".into(), ..Default::default() }),
    }
}

fn router(rules: Vec<RouterRule>) -> NodeKind {
    NodeKind::Router(RouterNode { rules })
}

#[test]
fn conditions_are_tried_in_order_and_else_catches_the_rest() {
    let fx = Fx::new();
    let runner = fx.runner(Vec::new());
    let rules = vec![rule("bugs", r#"type == "bug""#, None), rule("crash", r#"title contains "retry""#, None)];
    let (result, _) = run(&runner, &job("triage", router(rules.clone()), 0, 1));
    assert!(matches!(&result, JobResult::Done { port, output } if port == "crash" && output.summary.contains("CRASH")), "{result:?}");

    let mut bug = job("triage", router(rules), 0, 1);
    bug.item.issue.as_mut().unwrap().kind = IssueType::Bug;
    assert!(matches!(run(&runner, &bug).0, JobResult::Done { port, .. } if port == "bugs"));

    let (none, _) = run(&runner, &job("triage", router(vec![rule("x", r#"title contains "zzz""#, None)]), 0, 1));
    assert!(matches!(none, JobResult::Done { port, .. } if port == "else"));

    let (bad, _) = run(&runner, &job("triage", router(vec![rule("x", "nonsense ==", None)]), 0, 1));
    assert!(matches!(&bad, JobResult::Failed { error, .. } if error.contains("bad condition")), "{bad:?}");
}

#[test]
fn a_question_is_answered_yes_or_no_and_anything_else_fails() {
    let fx = Fx::new();
    let docs = fx.repo("docs", &[("README.md", "hi\n")]);
    let mut wd = workdir("docs", &docs, "");
    wd.worktree = false;
    let runner = fx.runner(vec![wd]);
    let rules = vec![rule("ui", "", Some("Does it change the UI?"))];
    fx.questions.answers.lock().unwrap().extend(["Yes.", "no", "maybe"]);

    let (yes, notes) = run(&runner, &job("triage", router(rules.clone()), 0, 1));
    assert!(matches!(&yes, JobResult::Done { port, output } if port == "ui" && output.summary.contains("because")), "{yes:?}");
    assert!((output(&yes).cost_usd - 0.01).abs() < 1e-9);
    assert!(notes.iter().any(|n| matches!(n, JobNote::Spent { .. })));
    let asked = fx.questions.asked.lock().unwrap()[0].clone();
    assert_eq!(asked.workdir.as_deref(), Some(docs.as_path()));
    assert!(asked.item.contains("HAR-7: Fix the retry"));

    assert!(matches!(run(&runner, &job("triage", router(rules.clone()), 0, 1)).0, JobResult::Done { port, .. } if port == "else"));
    let (garbage, _) = run(&runner, &job("triage", router(rules), 0, 1));
    assert!(matches!(&garbage, JobResult::Failed { error, .. } if error.contains("maybe")), "{garbage:?}");
}

#[test]
fn answers_are_read_strictly() {
    assert_eq!(yes_no(" YES "), Some(true));
    assert_eq!(yes_no("No."), Some(false));
    assert_eq!(yes_no("yes, because"), None);
    assert_eq!(yes_no(""), None);
    assert_eq!(yes_no("nope"), None);
}
