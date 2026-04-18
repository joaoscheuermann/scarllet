use super::*;

#[test]
fn roundtrip_openai() {
    let config = ScarlletConfig {
        provider: "openrouter".into(),
        providers: vec![Provider {
            name: "openrouter".into(),
            provider_type: ProviderType::Openai,
            api_key: "sk-test".into(),
            api_url: Some("https://openrouter.ai/api/v1".into()),
            model: "gpt-4o".into(),
            models: vec![
                ModelConfig {
                    id: "gpt-4o".into(),
                    reasoning: None,
                },
                ModelConfig {
                    id: "o3-mini".into(),
                    reasoning: Some("high".into()),
                },
            ],
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let loaded: ScarlletConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.provider, "openrouter");
    assert_eq!(loaded.providers[0].api_key, "sk-test");
    assert_eq!(loaded.providers[0].model, "gpt-4o");
    assert_eq!(loaded.providers[0].provider_type, ProviderType::Openai);
}

#[test]
fn roundtrip_gemini() {
    let config = ScarlletConfig {
        provider: "my-gemini".into(),
        providers: vec![Provider {
            name: "my-gemini".into(),
            provider_type: ProviderType::Gemini,
            api_key: "AIza-test".into(),
            api_url: None,
            model: "gemini-3.1-pro-preview".into(),
            models: vec![ModelConfig {
                id: "gemini-3.1-pro-preview".into(),
                reasoning: Some("high".into()),
            }],
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let loaded: ScarlletConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.provider, "my-gemini");
    assert_eq!(loaded.providers[0].provider_type, ProviderType::Gemini);
    assert!(loaded.providers[0].api_url.is_none());
    assert_eq!(
        loaded.providers[0].reasoning_effort(),
        Some("high")
    );
}

#[test]
fn empty_default() {
    let config = ScarlletConfig::default();
    assert!(config.provider.is_empty());
    assert!(config.providers.is_empty());
    assert_eq!(config.default_agent, "default");
}

#[test]
fn active_provider_found() {
    let config = ScarlletConfig {
        provider: "local".into(),
        providers: vec![Provider {
            name: "local".into(),
            provider_type: ProviderType::Openai,
            api_key: String::new(),
            api_url: Some("http://localhost:11434/v1".into()),
            model: "llama3".into(),
            models: vec![ModelConfig {
                id: "llama3".into(),
                reasoning: None,
            }],
        }],
        ..Default::default()
    };
    let p = config.active_provider().unwrap();
    assert_eq!(p.name, "local");
    assert_eq!(p.model, "llama3");
}

#[test]
fn active_provider_empty_name() {
    let config = ScarlletConfig::default();
    assert!(config.active_provider().is_none());
}

#[test]
fn active_provider_not_in_list() {
    let config = ScarlletConfig {
        provider: "missing".into(),
        providers: vec![],
        ..Default::default()
    };
    assert!(config.active_provider().is_none());
}

#[test]
fn deserialize_missing_fields_uses_defaults() {
    let json = r#"{}"#;
    let config: ScarlletConfig = serde_json::from_str(json).unwrap();
    assert!(config.provider.is_empty());
    assert!(config.providers.is_empty());
    assert_eq!(config.default_agent, "default");
}

#[test]
fn model_config_reasoning_lookup() {
    let provider = Provider {
        name: "test".into(),
        provider_type: ProviderType::Gemini,
        api_key: "key".into(),
        api_url: None,
        model: "gemini-pro".into(),
        models: vec![
            ModelConfig {
                id: "gemini-pro".into(),
                reasoning: Some("medium".into()),
            },
            ModelConfig {
                id: "gemini-flash".into(),
                reasoning: None,
            },
        ],
    };
    assert_eq!(provider.reasoning_effort(), Some("medium"));
    assert_eq!(provider.active_model_config().unwrap().id, "gemini-pro");
}

#[test]
fn default_agent_roundtrips_through_camelcase_json() {
    let config = ScarlletConfig {
        default_agent: "my-custom-agent".into(),
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        json.contains("\"defaultAgent\":\"my-custom-agent\""),
        "default_agent must serialize as `defaultAgent` (camelCase): {json}"
    );
    let loaded: ScarlletConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.default_agent, "my-custom-agent");
}

#[test]
fn deserialize_explicit_default_agent_field() {
    let json = r#"{ "defaultAgent": "scripted" }"#;
    let cfg: ScarlletConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.default_agent, "scripted");
}

#[test]
fn deserialize_legacy_config_without_default_agent_uses_default() {
    let json = r#"{
        "provider": "openrouter",
        "providers": [
            { "name": "openrouter", "type": "openai", "apiKey": "sk-x", "model": "gpt-4o" }
        ]
    }"#;
    let cfg: ScarlletConfig = serde_json::from_str(json).unwrap();
    assert_eq!(
        cfg.default_agent, "default",
        "legacy config files without defaultAgent must keep working"
    );
    assert_eq!(cfg.provider, "openrouter");
}

#[test]
fn deserialize_from_json_example() {
    let json = r#"{
        "provider": "gemini",
        "providers": [
            {
                "name": "gemini",
                "type": "gemini",
                "apiKey": "AIzaSy-test",
                "model": "gemini-3.1-pro-preview",
                "models": [
                    {
                        "id": "gemini-3.1-pro-preview",
                        "reasoning": "high"
                    }
                ]
            }
        ]
    }"#;
    let config: ScarlletConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.provider, "gemini");
    let p = config.active_provider().unwrap();
    assert_eq!(p.provider_type, ProviderType::Gemini);
    assert_eq!(p.api_key, "AIzaSy-test");
    assert!(p.api_url.is_none());
    assert_eq!(p.model, "gemini-3.1-pro-preview");
    assert_eq!(p.reasoning_effort(), Some("high"));
}
