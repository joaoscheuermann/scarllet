use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// Top-level user configuration loaded from `config.json`.
///
/// Holds the list of LLM providers the user has set up, which one is
/// currently selected, and the name of the agent module spawned by default
/// when a session needs a main agent. Serialized with camelCase keys to
/// match the JSON schema exposed to end-users.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScarlletConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// Name of the agent module spawned for new turns (matches
    /// `ModuleManifest::name` of an agent registered via the watcher).
    #[serde(default = "default_agent_name")]
    pub default_agent: String,
}

impl Default for ScarlletConfig {
    /// Defaults to no providers and the canonical `"default"` agent module.
    fn default() -> Self {
        Self {
            provider: String::new(),
            providers: Vec::new(),
            default_agent: default_agent_name(),
        }
    }
}

/// Returns the canonical name of the bundled default agent module.
fn default_agent_name() -> String {
    "default".to_string()
}

/// Discriminator for the LLM API dialect a [`Provider`] speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    /// OpenAI-compatible chat-completions API (also used by OpenRouter, local
    /// inference servers, etc.).
    Openai,
    /// Google Gemini / Generative Language API.
    Gemini,
}

impl Default for ProviderType {
    /// Defaults to [`ProviderType::Openai`] since most third-party inference
    /// endpoints expose an OpenAI-compatible surface.
    fn default() -> Self {
        Self::Openai
    }
}

/// Per-model overrides within a provider (e.g. reasoning effort level).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// A configured LLM provider with its credentials, endpoint, and model list.
///
/// One provider entry can expose multiple models; the `model` field selects
/// which one is currently active.
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
    /// Returns the [`ModelConfig`] that matches the currently selected model id,
    /// or `None` if no match exists in the models list.
    pub fn active_model_config(&self) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == self.model)
    }

    /// Shortcut to the reasoning effort level configured on the active model
    /// (e.g. `"low"`, `"medium"`, `"high"`).
    pub fn reasoning_effort(&self) -> Option<&str> {
        self.active_model_config()
            .and_then(|m| m.reasoning.as_deref())
    }
}

impl ScarlletConfig {
    /// Looks up the provider whose name matches the current `provider` selector.
    ///
    /// Returns `None` when the selector is empty or no provider with that name
    /// exists in the list.
    pub fn active_provider(&self) -> Option<&Provider> {
        if self.provider.is_empty() {
            return None;
        }
        self.providers.iter().find(|p| p.name == self.provider)
    }
}

/// Builds the config file path relative to an arbitrary base directory.
///
/// Useful for testing or overriding the OS config root.
pub fn config_path_in(base: &std::path::Path) -> PathBuf {
    base.join("scarllet").join("config.json")
}

/// Returns the platform-standard path to `scarllet/config.json`.
pub fn config_path() -> PathBuf {
    config_path_in(&dirs::config_dir().expect("could not determine OS config directory"))
}

/// Loads the config from disk, creating a default file if none exists.
///
/// Malformed JSON is logged as a warning and falls back to defaults so the
/// application can still start.
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

/// Atomically writes the config to disk as pretty-printed JSON, creating
/// parent directories if needed.
pub fn save(config: &ScarlletConfig) -> io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    std::fs::write(&path, json)
}

#[cfg(test)]
mod tests;
