# Copilot Instructions — Provider Configuration & Model Selection

These instructions scope what an AI coding agent may and must do during implementation. Follow them exactly. Do not deviate without explicit human approval.

## Governing documents

Read these before writing any code:

- `ticket.md` — acceptance criteria and edge cases (the "what")
- `data-model.md` — config.json schema and Rust struct definitions
- `contracts.md` — gRPC protobuf changes (messages and RPCs)
- `plan.md` — implementation order and verification plan
- `decisions.md` — architectural decisions already approved

## Scope boundaries

### You MUST

- Follow the effort order in `plan.md` (config → proto → core → llm → agent).
- Use the exact struct shapes from `data-model.md`.
- Use the exact protobuf messages and RPC from `contracts.md`.
- Delete `gemini.rs` entirely — do not keep it behind a feature flag.
- Remove the `provider` field from `ChatRequest` in `scarllet-llm/src/types.rs`.
- Remove old credential RPCs (`GetCredentials`, `SetCredential`) and their messages from the proto.
- Remove old `ProviderCredential` and credentials `HashMap` from `scarllet-sdk/src/config.rs`.
- Ensure `config::load()` auto-creates the file with empty defaults when it does not exist.
- On invalid JSON in config, log a warning and return empty defaults — do NOT overwrite the file.
- Keep all tests compiling and passing after each effort.

### You MUST NOT

- Add new dependencies unless strictly required and approved.
- Add provider-specific adapters (everything goes through OpenAI-compatible).
- Add model validation in Core (pass `active_model` as-is).
- Add runtime config mutation (no `SetCredential` replacement) — editing is manual for now.
- Change the TUI rendering code — it already handles `SystemEvent` and `AgentError`.
- Introduce environment variable fallbacks for provider/model in the agent.
- Add migration logic for the old config format.

## Per-effort instructions

### Effort 1 — `scarllet-sdk/src/config.rs`

1. Replace `ScarlletConfig` and `ProviderCredential` with:

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

2. Add `active_provider()` method:

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

3. Update `load()`:
   - If file does not exist, call `save()` with the default config, then return it.
   - If JSON is invalid, log a warning via `tracing::warn!` and return `ScarlletConfig::default()`.

4. Replace tests:
   - Roundtrip test with the new struct (provider with name, api_key, api_url, models, active_model).
   - `active_provider()` returns `Some` when matched, `None` when empty, `None` when name not found.
   - Empty default has empty string and empty vec.

### Effort 2 — `scarllet-proto/proto/orchestrator.proto`

1. Remove these RPCs from the `Orchestrator` service:
   - `rpc GetCredentials(...)`
   - `rpc SetCredential(...)`

2. Remove these messages:
   - `CredentialQuery`
   - `CredentialResponse`
   - `SetCredentialRequest`
   - `SetCredentialResponse`

3. Add to the `Orchestrator` service:
   ```protobuf
   rpc GetActiveProvider(ActiveProviderQuery) returns (ActiveProviderResponse);
   ```

4. Add messages:
   ```protobuf
   message ActiveProviderQuery {}
   message ActiveProviderResponse {
     bool configured = 1;
     string provider_name = 2;
     string api_url = 3;
     string api_key = 4;
     string model = 5;
   }
   ```

### Effort 3 — `scarllet-core/src/main.rs`

1. Remove the `get_credentials` and `set_credential` implementations.

2. Implement `get_active_provider`:
   ```rust
   async fn get_active_provider(&self, _req: Request<ActiveProviderQuery>)
       -> Result<Response<ActiveProviderResponse>, Status>
   {
       let cfg = self.config.read().await;
       match cfg.active_provider() {
           Some(provider) => Ok(Response::new(ActiveProviderResponse {
               configured: true,
               provider_name: provider.name.clone(),
               api_url: provider.api_url.clone(),
               api_key: provider.api_key.clone(),
               model: provider.active_model.clone(),
           })),
           None => Ok(Response::new(ActiveProviderResponse {
               configured: false,
               ..Default::default()
           })),
       }
   }
   ```

3. In `route_prompt`, before the agent lookup, add a provider check:
   ```rust
   let cfg = self.config.read().await; // or pass config into route_prompt
   if cfg.active_provider().is_none() {
       let path = scarllet_sdk::config::config_path();
       let sys = CoreEvent {
           payload: Some(core_event::Payload::System(SystemEvent {
               message: format!(
                   "No provider configured. Edit config.json at {} to set up a provider.",
                   path.display()
               ),
           })),
       };
       session_registry.read().await.broadcast(sys);
       return;
   }
   ```
   Note: `route_prompt` is a free function — you will need to pass `config: &Arc<RwLock<ScarlletConfig>>` as a parameter.

4. Remove `ProviderCredential` from imports. Update the startup log to say provider count instead of credential count.

### Effort 4 — `scarllet-llm`

1. Delete `packages/rust/scarllet-llm/src/gemini.rs`.

2. In `lib.rs`, remove `pub mod gemini;`.

3. In `types.rs`, remove the `provider` field from `ChatRequest`.

4. In `openai.rs`:
   - Change constructor to `fn new(api_key: String, base_url: String)`.
   - Remove `with_base_url`.
   - Assign `base_url` directly in the constructor.

5. In `client.rs`, rewrite `LlmClient`:
   ```rust
   pub struct LlmClient {
       provider: OpenAiProvider,
   }

   impl LlmClient {
       pub fn new(api_url: String, api_key: String) -> Self {
           Self {
               provider: OpenAiProvider::new(api_key, api_url),
           }
       }

       pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
           self.provider.chat(request).await
       }
   }
   ```
   Remove `core_addr`, `cache`, `resolve_key`, and all gRPC imports from this file.

6. In `Cargo.toml` for `scarllet-llm`, remove the `scarllet-proto` dependency (no longer needed).

### Effort 5 — `agents/default/src/main.rs`

1. Remove `DEFAULT_PROVIDER`, `DEFAULT_MODEL` constants.
2. Remove `SCARLLET_PROVIDER` and `SCARLLET_MODEL` env var reads.
3. Import `ActiveProviderQuery` from the proto.
4. In the task loop, before building the `ChatRequest`:
   ```rust
   let provider_resp = client
       .get_active_provider(ActiveProviderQuery {})
       .await
       .map_err(|e| e.to_string())?
       .into_inner();

   if !provider_resp.configured {
       let failure = AgentMessage {
           payload: Some(agent_message::Payload::Failure(AgentFailure {
               task_id: task.task_id.clone(),
               error: "No provider configured.".into(),
           })),
       };
       let _ = msg_tx.send(failure).await;
       continue;
   }

   let llm = LlmClient::new(provider_resp.api_url, provider_resp.api_key);
   ```
5. Use `provider_resp.model` in the `ChatRequest` instead of the old `model` variable.
6. Move `LlmClient` construction inside the task loop (created fresh each round with potentially new provider info).
7. Remove the `llm` binding from before the loop.

## Coding rules (enforced)

- **Early returns:** Guard invalid states at the top. `if !resp.configured { send failure; continue; }`.
- **Functional style (Rust spirit):** Keep I/O at boundaries. `active_provider()` is a pure lookup.
- **Explicit dependencies:** `route_prompt` receives `config` as a parameter, not via hidden state. `LlmClient::new()` takes explicit `api_url` and `api_key`, not a core address.
- **No hidden singletons:** The credential cache in the old `LlmClient` is removed. Provider info is fetched per-round and passed explicitly.
