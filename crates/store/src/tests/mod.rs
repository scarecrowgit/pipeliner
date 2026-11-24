//! Store tests: in-memory databases and temp directories only.

mod catalog;
mod extras;
mod issues;
mod ledger;
mod open;
mod search;
mod sessions;
mod work;

use std::path::PathBuf;

use model::{
    ChatMode, Message, ModelRef, RepoHost, RunnableRef, Session, SessionId, SessionOrigin, SessionState,
    StoredMessage, Usage, WorkMode, Workdir,
};

use crate::Store;

pub(crate) fn store() -> Store {
    Store::in_memory().unwrap()
}

pub(crate) fn session(id: &str, title: &str, updated: i64) -> Session {
    Session {
        id: SessionId::new(id),
        title: title.to_string(),
        parent: None,
        depth: 0,
        runnable: RunnableRef::agent("build"),
        mode: ChatMode::Build,
        model: ModelRef::parse("nr/cc/claude-opus-5-5"),
        workdir: None,
        state: SessionState::Idle,
        origin: SessionOrigin::Me,
        issue: None,
        pinned: false,
        archived: false,
        created: updated - 1000,
        updated,
        message_count: 0,
        cost_usd: 0.0,
        usage: Usage::default(),
        context_tokens: 0,
        context_window: 0,
        forced_skills: Vec::new(),
        pr_url: None,
    }
}

pub(crate) fn workdir(repo: &str) -> Workdir {
    Workdir {
        path: PathBuf::from(format!("/home/u/code/{repo}")),
        repo: repo.to_string(),
        branch: Some("main".into()),
        host: RepoHost::GitHub,
        mode: WorkMode::Worktree,
        source: None,
        runtime: "local".into(),
    }
}

pub(crate) fn stored(text: &str, at: i64) -> StoredMessage {
    StoredMessage {
        seq: 0,
        at,
        message: Message::user(text),
        compacted: false,
        model: None,
        usage: Usage::default(),
        cost_usd: 0.0,
    }
}

#[test]
fn the_store_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
}
