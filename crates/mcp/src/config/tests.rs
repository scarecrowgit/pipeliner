use super::*;

fn env(name: &str) -> Option<String> {
    match name {
        "HOME" => Some("/home/test".into()),
        "TOKEN" => Some("s3cret-token".into()),
        "EMPTY" => Some(String::new()),
        _ => None,
    }
}

fn x(text: &str) -> (String, Vec<String>, Vec<String>) {
    let (mut missing, mut secrets) = (Vec::new(), Vec::new());
    let out = expand(text, &env, &mut missing, &mut secrets);
    (out, missing, secrets)
}

struct Fixture {
    dir: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::create_dir_all(&paths.home).unwrap();
        Self { dir, paths }
    }

    fn config(&self, text: &str) {
        std::fs::write(&self.paths.config_file, text).unwrap();
    }

    fn claude(&self, text: &str) {
        std::fs::write(self.paths.home.join(".claude.json"), text).unwrap();
    }

    fn workspace(&self, mcp_json: &str) -> PathBuf {
        let ws = self.dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join(".mcp.json"), mcp_json).unwrap();
        ws
    }

    fn load(&self, ws: Option<&Path>) -> Loaded {
        load_server_configs(&self.paths, ws, &env)
    }
}

#[test]
fn expansion_rules() {
    assert_eq!(x("a ${TOKEN} b").0, "a s3cret-token b");
    assert_eq!(x("${NOPE:-fallback}").0, "fallback");
    assert_eq!(x("${TOKEN:-fallback}").0, "s3cret-token");
    assert_eq!(x("${EMPTY:-fallback}").0, "", "a set but empty variable wins over the default");
    assert_eq!(x("${NOPE:-}").0, "");
    assert_eq!(x("$TOKEN ${unterminated").0, "$TOKEN ${unterminated");
    assert_eq!(x("x${HOME}/y${TOKEN}").0, "x/home/test/ys3cret-token");
    let (out, missing, _) = x("k=${NOPE}!");
    assert_eq!(out, "k=${NOPE}!");
    assert_eq!(missing, vec!["NOPE".to_string()]);
    // HOME is not a secret; TOKEN is.
    assert_eq!(x("${HOME}/${TOKEN}").2, vec!["s3cret-token".to_string()]);
}

#[test]
fn config_toml_entries() {
    let f = Fixture::new();
    f.config(
        r#"
        default_model = "x"
        [[providers]]
        id = "p"
        [[mcp_servers]]
        name = "fs"
        command = "npx"
        args = ["-y", "server", "${HOME}", "--token=${TOKEN}"]
        env = { KEY = "${TOKEN}", OTHER = "${NOPE:-d}", LITERAL = "plain-literal" }
        cwd = "~/proj"
        auto_approve = ["read_file"]
        [[mcp_servers]]
        name = "off"
        command = "x"
        enabled = false
        [[mcp_servers]]
        name = "bad"
        [[mcp_servers]]
        name = "  "
        command = "x"
        [[mcp_servers]]
        name = "fs"
        command = "dup"
        [[mcp_servers]]
        name = "nocmd"
        command = " "
        "#,
    );
    let loaded = f.load(None);
    let servers = &loaded.servers;
    assert_eq!(servers.len(), 2, "{servers:?}");
    let fs = &servers[0];
    assert_eq!(fs.name, "fs");
    assert_eq!(fs.command, "npx");
    assert_eq!(fs.args, vec!["-y", "server", "/home/test", "--token=s3cret-token"]);
    assert_eq!(fs.env["KEY"], "s3cret-token");
    assert_eq!(fs.env["OTHER"], "d");
    assert_eq!(fs.cwd, Some(f.paths.home.join("proj")), "`~` is the app's home");
    assert!(fs.enabled);
    assert!(fs.auto_approves("read_file") && !fs.auto_approves("write_file"));
    assert_eq!(fs.source, ServerSource::ConfigToml);
    assert!(!servers[1].enabled);
    let w = &loaded.warnings;
    assert_eq!(w.len(), 4, "{w:?}"); // missing command field, empty name, duplicate, blank command
    assert!(w.iter().any(|m| m.contains("#3")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("defined twice")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("empty name")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("`nocmd`: empty `command`")), "{w:?}");
}

#[test]
fn secrets_stay_out_of_debug_and_redacted_text() {
    let f = Fixture::new();
    f.config(
        r#"
        [[mcp_servers]]
        name = "gh"
        command = "gh-mcp"
        args = ["--token=${TOKEN}", "${HOME}/x"]
        env = { GITHUB_TOKEN = "ghp_literal_value", SHORT = "1" }
        "#,
    );
    let cfg = &f.load(None).servers[0];
    let debug = format!("{cfg:?}");
    assert!(!debug.contains("s3cret-token"), "{debug}");
    assert!(!debug.contains("ghp_literal_value"), "{debug}");
    assert!(debug.contains("GITHUB_TOKEN") && debug.contains("/home/test/x"), "{debug}");
    let text = cfg.redact("auth failed for ghp_literal_value and s3cret-token in /home/test, 1 try");
    assert_eq!(text, "auth failed for *** and *** in /home/test, 1 try");
}

#[test]
fn invalid_toml_is_a_warning() {
    let f = Fixture::new();
    f.config("[[mcp_servers]\nname=");
    let loaded = f.load(None);
    assert!(loaded.servers.is_empty());
    assert_eq!(loaded.warnings.len(), 1);
    assert!(loaded.warnings[0].contains("invalid TOML"), "{:?}", loaded.warnings);

    f.config("mcp_servers = 3\nimport_claude_code = \"yes\"");
    let loaded = f.load(None);
    assert!(loaded.servers.is_empty());
    assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
}

#[test]
fn mcp_json_entries_and_skipped_transports() {
    let f = Fixture::new();
    let ws = f.workspace(
        r#"{"mcpServers": {
            "a": {"command": "node", "args": ["srv.js", "--token=${TOKEN}", 3], "env": {"N": 5, "K": "${MISSING}", "O": {}}, "autoApprove": ["*"]},
            "b": {"type": "stdio", "command": "b", "disabled": true},
            "c": {"command": "c", "cwd": "sub"},
            "web": {"type": "http", "url": "https://example.invalid/mcp"},
            "sse": {"type": "sse", "url": "https://example.invalid/sse"},
            "remote": {"url": "https://example.invalid/mcp"},
            "nocmd": {"args": []},
            "notobj": 7,
            "": {"command": "x"}
        }}"#,
    );
    let loaded = f.load(Some(&ws));
    let names: Vec<_> = loaded.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
    let a = &loaded.servers[0];
    assert_eq!(a.args, vec!["srv.js", "--token=s3cret-token", "3"]);
    assert_eq!(a.env["N"], "5");
    assert_eq!(a.env["K"], "${MISSING}");
    assert!(!a.env.contains_key("O"));
    assert_eq!(a.cwd.as_deref(), Some(ws.as_path()), "runs in the workspace");
    assert!(a.auto_approves("anything"));
    assert_eq!(a.source, ServerSource::McpJson);
    assert!(!loaded.servers[1].enabled);
    assert_eq!(loaded.servers[2].cwd, Some(ws.join("sub")));
    let w = &loaded.warnings;
    assert_eq!(w.iter().filter(|m| m.contains("only stdio")).count(), 3, "{w:?}");
    assert!(w.iter().any(|m| m.contains("(http)")) && w.iter().any(|m| m.contains("(sse)")));
    assert!(w.iter().any(|m| m.contains("`MISSING` is not set")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("no `command`")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("empty name")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("not an object")), "{w:?}");
    assert!(w.iter().any(|m| m.contains("env `O`")), "{w:?}");
}

#[test]
fn malformed_mcp_json_is_a_warning() {
    let f = Fixture::new();
    let ws = f.workspace("{oops");
    let loaded = f.load(Some(&ws));
    assert!(loaded.servers.is_empty());
    assert_eq!(loaded.warnings.len(), 1);
    assert!(loaded.warnings[0].contains("invalid JSON"));

    let ws = f.workspace(r#"{"mcpServers": []}"#);
    assert_eq!(f.load(Some(&ws)).warnings.len(), 1);
    let ws = f.workspace("{}");
    let loaded = f.load(Some(&ws));
    assert!(loaded.servers.is_empty() && loaded.warnings.is_empty());
}

#[test]
fn precedence_across_sources() {
    let f = Fixture::new();
    f.config(
        r#"
        import_claude_code = true
        [[mcp_servers]]
        name = "shared"
        command = "from-config"
        "#,
    );
    let ws = f.workspace(r#"{"mcpServers": {"shared": {"command": "from-project"}, "proj": {"command": "p"}}}"#);
    f.claude(
        r#"{"numStartups": 3, "projects": {}, "mcpServers": {
            "proj": {"command": "from-claude"}, "user": {"command": "u", "args": ["${NOPE:-dflt}"]},
            "remote": {"type": "http", "url": "https://example.invalid"}}}"#,
    );
    let loaded = f.load(Some(&ws));
    let summary: Vec<_> =
        loaded.servers.iter().map(|s| (s.name.as_str(), s.command.as_str(), s.source)).collect();
    assert_eq!(
        summary,
        vec![
            ("shared", "from-config", ServerSource::ConfigToml),
            ("proj", "p", ServerSource::McpJson),
            ("user", "u", ServerSource::ClaudeJson),
        ]
    );
    assert_eq!(loaded.servers[2].cwd, None);
    assert_eq!(loaded.servers[2].args, vec!["dflt"]);
    let w = &loaded.warnings;
    assert_eq!(w.iter().filter(|m| m.contains("already defined")).count(), 2, "{w:?}");
    assert!(w.iter().any(|m| m.contains("`proj` from ~/.claude.json ignored: already defined in .mcp.json")));
    assert_eq!(w.iter().filter(|m| m.contains("only stdio")).count(), 1, "{w:?}");
}

#[test]
fn claude_json_needs_the_import_flag() {
    let f = Fixture::new();
    f.claude(r#"{"mcpServers": {"user": {"command": "u"}}}"#);
    assert!(f.load(None).servers.is_empty(), "no config.toml: no import");
    f.config("import_claude_code = false\n");
    assert!(f.load(None).servers.is_empty());
    f.config("import_claude_code = true\n");
    let loaded = f.load(None);
    assert_eq!(loaded.servers.len(), 1);
    assert_eq!(loaded.servers[0].source.label(), "~/.claude.json");
}

#[test]
fn nothing_configured() {
    let f = Fixture::new();
    let loaded = f.load(Some(&f.dir.path().join("missing")));
    assert!(loaded.servers.is_empty() && loaded.warnings.is_empty());
}
