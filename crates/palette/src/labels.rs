//! The words and figures of the rows, the preview and the footer.

use base::{fmt, time};
use hub::api::{IndexStats, Millis, RunnableRef, Session, SessionState};
use ui::components::Status;

/// An agent's name as the mockups write it: `payments-lead` is `Payments lead`.
pub(crate) fn agent_name(runnable: &RunnableRef) -> String {
    let id = runnable.id().replace(['-', '_'], " ");
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The dot of a chat.
pub(crate) fn status(state: SessionState) -> Status {
    match state {
        SessionState::Idle => Status::Idle,
        SessionState::Running => Status::Running,
        SessionState::Waiting => Status::Waiting,
        SessionState::Failed => Status::Error,
        SessionState::Done => Status::Live,
    }
}

/// What a state is called next to its dot; idle chats say nothing.
pub(crate) fn state_word(state: SessionState) -> Option<&'static str> {
    match state {
        SessionState::Idle => None,
        SessionState::Running => Some("running"),
        SessionState::Waiting => Some("waiting for you"),
        SessionState::Failed => Some("failed"),
        SessionState::Done => Some("done"),
    }
}

/// The muted line under a title: `Build · payments-api · fix/PAY-1423 · PAY-1423`. Parts a
/// chat does not have are left out.
pub(crate) fn sub_line(session: &Session) -> String {
    let mut parts = vec![agent_name(&session.runnable)];
    if let Some(workdir) = &session.workdir {
        parts.push(workdir.repo.clone());
        parts.extend(workdir.branch.clone());
    }
    parts.extend(session.issue.as_ref().map(ToString::to_string));
    parts.join(" · ")
}

/// How long ago and what it cost: `12m · $0.21`.
pub(crate) fn age_and_cost(session: &Session, now: Millis, offset: i32) -> String {
    format!("{} · {}", time::ago(now, session.updated, offset), fmt::cost(session.cost_usd))
}

/// The line at the right of the search field: `2 shown · 96 match of 2,418`.
pub(crate) fn count_line(shown: usize, matching: u64, indexed: u64) -> String {
    format!("{} shown · {} match of {}", fmt::count(shown as u64), fmt::count(matching), fmt::count(indexed))
}

/// The footer's note: `Index: 2,418 chats · 61k messages · updated 2 min ago`.
pub(crate) fn index_line(index: &IndexStats, now: Millis, offset: i32) -> String {
    let mut line = format!("Index: {} · {} messages", fmt::plural(index.sessions, "chat", "chats"), fmt::tokens(index.messages));
    if let Some(updated) = index.updated {
        line.push_str(" · updated ");
        line.push_str(&ago_in_words(now, updated, offset));
    }
    line
}

/// `just now`, `2 min ago`, `3 h ago`, and the date after that.
pub(crate) fn ago_in_words(now: Millis, then: Millis, offset: i32) -> String {
    let short = time::ago(now, then, offset);
    if short == "now" {
        return "just now".to_string();
    }
    if let Some(minutes) = short.strip_suffix('m').filter(|n| n.chars().all(|c| c.is_ascii_digit())) {
        return format!("{minutes} min ago");
    }
    if let Some(hours) = short.strip_suffix('h').filter(|n| n.chars().all(|c| c.is_ascii_digit())) {
        return format!("{hours} h ago");
    }
    format!("on {short}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_read_as_words() {
        assert_eq!(agent_name(&RunnableRef::agent("build")), "Build");
        assert_eq!(agent_name(&RunnableRef::orchestrator("payments-lead")), "Payments lead");
    }

    #[test]
    fn relative_times_are_said_in_words() {
        let now = 1_795_000_000_000;
        assert_eq!(ago_in_words(now, now - 5_000, 0), "just now");
        assert_eq!(ago_in_words(now, now - 2 * 60_000, 0), "2 min ago");
        assert_eq!(ago_in_words(now, now - 3 * 3_600_000, 0), "3 h ago");
        assert!(ago_in_words(now, now - 30 * 86_400_000, 0).starts_with("on "));
    }

    #[test]
    fn the_count_line_uses_separators() {
        assert_eq!(count_line(2, 96, 2418), "2 shown · 96 match of 2,418");
    }
}
