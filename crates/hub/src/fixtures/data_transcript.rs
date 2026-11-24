//! Stored messages: the "Fix double charge on retry" transcript of the chat mockup, a long
//! ledger conversation for paging, and short exchanges for the other chats.

use std::collections::HashMap;

use crate::api::{
    BlobId, ContentBlock, DiffHunk, DiffLine, DiffLineKind, FileDiff, Message, ModelRef, Role, SessionId, StoredMessage,
    ToolDisplay, Usage,
};

use super::data_sessions::{FIX_1423, LEDGER, model};
use super::{HOUR, MINUTE, World};

pub fn stored(seq: u64, at: i64, role: Role, content: Vec<ContentBlock>, model: Option<ModelRef>) -> StoredMessage {
    let assistant = role == Role::Assistant;
    StoredMessage {
        seq,
        at,
        message: Message { role, content },
        compacted: false,
        model,
        usage: if assistant { Usage { input: 1_200, output: 240, cache_read: 30_000, cache_write: 900 } } else { Usage::default() },
        cost_usd: if assistant { 0.02 } else { 0.0 },
    }
}

pub fn text(text: &str) -> ContentBlock {
    ContentBlock::text(text)
}

/// A tool call whose input is written as JSON text.
pub fn tool_use(id: &str, name: &str, input: &str) -> ContentBlock {
    ContentBlock::ToolUse { id: id.to_string(), name: name.to_string(), input: input.parse().unwrap_or_default() }
}

pub fn tool_result(id: &str, content: &str) -> ContentBlock {
    ContentBlock::ToolResult { tool_use_id: id.to_string(), content: content.to_string(), is_error: false }
}

pub fn fill(world: &mut World) {
    let now = world.now;
    world.messages.insert(SessionId::new(FIX_1423), fix_1423(now));
    world.tool_displays.insert(SessionId::new(FIX_1423), HashMap::from([("t7".to_string(), retry_edit())]));
    world.messages.insert(SessionId::new(LEDGER), ledger(now));
    let short = [
        ("chat-split-checkout", "Plan how to split the checkout service into cart and payment.", "Two seams look clean: the cart API and the payment intent flow. I would start with the cart."),
        ("chat-refund-timeout", "Why do refunds with many line items time out?", "Each line item is a separate gateway call. Batching by 50 should keep us under the 10s timeout."),
        ("chat-retry-budget", "Plan a retry budget per merchant.", "I split it in two: Analyst writes the spec, Fixer implements the budget table."),
        ("chat-checkout-flake", "Why does checkout.spec flake on Safari?", "The 3-D Secure iframe loads after the test clicks Pay. Waiting for its load event fixes it."),
        ("chat-sdk-v9", "Bump the gateway SDK to v9.", "Done. Two call sites changed; the PR is open."),
        ("chat-pci-audit", "Audit PCI log redaction in payments-api.", "Stopped: the sentry MCP token expired before I could read the log sinks."),
        ("chat-1423-explore", "Find every caller of retryCharge.", "4 callers: checkout, subscriptions, invoices and the webhook retry job."),
        ("chat-1423-general", "Read the gateway idempotency docs.", "Keys live 24h; a repeated key returns the first response without charging again."),
        ("chat-1436-spec", "Write the retry budget spec.", "Spec written to out/PAY-1436/spec.md."),
        ("chat-1436-code", "Implement the retry budget table.", "Working on the migration."),
        ("chat-1436-migration", "Read the SDK v9 diff for Codex.", "Three breaking changes, all in webhooks."),
    ];
    for (id, question, answer) in short {
        let at = world.session(&SessionId::new(id)).map_or(now, |session| session.created);
        world.messages.insert(
            SessionId::new(id),
            vec![
                stored(1, at, Role::User, vec![text(question)], None),
                stored(2, at + 2 * MINUTE, Role::Assistant, vec![text(answer)], Some(model("opus-5.5"))),
            ],
        );
    }
}

const OLD_RETRY: &str = r#"export async function retryCharge(charge: Charge) {\n  const res = await gateway.charge(charge.amount, charge.card);\n  if (res.status === 'timeout') {\n    return retryCharge(charge);\n  }"#;
const NEW_RETRY: &str = r#"export async function retryCharge(charge: Charge) {\n  const key = charge.idempotencyKey ?? deriveKey(charge);\n  const res = await gateway.charge(charge.amount, charge.card, { idempotencyKey: key });\n  if (res.status === 'timeout') {\n    return retryCharge({ ...charge, idempotencyKey: key });\n  }"#;

/// The edit of `t7` as the live stream showed it: three lines in, two out, with line numbers
/// and the unchanged lines around them.
fn retry_edit() -> ToolDisplay {
    use DiffLineKind::{Add, Context, Del};
    let rows: [(DiffLineKind, Option<u32>, Option<u32>, &str); 9] = [
        (Context, Some(44), Some(44), "export async function retryCharge(charge: Charge) {"),
        (Del, Some(45), None, "  const res = await gateway.charge(charge.amount, charge.card);"),
        (Add, None, Some(45), "  const key = charge.idempotencyKey ?? deriveKey(charge);"),
        (Add, None, Some(46), "  const res = await gateway.charge(charge.amount, charge.card, { idempotencyKey: key });"),
        (Context, Some(46), Some(47), "  if (res.status === 'timeout') {"),
        (Del, Some(47), None, "    return retryCharge(charge);"),
        (Add, None, Some(48), "    return retryCharge({ ...charge, idempotencyKey: key });"),
        (Context, Some(48), Some(49), "  }"),
        (Context, Some(49), Some(50), "  return res;"),
    ];
    let lines = rows
        .into_iter()
        .map(|(kind, old_no, new_no, text)| DiffLine { kind, old_no, new_no, text: text.to_string(), no_newline: false })
        .collect();
    ToolDisplay::Diff(FileDiff {
        path: "src/charges/retry.ts".to_string(),
        added: 3,
        removed: 2,
        hunks: vec![DiffHunk { lines }],
        ..FileDiff::default()
    })
}

fn fix_1423(now: i64) -> Vec<StoredMessage> {
    let opus = Some(model("opus-5.5"));
    let retry_file: Vec<String> = (1..=58).map(|n| format!("{n:>3}  // retry.ts line {n}")).collect();
    let agents_md: Vec<String> = (1..=32).map(|n| format!("AGENTS.md line {n}")).collect();
    let grep = "src/charges/retry.ts:45: gateway.charge(charge.amount, charge.card)\n\
                src/charges/charge.ts:88: gateway.charge(amount, card)\n\
                src/charges/charge.ts:131: gateway.charge(amount, card, opts)\n\
                src/subscriptions/renew.ts:52: gateway.charge(plan.price, sub.card)\n\
                src/invoices/pay.ts:40: gateway.charge(invoice.total, card)\n\
                src/webhooks/retry-job.ts:19: gateway.charge(job.amount, job.card)";
    vec![
        stored(
            1,
            now - 2 * HOUR,
            Role::User,
            vec![
                ContentBlock::Image {
                    blob: BlobId::new("blob-checkout-double-charge"),
                    media_type: "image/png".to_string(),
                    name: "checkout-double-charge.png".to_string(),
                    bytes: 412 * 1024,
                },
                text("Fix the double charge on retry in @src/charges/retry.ts. Add a test that fails before the fix."),
            ],
            None,
        ),
        stored(
            2,
            now - HOUR,
            Role::Assistant,
            vec![
                text("I’ll check how retries reach the gateway, then add an idempotency key and pin it with a test."),
                tool_use("t1", "read_file", r#"{"path":"src/charges/retry.ts"}"#),
                tool_use("t2", "grep", r#"{"pattern":"gateway.charge\\("}"#),
                tool_use("t3", "read_file", r#"{"path":"AGENTS.md"}"#),
            ],
            opus.clone(),
        ),
        stored(
            3,
            now - HOUR + MINUTE,
            Role::User,
            vec![tool_result("t1", &retry_file.join("\n")), tool_result("t2", grep), tool_result("t3", &agents_md.join("\n"))],
            None,
        ),
        stored(
            4,
            now - HOUR + 2 * MINUTE,
            Role::Assistant,
            vec![
                tool_use("t4", "spawn_agent", r#"{"agent":"@explore","task":"Find every caller of retryCharge"}"#),
                tool_use("t5", "spawn_agent", r#"{"agent":"@general","task":"Read the gateway idempotency docs"}"#),
            ],
            opus.clone(),
        ),
        stored(
            5,
            now - HOUR + 4 * MINUTE,
            Role::User,
            vec![
                tool_result("t4", "4 callers: checkout, subscriptions, invoices and the webhook retry job."),
                tool_result("t5", "Keys live 24h; a repeated key returns the first response without charging again."),
            ],
            None,
        ),
        stored(
            6,
            now - HOUR + 5 * MINUTE,
            Role::Assistant,
            vec![tool_use(
                "t6",
                "todowrite",
                r#"{"items":[{"text":"Find every caller of retryCharge","status":"done"},{"text":"Add an idempotency key to retries","status":"done"},{"text":"Write a test that fails on main","status":"done"},{"text":"Run the charges test suite","status":"pending"}]}"#,
            )],
            opus.clone(),
        ),
        stored(7, now - HOUR + 5 * MINUTE, Role::User, vec![tool_result("t6", "Todo list updated")], None),
        stored(
            8,
            now - 30 * MINUTE,
            Role::Assistant,
            vec![tool_use(
                "t7",
                "edit_file",
                &format!(r#"{{"path":"src/charges/retry.ts","old_string":"{OLD_RETRY}","new_string":"{NEW_RETRY}"}}"#),
            )],
            opus.clone(),
        ),
        stored(9, now - 30 * MINUTE, Role::User, vec![tool_result("t7", "Applied 1 edit · LSP typescript: 0 errors")], None),
        stored(
            10,
            now - 12 * MINUTE,
            Role::Assistant,
            vec![
                text("Retries now reuse one idempotency key, so the gateway drops the duplicate charge. Running the charges suite next."),
                tool_use("t8", "bash", r#"{"command":"bun test src/charges"}"#),
            ],
            opus,
        ),
    ]
}

/// 96 messages; the first 30 were summarised away.
fn ledger(now: i64) -> Vec<StoredMessage> {
    let start = now - 2 * 24 * HOUR - 4 * HOUR;
    let topics = [
        "settlement files", "payout batches", "currency rounding", "chargeback holds", "fee lines", "reserve releases",
        "refund reversals", "bank holidays",
    ];
    (0..96u64)
        .map(|i| {
            let topic = topics[(i / 2) as usize % topics.len()];
            let at = start + i as i64 * MINUTE;
            let mut message = if i % 2 == 0 {
                stored(i + 1, at, Role::User, vec![text(&format!("How does reconciliation handle {topic}? (question {})", i / 2 + 1))], None)
            } else {
                stored(
                    i + 1,
                    at,
                    Role::Assistant,
                    vec![text(&format!("For {topic}, the ledger matches each bank line to one journal entry by amount and value date, then flags what is left over."))],
                    Some(model("sonnet-5.5")),
                )
            };
            message.compacted = i < 30;
            message
        })
        .collect()
}
