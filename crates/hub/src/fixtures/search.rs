//! Session listing and search over the fixture's chats, with the store's semantics: scope
//! filters, facet counts within the scope, chip filters, newest first.

use std::ops::Range;

use crate::api::{
    FacetCounts, IndexStats, MessageSnippet, SearchResult, Session, SessionFilter, SessionHit, SessionId, SessionOrigin,
    SessionQuery, SessionState, StartedBy,
};

use super::World;

/// Where in the tree a filter looks. `ParentFilter` is not re-exported by `engine::api`, so it
/// is read from its debug form.
enum Parent {
    Any,
    TopLevel,
    Of(String),
}

fn parent(filter: &SessionFilter) -> Parent {
    let text = format!("{:?}", filter.parent);
    if text == "TopLevel" {
        return Parent::TopLevel;
    }
    match text.split('"').nth(1) {
        Some(id) if text.starts_with("Of(") => Parent::Of(id.to_string()),
        _ => Parent::Any,
    }
}

pub fn sessions(world: &World, filter: &SessionFilter) -> Vec<Session> {
    let parent = parent(filter);
    let mut list: Vec<Session> = world
        .sessions
        .iter()
        .filter(|s| filter.archived.is_none_or(|archived| s.archived == archived))
        .filter(|s| filter.pinned.is_none_or(|pinned| s.pinned == pinned))
        .filter(|s| filter.state.is_none_or(|state| s.state == state))
        .filter(|s| match &parent {
            Parent::Any => true,
            Parent::TopLevel => s.parent.is_none(),
            Parent::Of(id) => s.parent.as_ref().is_some_and(|p| p.as_str() == id),
        })
        .cloned()
        .collect();
    list.sort_by_key(|session| std::cmp::Reverse(session.updated));
    list.into_iter().skip(filter.offset).take(filter.limit.unwrap_or(usize::MAX)).collect()
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

fn highlights(text: &str, words: &[String]) -> Vec<Range<usize>> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for word in words {
        let mut from = 0;
        while let Some(at) = lower[from..].find(word.as_str()) {
            out.push(from + at..from + at + word.len());
            from += at + word.len();
        }
    }
    out.sort_by_key(|range| range.start);
    out
}

fn by_agent(session: &Session) -> bool {
    matches!(session.origin, SessionOrigin::Agent { .. } | SessionOrigin::Run { .. })
}

pub fn search(world: &World, query: &SessionQuery) -> SearchResult {
    let words = words(&query.text);
    let messages_match = |id: &SessionId| -> Option<MessageSnippet> {
        let messages = world.messages.get(id)?;
        messages.iter().find_map(|message| {
            let text = message.message.plain_text();
            let lower = text.to_lowercase();
            (!words.is_empty() && words.iter().all(|word| lower.contains(word.as_str()))).then(|| MessageSnippet {
                seq: message.seq,
                highlights: highlights(&text, &words),
                text,
            })
        })
    };
    let in_scope: Vec<(&Session, Option<MessageSnippet>)> = world
        .sessions
        .iter()
        .filter(|s| query.archived.is_none_or(|archived| s.archived == archived))
        .filter(|s| query.pinned.is_none_or(|pinned| s.pinned == pinned))
        .filter(|s| query.runnable.as_ref().is_none_or(|runnable| &s.runnable == runnable))
        .filter(|s| query.repo.as_ref().is_none_or(|repo| s.workdir.as_ref().is_some_and(|w| &w.repo == repo)))
        .filter_map(|s| {
            if words.is_empty() {
                return Some((s, None));
            }
            let title = s.title.to_lowercase();
            if words.iter().all(|word| title.contains(word.as_str())) {
                return Some((s, messages_match(&s.id)));
            }
            messages_match(&s.id).map(|snippet| (s, Some(snippet)))
        })
        .collect();
    let count = |pred: &dyn Fn(&Session) -> bool| in_scope.iter().filter(|(s, _)| pred(s)).count() as u64;
    let facets = FacetCounts {
        all: in_scope.len() as u64,
        mine: count(&|s| !by_agent(s)),
        started_by_agents: count(&by_agent),
        waiting: count(&|s| s.state == SessionState::Waiting),
        running: count(&|s| s.state == SessionState::Running),
        failed: count(&|s| s.state == SessionState::Failed),
        has_pr: count(&|s| s.pr_url.is_some()),
        from_issue: count(&|s| s.issue.is_some()),
    };
    let mut hits: Vec<(&Session, Option<MessageSnippet>)> = in_scope
        .into_iter()
        .filter(|(s, _)| query.state.is_none_or(|state| s.state == state))
        .filter(|(s, _)| match query.started_by {
            None => true,
            Some(StartedBy::Me) => !by_agent(s),
            Some(StartedBy::Agents) => by_agent(s),
        })
        .filter(|(s, _)| !query.has_pr || s.pr_url.is_some())
        .filter(|(s, _)| !query.from_issue || s.issue.is_some())
        .collect();
    hits.sort_by_key(|(session, _)| std::cmp::Reverse(session.updated));
    let total = hits.len() as u64;
    let hits = hits
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .map(|(session, snippet)| SessionHit {
            children: world.sessions.iter().filter(|child| child.parent.as_ref() == Some(&session.id)).count() as u32,
            title_highlights: highlights(&session.title, &words),
            snippet,
            session: session.clone(),
        })
        .collect();
    SearchResult {
        hits,
        total,
        facets,
        index: IndexStats {
            sessions: world.sessions.len() as u64,
            messages: world.messages.values().map(|messages| messages.len() as u64).sum(),
            updated: world.sessions.iter().map(|session| session.updated).max(),
        },
    }
}
