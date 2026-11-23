//! Issue comments: `@mention` parsing, autocomplete matching and the wake logic
//! (which mentioned runners start a run, which don't, and Retry).
//!
//! Everything here is plain data in / data out so it can be unit-tested without
//! gpui entities.

use std::ops::Range;

use ui::images::ImageAttachment;

use crate::data::{
    self, Activity, Actor, Column, Issue, RUNNERS, Run, RunState, RunnerId, RunnerKind, Trigger,
    Wake, WakeOutcome, person_name,
};

/// Why the mock "runtime laptop" runner can't be woken.
pub const OFFLINE_REASON: &str = "runtime laptop offline since 14:02 · reconnect it or retry";

/// Why a mention doesn't wake when the issue's "On @mention" trigger is off.
pub const TRIGGER_OFF_REASON: &str = "On @mention is off for this issue · retry to wake it anyway";

/// Runners whose (mock) runtime is offline when the screen opens.
pub fn initially_offline() -> Vec<RunnerId> {
    RUNNERS
        .iter()
        .position(|r| r.id == "a-codex")
        .into_iter()
        .collect()
}

// ── Parsing ─────────────────────────────────────────────────────────────

/// A piece of comment text. Ranges are byte ranges into the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(Range<usize>),
    /// `@Name` (range covers the `@` and the name as typed).
    Mention(Range<usize>, RunnerId),
    /// Text between backticks (range excludes the backticks).
    Code(Range<usize>),
}

/// Runner whose name follows the `@` at the start of `rest`: longest name wins and must
/// end at the end of the text or before a non-alphanumeric char. Returns the id and the
/// byte length of the name.
fn mention_at(rest: &str) -> Option<(RunnerId, usize)> {
    RUNNERS
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            rest.is_char_boundary(r.name.len())
                && rest[..r.name.len()].eq_ignore_ascii_case(r.name)
                && !rest[r.name.len()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
        })
        .max_by_key(|(_, r)| r.name.len())
        .map(|(id, r)| (id, r.name.len()))
}

pub fn mention_spans(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut plain_start = 0;
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let special = match c {
            '`' => text[i + 1..]
                .find('`')
                .filter(|&n| n > 0)
                .map(|n| (Span::Code(i + 1..i + 1 + n), i + n + 2)),
            '@' if prev.is_none_or(char::is_whitespace) => mention_at(&text[i + 1..])
                .map(|(id, len)| (Span::Mention(i..i + 1 + len, id), i + 1 + len)),
            _ => None,
        };
        if let Some((span, end)) = special {
            if plain_start < i {
                spans.push(Span::Text(plain_start..i));
            }
            spans.push(span);
            plain_start = end;
            prev = text[..end].chars().next_back();
            i = end;
        } else {
            prev = Some(c);
            i += c.len_utf8();
        }
    }
    if plain_start < text.len() {
        spans.push(Span::Text(plain_start..text.len()));
    }
    spans
}

/// Mentioned runners, unique, in order of first appearance.
pub fn mentions(text: &str) -> Vec<RunnerId> {
    let mut out = Vec::new();
    for span in mention_spans(text) {
        if let Span::Mention(_, id) = span
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    out
}

/// The `@token` the cursor (assumed at the end) is typing: byte offset of the `@` and
/// the token after it.
pub fn mention_query(text: &str) -> Option<(usize, &str)> {
    let start = text.rfind(char::is_whitespace).map_or(0, |i| {
        i + text[i..].chars().next().map_or(1, char::len_utf8)
    });
    text[start..].strip_prefix('@').map(|token| (start, token))
}

/// Runners where some word of the name starts with `query`, with the matched byte range
/// inside the name. Empty query matches everything.
pub fn mention_matches(query: &str) -> Vec<(RunnerId, Range<usize>)> {
    let n = query.len();
    RUNNERS
        .iter()
        .enumerate()
        .filter_map(|(id, r)| {
            let mut offset = 0;
            for word in r.name.split(' ') {
                if word.is_char_boundary(n)
                    && word.len() >= n
                    && word[..n].eq_ignore_ascii_case(query)
                {
                    return Some((id, offset..offset + n));
                }
                offset += word.len() + 1;
            }
            None
        })
        .collect()
}

/// The runner is working this issue: it is the issue's runner, or the issue's runner is
/// an orchestrator / pipeline that runs it.
pub fn on_issue(issue: &Issue, id: RunnerId) -> bool {
    if issue.runner == Some(id) {
        return true;
    }
    issue
        .runner()
        .is_some_and(|r| r.kind != RunnerKind::Agent && r.desc.contains(data::runner(id).name))
}

// ── Posting and waking ──────────────────────────────────────────────────

/// Try to wake `id` for a comment by `by` (display name). On success starts a run on the
/// issue and appends the runner's reply. `force` ignores the "On @mention" trigger.
fn try_wake(
    issue: &mut Issue,
    id: RunnerId,
    by: &str,
    offline: &[RunnerId],
    force: bool,
) -> WakeOutcome {
    if offline.contains(&id) {
        return WakeOutcome::NotWoken {
            reason: OFFLINE_REASON.into(),
        };
    }
    let mention_ix = Trigger::ALL
        .iter()
        .position(|t| *t == Trigger::Mention)
        .unwrap_or(0);
    if !force && !issue.triggers[mention_ix] {
        return WakeOutcome::NotWoken {
            reason: TRIGGER_OFF_REASON.into(),
        };
    }
    let r = data::runner(id);
    issue.runs += 1;
    let mut reply = Activity::new(r.name, Actor::of(r.kind), "now", "Picking this up.");
    reply.woken_by = Some(by.to_string().into());
    issue.activity.push(reply);
    issue.runner = issue.runner.or(Some(id));
    issue.run = Some(Run::running(
        format!("{} working on your comment", r.name),
        3,
    ));
    issue.status = Column::Progress;
    WakeOutcome::Woken { run: issue.runs }
}

fn commenter(a: &Activity) -> String {
    person_name(&a.who).map_or_else(|| a.who.to_string(), str::to_string)
}

/// Post `text` as a comment by "ME": wakes every mentioned runner, or, with no mentions,
/// resumes a blocked run.
pub fn post(issue: &mut Issue, offline: &[RunnerId], text: &str, images: Vec<ImageAttachment>) {
    let mut comment = Activity::comment("ME", "now", text.to_string());
    comment.images = images;
    let by = commenter(&comment);
    issue.activity.push(comment);
    let comment_ix = issue.activity.len() - 1;

    let mentioned = mentions(text);
    for &id in &mentioned {
        let outcome = try_wake(issue, id, &by, offline, false);
        issue.activity[comment_ix].wakes.push(Wake {
            runner: id,
            outcome,
        });
    }

    if mentioned.is_empty()
        && let Some(r) = issue.runner()
        && issue
            .run
            .as_ref()
            .is_some_and(|run| run.state == RunState::Blocked)
    {
        issue.activity.push(Activity::agent(
            r.name,
            "now",
            "Thanks, continuing with that scope.",
        ));
        issue.run = Some(Run::running("resumed after your answer", 35));
    }
}

/// Retry a receipt (`wake_ix` of activity `activity_ix`). With `force` the runner's
/// runtime counts as reconnected (removed from `offline`) and the trigger toggle is
/// ignored. The receipt becomes `Woken` in place; the reply goes to the end of the feed.
pub fn wake(
    issue: &mut Issue,
    activity_ix: usize,
    wake_ix: usize,
    offline: &mut Vec<RunnerId>,
    force: bool,
) {
    let Some(a) = issue.activity.get(activity_ix) else {
        return;
    };
    let Some(w) = a.wakes.get(wake_ix) else {
        return;
    };
    if matches!(w.outcome, WakeOutcome::Woken { .. }) {
        return;
    }
    let id = w.runner;
    let by = commenter(a);
    if force {
        offline.retain(|o| *o != id);
    }
    let outcome = try_wake(issue, id, &by, offline, force);
    issue.activity[activity_ix].wakes[wake_ix].outcome = outcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> RunnerId {
        data::runner_by_name(name).expect("runner")
    }

    fn pay_1427() -> Issue {
        data::seed()
            .into_iter()
            .find(|i| i.key == "PAY-1427")
            .expect("PAY-1427")
    }

    #[test]
    fn multi_word_mention_is_case_insensitive() {
        let text = "@release captain ship it";
        assert_eq!(
            mention_spans(text),
            vec![
                Span::Mention(0..16, id("Release captain")),
                Span::Text(16..text.len())
            ]
        );
    }

    #[test]
    fn longest_name_wins() {
        // "PAY triage" is not a prefix of another name, but "Payments lead" and
        // "PAY triage" both start with "pay": the whole name decides.
        assert_eq!(
            mentions("@Payments lead and @pay triage"),
            vec![id("Payments lead"), id("PAY triage")]
        );
        assert!(mentions("@Payments").is_empty());
    }

    #[test]
    fn mention_must_follow_whitespace_and_end_the_word() {
        assert!(mentions("a@Fixer").is_empty());
        assert!(mentions("@Fixers").is_empty());
        assert_eq!(mentions("(@Fixer)"), Vec::<RunnerId>::new());
        assert_eq!(mentions("@Fixer, thanks"), vec![id("Fixer")]);
        assert_eq!(mentions("@Fixer@Fixer"), vec![id("Fixer")]);
    }

    #[test]
    fn mentions_are_unique_and_ordered() {
        assert_eq!(
            mentions("@Codex then @Fixer and @codex again"),
            vec![id("Codex"), id("Fixer")]
        );
    }

    #[test]
    fn code_spans_exclude_backticks() {
        let text = "Try `RefundBatcher` now";
        assert_eq!(
            mention_spans(text),
            vec![Span::Text(0..4), Span::Code(5..18), Span::Text(19..23)]
        );
        // No mention inside code; unclosed backtick stays text.
        assert!(mentions("`@Fixer`").is_empty());
        assert_eq!(mention_spans("a ` b"), vec![Span::Text(0..5)]);
    }

    #[test]
    fn query_cases() {
        assert_eq!(mention_query("hi @re"), Some((3, "re")));
        assert_eq!(mention_query("hi @re x"), None);
        assert_eq!(mention_query("mail@re"), None);
        assert_eq!(mention_query("@"), Some((0, "")));
        assert_eq!(mention_query("hi @"), Some((3, "")));
        assert_eq!(mention_query("hi @re "), None);
        assert_eq!(mention_query(""), None);
    }

    #[test]
    fn matches_by_word_prefix() {
        assert_eq!(
            mention_matches("re"),
            vec![(id("Reproducer"), 0..2), (id("Release captain"), 0..2)]
        );
        assert_eq!(mention_matches("").len(), 8);
        assert_eq!(mention_matches("CAP"), vec![(id("Release captain"), 8..11)]);
        assert!(mention_matches("zzz").is_empty());
    }

    #[test]
    fn on_issue_includes_pipeline_members() {
        let issue = pay_1427();
        assert!(on_issue(&issue, id("PAY triage")));
        assert!(on_issue(&issue, id("Fixer")));
        assert!(on_issue(&issue, id("Reproducer")));
        assert!(!on_issue(&issue, id("Codex")));
    }

    #[test]
    fn seed_receipts() {
        let issue = pay_1427();
        assert_eq!(issue.runs, 2);
        assert_eq!(issue.activity.iter().filter(|a| a.comment).count(), 2);
    }

    #[test]
    fn mention_wakes_and_counts_runs() {
        let mut issue = pay_1427();
        post(&mut issue, &initially_offline(), "@Fixer go on", Vec::new());
        let comment = issue.activity.iter().rev().nth(1).expect("comment");
        assert!(comment.comment);
        assert_eq!(
            comment.wakes,
            vec![Wake {
                runner: id("Fixer"),
                outcome: WakeOutcome::Woken { run: 3 }
            }]
        );
        let reply = issue.activity.last().expect("reply");
        assert_eq!(reply.who, "Fixer");
        assert_eq!(reply.woken_by.as_ref().map(|s| s.as_ref()), Some("You"));
        assert_eq!(issue.runs, 3);
        assert!(issue.is_running());
    }

    #[test]
    fn offline_runner_is_not_woken_and_retry_wakes_it() {
        let mut issue = pay_1427();
        let mut offline = initially_offline();
        post(&mut issue, &offline, "@Codex variant please", Vec::new());
        let ix = issue.activity.len() - 1;
        assert_eq!(issue.runs, 2);
        assert!(matches!(
            issue.activity[ix].wakes[0].outcome,
            WakeOutcome::NotWoken { .. }
        ));

        wake(&mut issue, ix, 0, &mut offline, true);
        assert!(offline.is_empty());
        assert_eq!(
            issue.activity[ix].wakes[0].outcome,
            WakeOutcome::Woken { run: 3 }
        );
        assert_eq!(issue.activity.last().map(|a| a.who.as_ref()), Some("Codex"));
        // Retrying a woken receipt does nothing.
        wake(&mut issue, ix, 0, &mut offline, true);
        assert_eq!(issue.runs, 3);
    }

    #[test]
    fn trigger_off_blocks_wake_until_forced() {
        let mut issue = pay_1427();
        issue.triggers[2] = false;
        let mut offline = Vec::new();
        post(&mut issue, &offline, "@Fixer and @Reproducer", Vec::new());
        let ix = issue.activity.len() - 1;
        assert_eq!(issue.activity[ix].wakes.len(), 2);
        assert!(
            issue.activity[ix]
                .wakes
                .iter()
                .all(|w| matches!(&w.outcome, WakeOutcome::NotWoken { reason } if reason.contains("On @mention is off")))
        );
        wake(&mut issue, ix, 1, &mut offline, true);
        assert_eq!(
            issue.activity[ix].wakes[1].outcome,
            WakeOutcome::Woken { run: 3 }
        );
    }

    #[test]
    fn plain_comment_resumes_blocked_run() {
        let mut issue = data::seed()
            .into_iter()
            .find(|i| i.key == "PAY-1435")
            .expect("PAY-1435");
        post(&mut issue, &[], "Datadog and Sentry only", Vec::new());
        assert_eq!(issue.run.as_ref().map(|r| r.pct), Some(35));
        assert_eq!(issue.runs, 1);
    }
}
