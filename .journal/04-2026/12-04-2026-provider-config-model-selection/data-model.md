# Data Model — Provider Configuration

## config.json (stored in `%APPDATA%/scarllet/config.json`)

### Schema

```json
{
  "active_provider": "<string: name of the selected provider, or empty>",
  "providers": [
    {
      "name": "<string: unique provider identifier>",
      "api_key": "<string: bearer token for the API>",
      "api_url": "<string: base URL, e.g. https://openrouter.ai/api/v1>",
      "models": ["<string: model ID>", "..."],
      "active_model": "<string: model ID currently selected for this provider>"
    }
  ]
}
```

### Example — two providers configured, OpenRouter active

```json
{
  "active_provider": "openrouter",
  "providers": [
    {
      "name": "openrouter",
      "api_key": "sk-or-v1-abc123",
      "api_url": "https://openrouter.ai/api/v1",
      "models": ["openai/gpt-4o", "anthropic/claude-sonnet-4", "google/gemini-2.0-flash"],
      "active_model": "openai/gpt-4o"
    },
    {
      "name": "local-ollama",
      "api_key": "",
      "api_url": "http://localhost:11434/v1",
      "models": ["llama3", "codellama"],
      "active_model": "llama3"
    }
  ]
}
```

### Empty default (auto-created when file does not exist)

```json
{
  "active_provider": "",
  "providers": []
}
```

## Rust structs (`scarllet-sdk/src/config.rs`)

Replaces the current `ScarlletConfig` + `ProviderCredential`.

```rust
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
}
```

### Convenience method

```rust
impl ScarlletConfig {
    pub fn active_provider(&self) -> Option<&Provider> {
        if self.active_provider.is_empty() {
            return None;
        }
        self.providers.iter().find(|p| p.name == self.active_provider)
    }
}
```

## Migration

The old format (`credentials: HashMap<String, ProviderCredential>`) is dropped entirely. No automatic migration — the old `credentials` field is simply ignored via `#[serde(default)]`. Users with an existing config will get the empty defaults for the new fields and must reconfigure manually.

## Storage

| Aspect | Decision |
|--------|----------|
| Location | `%APPDATA%/scarllet/config.json` (unchanged) |
| Auto-create | Yes — Core creates the file with empty defaults on startup if missing |
| Invalid JSON fallback | Log error, use empty defaults, do not overwrite the broken file |
| Persistence | Core writes to disk only when explicitly saving (manual edit for now) |
