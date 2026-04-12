---
status: done
order: 4
created: 2026-04-10 19:48
title: "Credential store and configuration management"
---

## Description

Implement the global credential store in Core. On startup, Core reads a JSON configuration file from the OS-standard config directory. Credentials can be added or updated via the `SetCredential` gRPC RPC and retrieved via `GetCredentials`. Changes are persisted to disk immediately. This enables agents and the LLM library (Effort 8) to retrieve API keys without per-agent configuration.

## Objective

After starting Core, a gRPC client can call `SetCredential` with a provider name and API key. The credential is persisted to `scarllet/config.json` in the OS config directory. Restarting Core loads the credential from disk. `GetCredentials` returns the stored key for a given provider.

## Implementation Details

1. **`scarllet-proto` additions:**
   - `rpc GetCredentials(CredentialQuery) returns (CredentialResponse)` — query by provider name.
   - `rpc SetCredential(SetCredentialRequest) returns (SetCredentialResponse)` — upsert provider + key.
   - Message types: `CredentialQuery { provider: string }`, `CredentialResponse { provider, api_key, found }`, `SetCredentialRequest { provider, api_key }`, `SetCredentialResponse { success }`.
2. **`scarllet-sdk` config types:**
   - `ScarlletConfig` struct: contains `credentials: HashMap<String, ProviderCredential>` plus future-extensible fields.
   - `ProviderCredential { api_key: String }`.
   - Config file helpers: `load_config()`, `save_config()` at `dirs::config_dir()/scarllet/config.json`.
3. **`scarllet-core` credential store:**
   - On startup: call `load_config()`. If file missing, start with empty config.
   - In-memory `ScarlletConfig` behind `Arc<RwLock<...>>`.
   - `SetCredential` handler: update in-memory map, call `save_config()` to flush to disk.
   - `GetCredentials` handler: read from in-memory map, return credential or "not found".
4. **File format:**
   ```json
   {
     "credentials": {
       "openai": { "api_key": "sk-..." },
       "anthropic": { "api_key": "sk-ant-..." }
     }
   }
   ```

## Verification Criteria

- Start Core with no existing config file → starts normally (empty credentials).
- Call `SetCredential(provider="openai", api_key="sk-test-123")` → returns success.
- Verify `scarllet/config.json` exists in OS config directory with the credential.
- Call `GetCredentials(provider="openai")` → returns `api_key: "sk-test-123"`.
- Call `GetCredentials(provider="nonexistent")` → returns `found: false`.
- Restart Core → call `GetCredentials(provider="openai")` → still returns `"sk-test-123"` (persisted).
- `npx nx run scarllet-core:test` passes unit tests for config load/save and credential CRUD.

## Done

- Credentials persist across Core restarts via JSON config file.
- Observable via gRPC calls and inspecting the config file on disk.
