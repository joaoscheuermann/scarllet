use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScarlletConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub providers: Vec<Provider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Openai,
    Gemini,
}

impl Default for ProviderType {
    fn default() -> Self {
        Self::Openai
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub name: String,
    #[serde(rename = "type", default)]
    pub provider_type: ProviderType,
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

impl Provider {
    pub fn active_model_config(&self) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == self.model)
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        self.active_model_config()
            .and_then(|m| m.reasoning.as_deref())
    }
}

impl ScarlletConfig {
    pub fn active_provider(&self) -> Option<&Provider> {
        if self.provider.is_empty() {
            return None;
        }
        self.providers.iter().find(|p| p.name == self.provider)
    }
}

pub fn config_path() -> PathBuf {
    let config_dir = dirs::config_dir().expect("could not determine OS config directory");
    config_dir.join("scarllet").join("config.json")
}

pub fn load() -> io::Result<ScarlletConfig> {
    let path = config_path();
    if !path.exists() {
        let config = ScarlletConfig::default();
        save(&config)?;
        return Ok(config);
    }
    let contents = std::fs::read_to_string(&path)?;
    match serde_json::from_str(&contents) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            tracing::warn!("Invalid config.json, using defaults: {e}");
            Ok(ScarlletConfig::default())
        }
    }
}

pub fn save(config: &ScarlletConfig) -> io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    std::fs::write(&path, json)
}

#[cfg(test)]
mod tests {
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
        };
        assert!(config.active_provider().is_none());
    }

    #[test]
    fn deserialize_missing_fields_uses_defaults() {
        let json = r#"{}"#;
        let config: ScarlletConfig = serde_json::from_str(json).unwrap();
        assert!(config.provider.is_empty());
        assert!(config.providers.is_empty());
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
}
