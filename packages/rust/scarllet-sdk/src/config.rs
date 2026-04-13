use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScarlletConfig {
    #[serde(default)]
    pub active_provider: String,
    #[serde(default)]
    pub providers: Vec<Provider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    pub api_key: String,
    pub api_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub active_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
}

impl ScarlletConfig {
    pub fn active_provider(&self) -> Option<&Provider> {
        if self.active_provider.is_empty() {
            return None;
        }
        self.providers
            .iter()
            .find(|p| p.name == self.active_provider)
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
    fn roundtrip() {
        let config = ScarlletConfig {
            active_provider: "openrouter".into(),
            providers: vec![Provider {
                name: "openrouter".into(),
                api_key: "sk-test".into(),
                api_url: "https://openrouter.ai/api/v1".into(),
                models: vec!["gpt-4o".into(), "claude-sonnet".into()],
                active_model: "gpt-4o".into(),
                reasoning_effort: Some("high".into()),
                extra_body: None,
            }],
        };
        let json = serde_json::to_string(&config).unwrap();
        let loaded: ScarlletConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.active_provider, "openrouter");
        assert_eq!(loaded.providers[0].api_key, "sk-test");
        assert_eq!(loaded.providers[0].active_model, "gpt-4o");
    }

    #[test]
    fn empty_default() {
        let config = ScarlletConfig::default();
        assert!(config.active_provider.is_empty());
        assert!(config.providers.is_empty());
    }

    #[test]
    fn active_provider_found() {
        let config = ScarlletConfig {
            active_provider: "local".into(),
            providers: vec![Provider {
                name: "local".into(),
                api_key: String::new(),
                api_url: "http://localhost:11434/v1".into(),
                models: vec!["llama3".into()],
                active_model: "llama3".into(),
                reasoning_effort: None,
                extra_body: None,
            }],
        };
        let p = config.active_provider().unwrap();
        assert_eq!(p.name, "local");
        assert_eq!(p.active_model, "llama3");
    }

    #[test]
    fn active_provider_empty_name() {
        let config = ScarlletConfig::default();
        assert!(config.active_provider().is_none());
    }

    #[test]
    fn active_provider_not_in_list() {
        let config = ScarlletConfig {
            active_provider: "missing".into(),
            providers: vec![],
        };
        assert!(config.active_provider().is_none());
    }

    #[test]
    fn deserialize_missing_fields_uses_defaults() {
        let json = r#"{}"#;
        let config: ScarlletConfig = serde_json::from_str(json).unwrap();
        assert!(config.active_provider.is_empty());
        assert!(config.providers.is_empty());
    }
}
