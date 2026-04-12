# Plan — Provider Configuration & Model Selection

## System boundaries

```
┌─────────────────────────────────────────────────────────┐
│                    config.json (%APPDATA%)               │
│  Source of truth: providers[], active_provider           │
└────────────────────────┬────────────────────────────────┘
                         │ load on startup
                         ▼
┌─────────────────────────────────────────────────────────┐
│                   scarllet-core                          │
│  Owns: in-memory ScarlletConfig, GetActiveProvider RPC   │
│  Pre-dispatch check: rejects prompts if not configured   │
└────────┬───────────────────────────────────┬─────────────┘
         │ CoreEvent (SystemEvent)           │ GetActiveProvider RPC
         ▼                                   ▼
┌─────────────────┐              ┌──────────────────────────┐
│   scarllet-tui  │              │   agent (default)         │
│  Displays system│              │  Fetches provider info    │
│  + error msgs   │              │  per round via gRPC       │
│  (no changes)   │              │  Calls LlmClient.chat()   │
└─────────────────┘              └─────────────┬────────────┘
                                               │
                                               ▼
                                 ┌──────────────────────────┐
                                 │   scarllet-llm            │
                                 │  OpenAiProvider only      │
                                 │  Receives api_url,        │
                                 │  api_key, model explicitly │
                                 └──────────────────────────┘
```

## Component responsibilities

| Component | Owns | Does NOT own |
|-----------|------|--------------|
| `scarllet-sdk/config.rs` | Config struct definitions, load/save, `active_provider()` lookup | Runtime state, gRPC serving |
| `scarllet-proto` | `GetActiveProvider` RPC + messages, protobuf code generation | Business logic |
| `scarllet-core` | Loading config, serving `GetActiveProvider`, pre-dispatch provider check, broadcasting "not configured" SystemEvent | LLM calls, model validation |
| `scarllet-llm` | OpenAI-compatible HTTP adapter, error mapping | Credential resolution, provider selection |
| `agents/default` | Conversation loop, per-round provider fetch, sending results/failures | Provider config, model choice |
| `scarllet-tui` | Rendering messages (already handles SystemEvent and AgentError) | Nothing changes here |

## Implementation order

### Effort 1 — Config data model + auto-create (`scarllet-sdk`)

**Files:** `packages/rust/scarllet-sdk/src/config.rs`

- Replace `ScarlletConfig` and `ProviderCredential` with the new structs from `data-model.md`.
- Add `ScarlletConfig::active_provider()` convenience method.
- Update `load()`: on missing file, create it with empty defaults and return the defaults. On invalid JSON, log a warning, return empty defaults (do not overwrite the broken file).
- Update `save()`: writes the new format.
- Update/replace existing tests for the new struct shape.

**Runnable outcome:** `cargo test -p scarllet-sdk` passes with roundtrip and edge-case tests.

### Effort 2 — Proto contract change (`scarllet-proto`)

**Files:** `packages/rust/scarllet-proto/proto/orchestrator.proto`

- Remove `GetCredentials`, `SetCredential` RPCs from the `Orchestrator` service.
- Remove `CredentialQuery`, `CredentialResponse`, `SetCredentialRequest`, `SetCredentialResponse` messages.
- Add `GetActiveProvider` RPC, `ActiveProviderQuery`, `ActiveProviderResponse` messages per `contracts.md`.

**Runnable outcome:** `cargo build -p scarllet-proto` succeeds. Downstream crates will not compile yet (expected).

### Effort 3 — Core implementation (`scarllet-core`)

**Files:** `packages/rust/scarllet-core/src/main.rs`

- Remove `get_credentials` and `set_credential` handler implementations.
- Implement `get_active_provider`: read config, call `active_provider()`, map to response.
- Update `route_prompt`: before dispatching to an agent, check `config.active_provider()`. If `None`, broadcast a `SystemEvent` with the message: `"No provider configured. Edit config.json at <path> to set up a provider."` and return early.
- Update config loading at startup: use the new `config::load()` which auto-creates the file.
- Remove the `ProviderCredential` import.

**Runnable outcome:** `cargo build -p scarllet-core` succeeds. Core starts, creates config if missing, responds to `GetActiveProvider` calls.

### Effort 4 — LLM simplification (`scarllet-llm`)

**Files:**
- `packages/rust/scarllet-llm/src/lib.rs` — remove `pub mod gemini;`
- `packages/rust/scarllet-llm/src/gemini.rs` — delete file
- `packages/rust/scarllet-llm/src/client.rs` — simplify
- `packages/rust/scarllet-llm/src/openai.rs` — make `base_url` a required constructor param

Changes to `LlmClient`:
- Remove the `core_addr`, credential cache, and `resolve_key` method.
- `LlmClient::new()` takes `api_url: String, api_key: String` directly.
- `chat()` no longer dispatches by provider name — it always uses `OpenAiProvider`.
- Remove the `provider` field from `ChatRequest` (it's no longer needed; the provider is implicit in the client instance).

Changes to `OpenAiProvider`:
- Constructor takes both `api_key` and `base_url` as required params (drop the default `https://api.openai.com/v1`).
- Remove `with_base_url` builder method (no longer needed).

**Runnable outcome:** `cargo build -p scarllet-llm` succeeds. `cargo test -p scarllet-llm` passes.

### Effort 5 — Agent update (`agents/default`)

**Files:** `packages/rust/agents/default/src/main.rs`

- Remove `DEFAULT_PROVIDER`, `DEFAULT_MODEL` constants.
- Remove `SCARLLET_PROVIDER`, `SCARLLET_MODEL` env var reads.
- At the start of each task round (before building `ChatRequest`), call `GetActiveProvider` RPC.
- If `configured == false`, send `AgentFailure` with a clear message and continue the loop (wait for next task).
- Construct `LlmClient::new(api_url, api_key)` with the values from the response.
- Use `resp.model` as the model in `ChatRequest`.
- Keep the existing conversation history logic unchanged.

**Runnable outcome:** Full round-trip works: TUI → Core → Agent → LLM → response displayed.

## Verification plan

| Step | What to verify |
|------|----------------|
| 1 | `cargo test -p scarllet-sdk` — config roundtrip, empty default, missing fields |
| 2 | `cargo build -p scarllet-proto` — proto compiles |
| 3 | `cargo build -p scarllet-core` — core compiles with new proto |
| 4 | `cargo test -p scarllet-llm` — OpenAI provider tests pass, no gemini references |
| 5 | `cargo build -p default-agent` — agent compiles with new client API |
| 6 | Manual: start with no config → TUI shows "no provider configured" message |
| 7 | Manual: configure a provider → send a prompt → get a response |
| 8 | Manual: set an invalid API key → see the HTTP error in TUI |
| 9 | Manual: set an unreachable `api_url` → see network error in TUI |
