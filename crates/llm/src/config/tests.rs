use super::*;
use std::collections::HashMap;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |k| map.get(k).cloned()
}

fn no_probe() -> bool {
    panic!("the Ollama probe must not run here")
}

#[test]
fn parses_the_old_file_format_and_ignores_foreign_keys() {
    let env = env_of(&[("MY_KEY", "sk-secret")]);
    let config = Config::parse(
        r#"
        default_model = "nr/cc/claude-opus-5-5"
        import_claude_code = true

        [[providers]]
        id = "anthropic"
        kind = "anthropic"
        base_url = "https://proxy.example.com/v1/"
        api_key_env = "MY_KEY"
        models = ["claude-sonnet-5-5"]
        colour = "blue"

        [[providers]]
        id = "local"
        kind = "openai"
        base_url = "http://127.0.0.1:8080/v1/"

        [[providers]]
        id = "inline"
        kind = "openai"
        api_key = "k"
        api_key_env = "MISSING"

        [[mcp_servers]]
        name = "fs"
        command = "mcp-fs"
        "#,
        &env,
    )
    .unwrap();
    assert_eq!(config.default_model, Some(ModelRef::new("nr", "cc/claude-opus-5-5")));
    assert_eq!(
        config.providers,
        vec![
            ProviderConfig {
                id: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                base_url: "https://proxy.example.com".into(),
                api_key: Some("sk-secret".into()),
                models: vec!["claude-sonnet-5-5".into()],
            },
            ProviderConfig {
                id: "local".into(),
                kind: ProviderKind::OpenAi,
                base_url: "http://127.0.0.1:8080/v1".into(),
                api_key: None,
                models: vec![],
            },
            ProviderConfig {
                id: "inline".into(),
                kind: ProviderKind::OpenAi,
                base_url: OPENAI_URL.into(),
                api_key: Some("k".into()),
                models: vec![],
            },
        ]
    );
    assert_eq!(Config::parse("", &env).unwrap(), Config::default());
}

#[test]
fn rejects_bad_files_without_quoting_keys() {
    let env = env_of(&[]);
    assert!(Config::parse("[[providers]]\nid = \"x\"\nkind = \"gemini\"", &env).is_err());
    let dup = "[[providers]]\nid='a'\nkind='openai'\n[[providers]]\nid='a'\nkind='anthropic'";
    assert!(Config::parse(dup, &env).unwrap_err().to_string().contains("twice"));
    assert!(Config::parse("[[providers]]\nid='a/b'\nkind='openai'", &env).is_err());

    let broken = "[[providers]]\nid = 'a'\nkind = 'openai'\napi_key = \"sk-live-123\" oops\n";
    let err = Config::parse(broken, &env).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("line 4"), "{text}");
    assert!(!text.contains("sk-live-123"), "{text}");
}

#[test]
fn debug_redacts_keys() {
    let config = Config::parse("[[providers]]\nid='a'\nkind='openai'\napi_key='sk-live-123'", &env_of(&[])).unwrap();
    let text = format!("{config:?}");
    assert!(!text.contains("sk-live-123"), "{text}");
    assert!(text.contains("<redacted>"), "{text}");
}

#[test]
fn load_reads_the_file_and_treats_a_missing_one_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let config = Config::load_with(&paths, &env_of(&[]), &|| false).unwrap();
    assert_eq!(config, Config::default());

    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(&paths.config_file, "providers = 3").unwrap();
    let err = Config::load_with(&paths, &env_of(&[]), &|| false).unwrap_err();
    assert!(format!("{err:#}").contains("config.toml"), "{err:#}");

    std::fs::write(&paths.config_file, "default_model = 'openai/gpt-5'").unwrap();
    let config = Config::load_with(&paths, &env_of(&[("OPENAI_API_KEY", "o")]), &|| false).unwrap();
    assert_eq!(config.default_model, Some(ModelRef::new("openai", "gpt-5")));
    assert_eq!(config.providers.len(), 1);
}

#[test]
fn env_detection_follows_the_old_rules() {
    let env = env_of(&[
        ("ANTHROPIC_API_KEY", "a-key"),
        ("ANTHROPIC_BASE_URL", "http://claude-code-proxy"),
        ("OPENAI_API_KEY", "o-key"),
        ("OPENAI_BASE_URL", "https://gateway.example.com/v1/"),
        ("OPENROUTER_API_KEY", "  "),
    ]);
    let mut config = Config::default();
    config.add_env_providers(&env, &|| true);
    let summary: Vec<_> = config
        .providers
        .iter()
        .map(|p| (p.id.as_str(), p.kind, p.base_url.as_str(), p.api_key.as_deref()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("anthropic", ProviderKind::Anthropic, "https://api.anthropic.com", Some("a-key")),
            ("openai", ProviderKind::OpenAi, "https://gateway.example.com/v1", Some("o-key")),
            ("ollama", ProviderKind::OpenAi, "http://localhost:11434/v1", None),
        ]
    );

    let mut config = Config::default();
    config.add_env_providers(&env_of(&[("OPENROUTER_API_KEY", "r")]), &|| false);
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].base_url, OPENROUTER_URL);
}

#[test]
fn file_providers_win_and_ollama_host_skips_the_probe() {
    let file = "[[providers]]\nid = 'openrouter'\nkind = 'openai'\nbase_url = 'https://x/v1'";
    let env = env_of(&[("OPENROUTER_API_KEY", "r"), ("OLLAMA_HOST", "0.0.0.0")]);
    let mut config = Config::parse(file, &env).unwrap();
    config.add_env_providers(&env, &no_probe);
    assert_eq!(config.providers.len(), 2);
    assert_eq!(config.providers[0].base_url, "https://x/v1");
    assert_eq!(config.providers[0].api_key, None);
    assert_eq!(config.providers[1].base_url, "http://localhost:11434/v1");

    let mut config = Config::parse("[[providers]]\nid='ollama'\nkind='openai'", &env_of(&[])).unwrap();
    config.add_env_providers(&env_of(&[]), &no_probe);
    assert_eq!(config.providers.len(), 1);

    let mut config = Config::default();
    config.add_env_providers(&env_of(&[]), &|| false);
    assert!(config.providers.is_empty());
}

#[test]
fn ollama_host_forms() {
    assert_eq!(ollama_base_url("0.0.0.0"), "http://localhost:11434/v1");
    assert_eq!(ollama_base_url("127.0.0.1:9999"), "http://127.0.0.1:9999/v1");
    assert_eq!(ollama_base_url("https://ollama.lan"), "https://ollama.lan/v1");
    assert_eq!(ollama_base_url("http://box:11434/"), "http://box:11434/v1");
    assert_eq!(ollama_base_url("http://box:11434/v1"), "http://box:11434/v1");
    assert_eq!(ollama_base_url(":8000"), "http://localhost:8000/v1");
}
