//! The schema, as steps applied in order.
//!
//! A database's `user_version` is the number of steps applied to it, so a step is never edited
//! after it ships: add a new one instead. Conventions: timestamps are integer epoch
//! milliseconds, enums are their serde names (`waiting`, `in_progress`), booleans are 0 or 1,
//! nested structures are JSON text, and fields the app filters or sorts on are real columns.

/// Schema steps in order.
pub(crate) const MIGRATIONS: &[&str] = &[
    // 1: bookkeeping for the app itself (schema notes, one-time import markers).
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;",
    // 2: chats. `n` is a stable integer key for the full-text index; `id` is the session id.
    "
    CREATE TABLE sessions (
        n INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        legacy_id TEXT UNIQUE,
        title TEXT NOT NULL,
        parent TEXT,
        depth INTEGER NOT NULL,
        runnable TEXT NOT NULL,
        mode TEXT NOT NULL,
        model TEXT NOT NULL,
        workdir TEXT,
        repo TEXT,
        state TEXT NOT NULL,
        origin TEXT NOT NULL,
        by_agent INTEGER NOT NULL,
        issue TEXT,
        pinned INTEGER NOT NULL,
        archived INTEGER NOT NULL,
        created INTEGER NOT NULL,
        updated INTEGER NOT NULL,
        message_count INTEGER NOT NULL DEFAULT 0,
        cost_usd REAL NOT NULL,
        usage TEXT NOT NULL,
        context_tokens INTEGER NOT NULL,
        context_window INTEGER NOT NULL,
        forced_skills TEXT NOT NULL,
        pr_url TEXT,
        todos TEXT NOT NULL DEFAULT '[]',
        files TEXT NOT NULL DEFAULT '[]'
    ) STRICT;
    CREATE INDEX sessions_by_updated ON sessions (archived, updated DESC);
    CREATE INDEX sessions_by_parent ON sessions (parent);
    CREATE INDEX sessions_by_repo ON sessions (repo);
    CREATE INDEX sessions_by_issue ON sessions (issue);

    CREATE TABLE messages (
        id INTEGER PRIMARY KEY,
        session TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        at INTEGER NOT NULL,
        role TEXT NOT NULL,
        content TEXT NOT NULL,
        text TEXT NOT NULL,
        compacted INTEGER NOT NULL DEFAULT 0,
        model TEXT,
        usage TEXT NOT NULL,
        cost_usd REAL NOT NULL,
        UNIQUE (session, seq)
    ) STRICT;

    CREATE VIRTUAL TABLE messages_fts USING fts5 (
        text, content = 'messages', content_rowid = 'id', tokenize = 'unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
        INSERT INTO messages_fts (rowid, text) VALUES (new.id, new.text);
    END;
    CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
        INSERT INTO messages_fts (messages_fts, rowid, text) VALUES ('delete', old.id, old.text);
    END;

    CREATE VIRTUAL TABLE sessions_fts USING fts5 (
        title, content = 'sessions', content_rowid = 'n', tokenize = 'unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER sessions_fts_insert AFTER INSERT ON sessions BEGIN
        INSERT INTO sessions_fts (rowid, title) VALUES (new.n, new.title);
    END;
    CREATE TRIGGER sessions_fts_delete AFTER DELETE ON sessions BEGIN
        INSERT INTO sessions_fts (sessions_fts, rowid, title) VALUES ('delete', old.n, old.title);
    END;
    CREATE TRIGGER sessions_fts_update AFTER UPDATE OF title ON sessions BEGIN
        INSERT INTO sessions_fts (sessions_fts, rowid, title) VALUES ('delete', old.n, old.title);
        INSERT INTO sessions_fts (rowid, title) VALUES (new.n, new.title);
    END;
    ",
    // 3: work: issues and their feed, runs and their logs, reviews, the Inbox.
    "
    CREATE TABLE issue_counters (prefix TEXT PRIMARY KEY, last INTEGER NOT NULL) STRICT;

    CREATE TABLE issues (
        key TEXT PRIMARY KEY,
        project TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        status TEXT NOT NULL,
        priority TEXT NOT NULL,
        kind TEXT NOT NULL,
        owner TEXT NOT NULL,
        repo TEXT,
        created INTEGER NOT NULL,
        updated INTEGER NOT NULL,
        filed_by TEXT,
        runner TEXT,
        triggers TEXT NOT NULL,
        run TEXT,
        run_count INTEGER NOT NULL,
        pr_url TEXT,
        images TEXT NOT NULL
    ) STRICT;
    CREATE INDEX issues_by_project ON issues (project, updated DESC);

    CREATE TABLE issue_events (
        issue TEXT NOT NULL REFERENCES issues (key) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        at INTEGER NOT NULL,
        event TEXT NOT NULL,
        data TEXT NOT NULL,
        PRIMARY KEY (issue, seq)
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE runs (
        id TEXT PRIMARY KEY,
        pipeline TEXT NOT NULL,
        version INTEGER NOT NULL,
        issue TEXT,
        number INTEGER NOT NULL,
        title TEXT NOT NULL,
        input TEXT NOT NULL,
        state TEXT NOT NULL,
        open INTEGER NOT NULL,
        test INTEGER NOT NULL,
        started INTEGER NOT NULL,
        finished INTEGER,
        cost_usd REAL NOT NULL,
        usage TEXT NOT NULL,
        error TEXT,
        pr_url TEXT,
        nodes TEXT NOT NULL,
        renders TEXT NOT NULL DEFAULT '[]'
    ) STRICT;
    CREATE INDEX runs_by_issue ON runs (issue, started DESC);
    CREATE INDEX runs_open ON runs (started) WHERE open = 1;

    CREATE TABLE run_logs (
        run TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        at INTEGER NOT NULL,
        node TEXT,
        variant INTEGER NOT NULL,
        level TEXT NOT NULL,
        text TEXT NOT NULL,
        PRIMARY KEY (run, seq)
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE reviews (
        id TEXT PRIMARY KEY,
        run TEXT NOT NULL,
        node TEXT NOT NULL,
        issue TEXT,
        title TEXT NOT NULL,
        state TEXT NOT NULL,
        created INTEGER NOT NULL,
        agent TEXT NOT NULL,
        provenance TEXT NOT NULL,
        variants TEXT NOT NULL,
        renders TEXT NOT NULL,
        feedback TEXT NOT NULL,
        picked INTEGER
    ) STRICT;
    CREATE INDEX reviews_by_state ON reviews (state, created DESC);
    CREATE INDEX reviews_by_run ON reviews (run);

    CREATE TABLE inbox (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        target TEXT,
        title TEXT NOT NULL,
        summary TEXT NOT NULL,
        outcome TEXT,
        source TEXT NOT NULL,
        issue TEXT,
        session TEXT,
        run TEXT,
        links TEXT NOT NULL,
        created INTEGER NOT NULL,
        read INTEGER NOT NULL,
        archived INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX inbox_by_created ON inbox (archived, created DESC);
    ",
    // 4: the spend ledger, catalog state that is not in files, projects and settings.
    "
    CREATE TABLE usage_ledger (
        id INTEGER PRIMARY KEY,
        at INTEGER NOT NULL,
        session TEXT,
        run TEXT,
        node TEXT,
        runnable TEXT NOT NULL,
        name TEXT NOT NULL,
        created_by TEXT,
        model TEXT NOT NULL,
        input INTEGER NOT NULL,
        output INTEGER NOT NULL,
        cache_read INTEGER NOT NULL,
        cache_write INTEGER NOT NULL,
        cost_usd REAL NOT NULL,
        imported INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX usage_by_at ON usage_ledger (at);
    CREATE INDEX usage_by_session ON usage_ledger (session);
    CREATE INDEX usage_by_run ON usage_ledger (run, node);
    CREATE INDEX usage_by_runnable ON usage_ledger (runnable, at);

    CREATE TABLE pipeline_versions (
        pipeline TEXT NOT NULL,
        version INTEGER NOT NULL,
        state TEXT NOT NULL,
        def_text TEXT NOT NULL,
        hash TEXT NOT NULL,
        note TEXT NOT NULL,
        deployed_at INTEGER NOT NULL,
        PRIMARY KEY (pipeline, version)
    ) STRICT, WITHOUT ROWID;
    CREATE UNIQUE INDEX pipeline_versions_one_live ON pipeline_versions (pipeline) WHERE state = 'live';

    CREATE TABLE spawned_runnables (
        reference TEXT PRIMARY KEY,
        by_ref TEXT NOT NULL,
        run TEXT,
        session TEXT,
        ended INTEGER,
        data TEXT NOT NULL
    ) STRICT;
    CREATE INDEX spawned_by_run ON spawned_runnables (run);

    CREATE TABLE permission_rules (
        id INTEGER PRIMARY KEY,
        scope TEXT NOT NULL,
        scope_key TEXT NOT NULL,
        tool TEXT NOT NULL,
        pattern TEXT NOT NULL,
        decision TEXT NOT NULL,
        UNIQUE (scope, scope_key, tool, pattern)
    ) STRICT;

    CREATE TABLE autopilots (
        id TEXT PRIMARY KEY,
        project TEXT NOT NULL,
        name TEXT NOT NULL,
        trigger TEXT NOT NULL,
        target TEXT NOT NULL,
        prompt TEXT NOT NULL,
        enabled INTEGER NOT NULL,
        paused_reason TEXT,
        next_run INTEGER,
        last_run INTEGER,
        last_result TEXT NOT NULL,
        budget_usd REAL
    ) STRICT;
    CREATE INDEX autopilots_by_project ON autopilots (project);

    CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        key_prefix TEXT NOT NULL,
        repos TEXT NOT NULL
    ) STRICT;

    CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1), data TEXT NOT NULL) STRICT;
    ",
    // 5: deleted comments stay as tombstones so a seq is never handed out twice; what a tool
    // call showed (its `ToolDisplay`) next to its stored result; messages queued behind a
    // running turn; a run's graph and executor snapshot, moved out of the `meta` markers
    // `run-graph:<id>` and `run-snapshot:<id>`.
    "
    ALTER TABLE issue_events ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;

    CREATE TABLE tool_displays (
        session TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
        call TEXT NOT NULL,
        display TEXT NOT NULL,
        PRIMARY KEY (session, call)
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE queued_messages (
        session TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        text TEXT NOT NULL,
        images TEXT NOT NULL,
        PRIMARY KEY (session, position)
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE run_state (run TEXT PRIMARY KEY, graph TEXT, snapshot TEXT) STRICT;
    INSERT INTO run_state (run, graph)
        SELECT substr(key, 11), value FROM meta WHERE key LIKE 'run-graph:%';
    INSERT INTO run_state (run, snapshot)
        SELECT substr(key, 14), value FROM meta WHERE key LIKE 'run-snapshot:%'
        ON CONFLICT (run) DO UPDATE SET snapshot = excluded.snapshot;
    DELETE FROM meta WHERE key LIKE 'run-graph:%' OR key LIKE 'run-snapshot:%';
    ",
];
