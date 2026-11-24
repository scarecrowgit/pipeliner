//! Reviews with their variants (A/B/C of "Double charge on retry"), Inbox items, and the diffs
//! a review shows.

use crate::api::{
    DiffHunk, DiffLine, DiffLineKind, FileChange, FileDiff, InboxId, InboxItem, InboxKind, IssueKey, Link, NodeId,
    Outcome, RequestId, Review, ReviewId, ReviewState, RunId, TestResult, Variant,
};

use super::{HOUR, MINUTE, World};

pub const REVIEW_1423: &str = "review-1423";

fn variant(index: u32, name: &str, approach: &str, files: &[(&str, u32, u32)], tests: (u32, u32), secs: u64, cost: f64) -> Variant {
    let files: Vec<FileChange> =
        files.iter().map(|(path, added, removed)| FileChange { path: path.to_string(), added: *added, removed: *removed }).collect();
    Variant { renders: Vec::new(),
        index,
        name: name.to_string(),
        approach: approach.to_string(),
        branch: Some(format!("fix/PAY-1423-{}", crate::api::variant_letter(index).to_lowercase())),
        worktree: None,
        session: None,
        added: files.iter().map(|file| file.added).sum(),
        removed: files.iter().map(|file| file.removed).sum(),
        files,
        tests: Some(TestResult { passed: tests.0, total: tests.1 }),
        tests_ok: Some(tests.0 == tests.1),
        secs,
        cost_usd: cost,
    }
}

#[expect(clippy::too_many_arguments, reason = "one row of the mockup's review table")]
fn review(id: &str, run: &str, issue: &str, title: &str, agent: &str, provenance: &[&str], created: i64, variants: Vec<Variant>) -> Review {
    Review {
        id: ReviewId::new(id),
        run: RunId::new(run),
        node: NodeId::new("review"),
        issue: Some(IssueKey::new(issue)),
        title: title.to_string(),
        state: ReviewState::Open,
        created,
        provenance: provenance.iter().map(|step| step.to_string()).collect(),
        agent: agent.to_string(),
        variants,
        renders: Vec::new(),
        feedback: Vec::new(),
        picked: None,
    }
}

pub fn fill(world: &mut World) {
    let now = world.now;
    world.reviews = vec![
        review(
            REVIEW_1423,
            "run-1423",
            "PAY-1423",
            "Double charge on retry",
            "Fixer",
            &["Jira", "Router · rule 1", "Fixer ×3", "You"],
            now - 40 * MINUTE,
            vec![
                variant(
                    1,
                    "Idempotency key on retry",
                    "Reuses one key across retries, so the gateway drops the second charge.",
                    &[("src/charges/retry.ts", 3, 2), ("src/charges/retry.test.ts", 12, 0), ("src/charges/keys.ts", 3, 2)],
                    (212, 212),
                    102,
                    0.21,
                ),
                variant(
                    2,
                    "Lock the row before charging",
                    "Holds a row lock for the whole gateway call. checkout.spec times out under load.",
                    &[("src/charges/retry.ts", 19, 7), ("src/db/locks.ts", 12, 2)],
                    (211, 212),
                    118,
                    0.24,
                ),
                variant(
                    3,
                    "Dedupe in the webhook handler",
                    "Smallest change, but API retries still charge twice. Covers webhooks only.",
                    &[("src/webhooks/charge.ts", 12, 2)],
                    (212, 212),
                    81,
                    0.18,
                ),
            ],
        ),
        review(
            "review-1427",
            "run-1427",
            "PAY-1427",
            "Refund webhook times out",
            "Reproducer",
            &["Jira", "Router · rule 2", "Reproducer", "You"],
            now - HOUR,
            vec![variant(
                1,
                "Minimal reproduction",
                "One refund with 420 line items times out at 11.8s; batching by 50 brings it to 1.9s.",
                &[("out/PAY-1427/repro.md", 34, 0)],
                (88, 88),
                540,
                0.10,
            )],
        ),
        review(
            "review-1431",
            "run-1431",
            "PAY-1431",
            "Saved cards for guest checkout",
            "Analyst",
            &["Jira", "Router · rule 3", "Analyst ×2", "You"],
            now - 3 * HOUR,
            vec![
                variant(1, "Tokenised guest cards", "Stores a gateway token keyed by email; no account needed.", &[("out/PAY-1431/spec.md", 61, 0)], (0, 0), 140, 0.15),
                variant(2, "Vault-only cards", "Keeps cards in the gateway vault and emails a magic link to reuse them.", &[("out/PAY-1431/spec.md", 48, 0)], (0, 0), 151, 0.16),
            ],
        ),
    ];
    let item = |id: &str, kind: InboxKind, title: &str, source: &str, issue: &str, ago: i64| InboxItem {
        id: InboxId::new(id),
        kind,
        title: title.to_string(),
        summary: String::new(),
        outcome: Some(Outcome::NeedsReview),
        source: source.to_string(),
        issue: Some(IssueKey::new(issue)),
        session: None,
        run: None,
        links: Vec::new(),
        created: now - ago,
        read: false,
        archived: false,
    };
    world.inbox = vec![
        InboxItem {
            run: Some(RunId::new("run-1423")),
            ..item("inbox-1423", InboxKind::Review { review: ReviewId::new(REVIEW_1423) }, "Double charge on retry", "Fixer · 3 variants", "PAY-1423", 40 * MINUTE)
        },
        item("inbox-1427", InboxKind::Review { review: ReviewId::new("review-1427") }, "Refund webhook times out", "Reproducer · 1 variant", "PAY-1427", HOUR),
        item("inbox-1431", InboxKind::Review { review: ReviewId::new("review-1431") }, "Saved cards for guest checkout", "Analyst · 2 variants", "PAY-1431", 3 * HOUR),
        InboxItem {
            outcome: Some(Outcome::Blocked),
            summary: "Which log sinks are in scope? I found **stdout**, **Datadog** and **S3 archives**.".to_string(),
            ..item("inbox-1435", InboxKind::Question { request: RequestId::new("ask-1435") }, "Which log sinks are in scope?", "Auditor · PAY-1435", "PAY-1435", 40 * MINUTE)
        },
        InboxItem {
            outcome: Some(Outcome::Done),
            read: true,
            summary: "Webhooks with a bad signature now get a 401. Added 6 tests.".to_string(),
            links: vec![Link { label: "PR #2280".to_string(), url: "https://github.com/acme/payments-api/pull/2280".to_string() }],
            ..item("inbox-1419", InboxKind::Report, "Webhook signature check shipped", "Fixer · PR #2280 merged", "PAY-1419", 5 * HOUR)
        },
    ];
}

/// The diff of one file of one variant: the mockup's retry.ts change for variant A, a plausible
/// hunk for anything else.
pub fn variant_diff(review: &Review, variant: u32, path: &str) -> Option<FileDiff> {
    let file = review.variants.iter().find(|v| v.index == variant)?.files.iter().find(|f| f.path == path)?;
    let line = |kind, old_no: Option<u32>, new_no: Option<u32>, text: &str| DiffLine {
        kind,
        old_no,
        new_no,
        text: text.to_string(),
        no_newline: false,
    };
    use DiffLineKind::*;
    let lines = if review.id.as_str() == REVIEW_1423 && variant == 1 && path == "src/charges/retry.ts" {
        vec![
            line(Context, Some(44), Some(44), "export async function retryCharge(charge: Charge) {"),
            line(Del, Some(45), None, "  const res = await gateway.charge(charge.amount, charge.card);"),
            line(Add, None, Some(45), "  const key = charge.idempotencyKey ?? deriveKey(charge);"),
            line(Add, None, Some(46), "  const res = await gateway.charge(charge.amount, charge.card, { idempotencyKey: key });"),
            line(Context, Some(46), Some(47), "  if (res.status === 'timeout') {"),
            line(Del, Some(47), None, "    return retryCharge(charge);"),
            line(Add, None, Some(48), "    return retryCharge({ ...charge, idempotencyKey: key });"),
            line(Context, Some(48), Some(49), "  }"),
            line(Context, Some(49), Some(50), "  return res;"),
            line(Context, Some(50), Some(51), "}"),
        ]
    } else {
        let mut lines = vec![line(Context, Some(1), Some(1), "// changed by the agent")];
        lines.extend((0..file.removed).map(|n| line(Del, Some(n + 2), None, &format!("old line {}", n + 1))));
        lines.extend((0..file.added).map(|n| line(Add, None, Some(n + 2), &format!("new line {}", n + 1))));
        lines
    };
    Some(FileDiff {
        path: path.to_string(),
        added: file.added,
        removed: file.removed,
        hunks: vec![DiffHunk { lines }],
        created: file.removed == 0,
        ..FileDiff::default()
    })
}
