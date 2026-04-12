# Decisions — Provider Configuration & Model Selection

## D-1: All providers use OpenAI-compatible API format

**Status:** Approved

**Context:** The codebase has separate adapters for OpenAI and Gemini. Users want to add arbitrary providers (OpenRouter, local Ollama, Azure, etc.) without writing new adapter code.

**Decision:** Remove the Gemini adapter. All providers use the OpenAI-compatible chat completions endpoint (`POST {api_url}/chat/completions` with Bearer auth). This is the de facto standard — Gemini, Ollama, LiteLLM, OpenRouter, and most providers expose an OpenAI-compatible surface.

**Principles applied:**
- **KISS** — one adapter instead of N provider-specific ones.
- **OCP** — new providers are added by config (data), not by editing code.
- **DRY** — no duplicated HTTP/serialization logic across adapters.

---

## D-2: Model selection owned by Core, not agents

**Status:** Approved

**Context:** The default agent hardcodes `DEFAULT_PROVIDER = "gemini"` and `DEFAULT_MODEL = "gemini-2.0-flash"`, with env var overrides. This makes model switching require restarting the agent with different env vars.

**Decision:** Core owns the active provider and model in `config.json`. Agents fetch the active provider info via `GetActiveProvider` gRPC at the start of each round. Agents have zero knowledge of provider names or model IDs.

**Principles applied:**
- **SRP** — agents own conversation logic; Core owns provider selection.
- **DIP** — agents depend on a stable gRPC interface, not on config files or env vars.

---

## D-3: Per-provider `active_model` (not global)

**Status:** Approved

**Context:** The active model could be a single global field or per-provider. A global field requires the user to change both `active_provider` and `active_model` when switching providers. Per-provider means each provider remembers its own selected model.

**Decision:** `active_model` is a field on each provider object. When switching `active_provider`, the model follows the provider's own `active_model`. This reduces friction when toggling between providers.

**Trade-off:** Slightly more complex config structure, but better UX when switching providers frequently.

---

## D-4: Remove old credential RPCs (breaking change)

**Status:** Approved

**Context:** `GetCredentials` and `SetCredential` RPCs served the old `HashMap<String, ProviderCredential>` format. The new `GetActiveProvider` RPC supersedes them. Config mutation is manual for now.

**Decision:** Remove both old RPCs and their messages. This is a breaking change but acceptable — no external consumers depend on these RPCs, and all internal callers (agent, core) are updated in the same changeset.

**Principles applied:**
- **KISS** — dead code removed, not preserved behind compatibility layers.
- **ISP** — agents only need `GetActiveProvider`, not a generic credential lookup.

---

## D-5: No model validation in Core

**Status:** Approved

**Context:** Core could validate that `active_model` exists in the provider's `models` list before serving it.

**Decision:** Core passes `active_model` as-is. Validation would add complexity with no clear benefit — the provider's API is the ultimate authority on valid model IDs. HTTP errors from invalid models are propagated to the TUI as `AgentFailure`.

**Principles applied:**
- **KISS** — avoid speculative validation; let the real system (provider API) be the validator.

---

## D-6: Pre-dispatch provider check in Core's `route_prompt`

**Status:** Approved

**Context:** When no provider is configured, the system could either: (a) dispatch to the agent and let it fail via `GetActiveProvider → configured=false`, or (b) Core checks before dispatching and sends a `SystemEvent` immediately.

**Decision:** Both paths handle it. Core checks in `route_prompt` for fast user feedback (SystemEvent: "No provider configured..."). The agent also handles `configured=false` defensively (sends AgentFailure). Belt and suspenders.

**Principles applied:**
- **KISS** — fast feedback path in Core avoids unnecessary agent spawn.
- **DIP** — agent doesn't assume Core will always catch it; handles the error itself.

---

## D-7: `LlmClient` no longer resolves credentials from Core

**Status:** Approved

**Context:** The old `LlmClient` held a `core_addr`, connected to Core via gRPC to fetch credentials, and cached them. This mixed concerns: the LLM library knew about Core's gRPC interface.

**Decision:** `LlmClient::new(api_url, api_key)` takes explicit parameters. The agent is responsible for fetching provider info from Core and passing it to the LLM client. The `scarllet-proto` dependency is removed from `scarllet-llm`.

**Principles applied:**
- **DIP** — LLM library depends on explicit data (strings), not on Core's transport layer.
- **SRP** — credential resolution is the agent's job, not the HTTP client's.
- **ISP** — `LlmClient` exposes only what it needs: `chat()`.
