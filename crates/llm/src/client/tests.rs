use super::*;
use crate::NoBlobs;

fn provider(id: &str, kind: ProviderKind, models: &[&str], key: Option<&str>) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        kind,
        base_url: "http://127.0.0.1:9".into(),
        api_key: key.map(String::from),
        models: models.iter().map(|m| m.to_string()).collect(),
    }
}

#[test]
fn unknown_providers_and_empty_models_are_unknown_models() {
    let client = Providers::new(Config {
        providers: vec![provider("a", ProviderKind::Anthropic, &[], Some("k"))],
        default_model: None,
    });
    let mut events = Vec::new();
    let err = client
        .stream(&ModelRef::parse("nope/x"), &ChatRequest::default(), &NoBlobs, &CancelToken::new(), &mut |e| events.push(e))
        .unwrap_err();
    assert_eq!(err, LlmError::UnknownModel("nope/x".into()));
    let err = client
        .stream(&ModelRef::new("a", ""), &ChatRequest::default(), &NoBlobs, &CancelToken::new(), &mut |e| events.push(e))
        .unwrap_err();
    assert!(matches!(err, LlmError::UnknownModel(_)));
    assert!(matches!(client.list_models("nope"), Err(LlmError::UnknownModel(_))));
    assert!(events.is_empty());
}

#[test]
fn an_already_cancelled_token_never_connects() {
    let client = Providers::new(Config {
        providers: vec![provider("a", ProviderKind::OpenAi, &[], None)],
        default_model: None,
    });
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = client.stream(&ModelRef::parse("a/m"), &ChatRequest::default(), &NoBlobs, &cancel, &mut |_| {}).unwrap_err();
    assert_eq!(err, LlmError::Cancelled);
}

#[test]
fn providers_and_default_model() {
    let mut config = Config {
        providers: vec![
            provider("local", ProviderKind::OpenAi, &[], None),
            provider("anthropic", ProviderKind::Anthropic, &["claude-opus-5-5", "claude-haiku-5-5"], Some("secret")),
        ],
        default_model: None,
    };
    let client = Providers::new(config.clone());
    assert_eq!(
        client.providers(),
        vec![
            ProviderInfo {
                id: "local".into(),
                kind: "openai".into(),
                base_url: "http://127.0.0.1:9".into(),
                models: vec![],
                has_key: false,
            },
            ProviderInfo {
                id: "anthropic".into(),
                kind: "anthropic".into(),
                base_url: "http://127.0.0.1:9".into(),
                models: vec!["claude-opus-5-5".into(), "claude-haiku-5-5".into()],
                has_key: true,
            },
        ]
    );
    assert_eq!(client.default_model(), Some(ModelRef::new("anthropic", "claude-opus-5-5")));
    assert!(!format!("{client:?}").contains("secret"));

    config.default_model = Some(ModelRef::parse("local/qwen3"));
    assert_eq!(Providers::new(config).default_model(), Some(ModelRef::new("local", "qwen3")));
    assert_eq!(Providers::new(Config::default()).default_model(), None);
}
