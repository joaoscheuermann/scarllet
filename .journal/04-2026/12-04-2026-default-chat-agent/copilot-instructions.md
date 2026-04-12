# Copilot Instructions — Default Chat Agent Implementation

These instructions are scoped to the approved architecture. Follow them during the implementation phase.

## General rules

- Follow early returns and guard clauses — handle errors at the top, keep happy path flat.
- Pass dependencies explicitly (Core address, API keys, config) — no hidden globals.
- Use `async-trait` for trait implementations when needed.
- All new public types and functions get a one-line doc comment.
- Run `npx nx run <project>:build` after each file change to catch errors early.
- Run `npx nx run <project>:test` to verify existing tests still pass.

## Effort 1: Proto — AgentStream RPC and messages

**File**: `packages/rust/scarllet-proto/proto/orchestrator.proto`

- Add `rpc AgentStream(stream AgentMessage) returns (stream AgentTask);` to the `Orchestrator` service. Place after `AttachTui`.
- Add the 5 new message types (`AgentMessage`, `AgentRegister`, `AgentProgress`, `AgentResult`, `AgentFailure`, `AgentTask`) after the existing TUI streaming messages.
- `AgentMessage` uses `oneof payload` with 4 variants (register, progress, result, failure).
- `AgentTask` is a plain message (no oneof needed for MVP).
- Do NOT modify any existing message types or RPCs.
- Verify: `npx nx run scarllet-proto:build` passes.

## Effort 2: LLM — Gemini provider adapter

**File**: `packages/rust/scarllet-llm/src/gemini.rs` (new)

- Create `GeminiProvider` struct with `api_key: String` and `http: reqwest::Client`.
- Implement `LlmProvider` trait for `GeminiProvider`.
- Translation rules:
  - `ChatMessage { role: System }` → `systemInstruction.parts[0].text` (not in contents)
  - `ChatMessage { role: User }` → `contents[].role = "user"`
  - `ChatMessage { role: Assistant }` → `contents[].role = "model"`
  - `temperature` → `generationConfig.temperature`
  - `max_tokens` → `generationConfig.maxOutputTokens`
- API endpoint: `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}`
- Use query param `key=` for auth (NOT Bearer token — Gemini uses API key in URL).
- Map HTTP errors: 401/403 → Unauthorized, 429 → RateLimited, 500+ → ServerError.
- Handle empty candidates: return `InvalidResponse`.
- All Gemini-specific serde types (`GeminiRequest`, `GeminiResponse`, etc.) are private to this module.
- Add unit test for role mapping.

**File**: `packages/rust/scarllet-llm/src/lib.rs`

- Add `pub mod gemini;`

**File**: `packages/rust/scarllet-llm/src/client.rs`

- Add `use crate::gemini::GeminiProvider;`
- In `LlmClient::chat()`, add match arm: `"gemini" => GeminiProvider::new(api_key).chat(request).await`
- Keep the existing OpenAI fallback for unknown providers.

## Effort 3: Core — AgentRegistry + AgentStream RPC

**File**: `packages/rust/scarllet-core/src/agents.rs` (new)

- Create `AgentRegistry` with `HashMap<String, mpsc::Sender<AgentTask>>`.
- Methods: `register(name, sender)`, `deregister(name)`, `get(name) -> Option<&Sender>`, `is_running(name) -> bool`.
- Add tests for register/deregister/get.

**File**: `packages/rust/scarllet-core/src/main.rs`

- Add `mod agents;` and `use agents::AgentRegistry;`.
- Add `agent_registry: Arc<RwLock<AgentRegistry>>` to `OrchestratorService`.
- Create it in `main()` and pass to the service.
- Implement `AgentStream` RPC:
  1. Create `mpsc::channel::<AgentTask>(64)` for outgoing tasks.
  2. Spawn a reader task for the incoming `AgentMessage` stream.
  3. On `AgentRegister`: register the task sender in `AgentRegistry` under the agent name. Log.
  4. On `AgentProgress`: look up `task_id` in task_manager for agent_name, broadcast `AgentThinkingEvent` to TUI sessions.
  5. On `AgentResult`: broadcast `AgentResponseEvent` to TUI sessions.
  6. On `AgentFailure`: broadcast `AgentErrorEvent` to TUI sessions.
  7. Return `ReceiverStream::new(rx)` as the response stream.
  8. On stream close: deregister agent from `AgentRegistry`. Log.

## Effort 4: Core — Update route_prompt for live agents

**File**: `packages/rust/scarllet-core/src/main.rs`

- In `route_prompt`, after finding the agent name:
  1. Check `agent_registry.read().await.is_running(&agent_name)`.
  2. If **running**: get the sender, send `AgentTask { task_id, prompt: text, working_directory }` through it. Do NOT spawn a new process.
  3. If **not running**: spawn the agent binary (existing `spawn_agent` flow). The agent will connect back, open `AgentStream`, and register. Core queues the task — use a short delay or a "pending tasks" buffer to handle the race between spawn and registration.
- For the spawn-then-wait-for-registration race: simplest approach is a retry loop — after spawning, poll `agent_registry.is_running()` for up to 10 seconds, then send the task. If timeout: broadcast `AgentErrorEvent`.
- The existing `spawn_agent` function remains for backward compatibility with short-lived agents. `route_prompt` chooses the path based on whether the agent has an active stream.

## Effort 5: Chat agent binary

**New crate**: `packages/rust/scarllet-chat-agent/`

**Cargo.toml dependencies**: `scarllet-proto`, `scarllet-llm`, `tonic`, `tokio`, `serde_json`, `tracing`, `tracing-subscriber`, `clap`.

**`project.json`**: Use `@monodon/rust` template for a binary. Add `run` target.

**`Cargo.toml` workspace**: Add `packages/rust/scarllet-chat-agent` to root `Cargo.toml` `[workspace].members`.

**`src/main.rs` structure**:

```
fn main():
    if args contain --manifest:
        print manifest JSON, exit

    read SCARLLET_CORE_ADDR from env
    connect to Core via gRPC
    open AgentStream bidirectional stream
    send AgentRegister { agent_name: "chat" }

    create Conversation { system_prompt, history: vec![] }
    create LlmClient::new(core_addr)

    loop: receive AgentTask from stream
        append ChatMessage { role: User, content: task.prompt } to history
        send AgentProgress { task_id, content: "" }  (thinking)

        build ChatRequest:
            provider: "gemini"
            model: "gemini-2.0-flash" (or from env SCARLLET_MODEL)
            messages: [system_prompt] + history

        match llm_client.chat(request).await:
            Ok(response):
                append ChatMessage { role: Assistant, content } to history
                send AgentResult { task_id, content }
            Err(e):
                send AgentFailure { task_id, error: e.to_string() }
                (don't exit — stay alive for next prompt)
```

**Manifest JSON** (printed on `--manifest`):
```json
{ "name": "chat", "kind": "agent", "version": "0.1.0", "description": "Default chat agent" }
```

## Effort 6: Integration

- Build all crates.
- Set Gemini API key: add `"gemini": { "api_key": "..." }` to `%APPDATA%/scarllet/config.json`.
- Copy or symlink the compiled `scarllet-chat-agent` binary into `%APPDATA%/scarllet/agents/`.
- Start Core → start TUI → type a prompt → verify end-to-end flow.

## Do NOT

- Do not modify existing TUI code — the event rendering already handles all CoreEvent types.
- Do not modify the existing `AttachTui` or `ReportProgress` RPCs.
- Do not add a plugin system, agent framework, or generic orchestration layer.
- Do not persist conversation history to disk.
- Do not add new dependencies to `scarllet-sdk` or `scarllet-tui`.
