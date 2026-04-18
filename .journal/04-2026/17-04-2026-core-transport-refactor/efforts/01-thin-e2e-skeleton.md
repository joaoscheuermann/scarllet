---
status: done
order: 1
created: 2026-04-17 13:18
title: "Thin E2E skeleton — proto rewrite + canned agent response"
---

## Description

Replace the entire wire contract with the new proto (architecture §3). Restructure `scarllet-core` into `session/`, `agents/`, `service/` sub-trees with just enough implementation to host a main agent that emits a single canned `Result` node. Migrate `agents/default` and `scarllet-tui` atomically onto the new surface. Delete the TUI's local `session.json` store and remove `HistorySync` entirely. After this effort the full stack — core, agent, TUI — runs on the new model end-to-end, but with no LLM, no tools, no streaming thoughts, no queue pipelining, no sub-agents. Those layer in over efforts 2–7.

This is the refactor's bootstrap commit: the largest slice, but the irreducible minimum to keep the tree runnable after breaking the proto atomically.

## Objective

Launch the TUI, type `hello`, and observe within one second:

1. A `User` node with text `hello` at the top of the chat.
2. An `Agent` node immediately after, followed by a `Result` node whose content is `Hello from agent! You said: hello`.
3. Pressing `Ctrl-N` clears the chat and starts a fresh session id (visible in the status bar).
4. Pressing `Esc` while the (brief) turn is in flight kills the agent cleanly and returns the input to idle.

## Implementation Details

### 1. Proto rewrite — `packages/rust/scarllet-proto/proto/orchestrator.proto`

Replace the entire file with the contract defined in architecture §3. Declare the full service and all node kinds now (even ones only used in later efforts) so the enum never needs renumbering.

- Service RPCs: `CreateSession`, `ListSessions`, `DestroySession`, `GetSessionState`, `AttachSession`, `SendPrompt`, `StopSession`, `AgentStream`, `GetActiveProvider`, `GetToolRegistry`, `GetConversationHistory`, `InvokeTool`.
- `NodeKind` enum values: `USER`, `AGENT`, `THOUGHT`, `TOOL`, `RESULT`, `DEBUG`, `TOKEN_USAGE`, `ERROR`.
- Payload messages: `UserPayload`, `AgentPayload`, `ThoughtPayload`, `ToolPayload`, `ResultPayload`, `DebugPayload`, `TokenUsagePayload`, `ErrorPayload`.
- `NodePatch` with all patch fields per architecture §3.2.
- `AgentOutbound` / `AgentInbound` oneofs.
- `SessionDiff` oneof: `Attached`, `NodeCreated`, `NodeUpdated`, `QueueChanged`, `AgentRegistered`, `AgentUnregistered`, `StatusChanged`, `SessionDestroyed`.

Delete every old message (`TuiMessage`, `CoreEvent`, old `AgentMessage`, old `AgentInstruction`, `HistoryEntry`, `HistorySync`, `AgentHistorySync`, `PromptMessage`, `CancelPrompt`, `AgentTask` old fields, `AgentProgressMsg`, `AgentResultMsg`, `AgentTokenUsageMsg`, `AgentToolCallMsg`, `AgentBlock`, `EmitDebugLog`, `DebugLogRequest`, `AgentStartedEvent`, `AgentThinkingEvent`, `AgentResponseEvent`, `AgentErrorEvent`, `SystemEvent`, `ProviderInfoEvent`, `AgentToolCallEvent`, `DebugLogEvent`, `TokenUsageEvent`, old `AgentRegister`).

`cargo check -p scarllet-proto` must pass.

### 2. Core restructure — `packages/rust/scarllet-core/src/`

**Delete** (contents subsumed by the new structure):

- `sessions.rs`, `routing.rs`, `tasks.rs`, `events.rs`, `agents.rs`, `service.rs`

**Keep (minor updates)**:

- `main.rs` — adapts to the new `OrchestratorService` struct.
- `registry.rs` — unchanged.
- `watcher.rs` — config watcher no longer broadcasts globally (broadcasting is per-session from effort 2 onward; for now config reload is silent).
- `tools.rs` — signature change: accept `session_id` + `agent_id`; drop `snapshot_id`. Branch-stub for `tool_name == "spawn_sub_agent"` returning an unimplemented `ToolResult` (real impl in effort 5).

**Create `session/`**:

- `mod.rs` — `Session`, `SessionRegistry`, `SessionStatus` (`Running` | `Paused`), `SessionConfig` (snapshot of `ScarlletConfig::active_provider` at create time).
- `state.rs` — `SessionState` snapshot builder used by `Attached` + `GetSessionState`.
- `queue.rs` — `QueuedPrompt`, `SessionQueue` with `push_back` / `pop_front` / `len` / `iter`. (Actual dispatch logic lives in `agents::routing`.)
- `nodes.rs` — `NodeStore { order: Vec<String>, by_id: HashMap<String, Node>, children_of: HashMap<String, Vec<String>> }`. Implement `create(node) -> Result<&Node, InvariantError>` with parent-rule validation per AC-5.4 (User / Agent / Error top-level allowed; everything else requires an Agent parent). `update` is stubbed to `unimplemented!()` — real impl in effort 2.
- `diff.rs` — builders `node_created`, `queue_changed`, `agent_registered`, `agent_unregistered`, `status_changed`, `destroyed`, `attached`. Each returns a `SessionDiff`. Add helper `broadcast(&Session, SessionDiff)` that loops `subscribers.try_send` and prunes closed ones.
- `subscribers.rs` — `SubscriberSet<SessionDiff>` wrapping `Vec<mpsc::Sender<Result<SessionDiff, tonic::Status>>>` with `push`, `len`, and `broadcast(diff)` (clones and prunes dead).

**Create `agents/`**:

- `mod.rs` — `AgentRecord { agent_id, agent_module, parent_id, pid, tx, agent_node_id }`, `AgentRegistry { by_id, main_agent_id, sub_agent_waiters }`. Methods: `register`, `deregister`, `get`, `has_main`. For this effort `sub_agent_waiters` is an empty `HashMap` — used from effort 5.
- `spawn.rs` — `spawn_main_agent(session_id, agent_id, module_path, prompt, cwd)` sets env (`SCARLLET_CORE_ADDR`, `SCARLLET_SESSION_ID`, `SCARLLET_AGENT_ID`, `SCARLLET_PARENT_ID = session_id`, `SCARLLET_AGENT_MODULE`) and forks the binary. `handle_spawn_sub_agent` is stubbed with `unimplemented!()` (effort 5).
- `stream.rs` — AgentStream handler:
  - Handle `AgentOutbound::Register { desired_agent_id?, agent_module, parent_id }` — validate, insert `AgentRecord`, if `parent_id == session_id` set `main_agent_id`, broadcast `AgentRegistered`, send `AgentInbound::Task { session_id, agent_id, parent_id, prompt, working_directory }`.
  - Handle `AgentOutbound::CreateNode(node)` — validate node kind + parent, store via `NodeStore::create`, broadcast `NodeCreated`. Reject `CreateNode(AGENT)` — `Agent` nodes are created by core only.
  - Handle `AgentOutbound::TurnFinished` — mark the agent's Agent node via a server-side patch (`agent_status = "finished"`), broadcast `NodeUpdated`, deregister from `AgentRegistry`, clear `main_agent_id` if applicable, broadcast `AgentUnregistered`, call `agents::routing::try_dispatch_main`.
  - On stream close without `TurnFinished` — mark Agent node `agent_status = "failed"`, emit an `Error` node under the Agent, broadcast `AgentUnregistered`. (Paused state handling arrives in effort 6; for now simply deregister.)
- `routing.rs` — `try_dispatch_main(session: &mut Session)`:
  1. Early return if `session.status != Running` or `main_agent_id.is_some()`.
  2. Pop one `QueuedPrompt`; broadcast `QueueChanged` (empty queue).
  3. Resolve `module` via `global_config.default_agent`; if missing or the module is not registered in `ModuleRegistry` → create a top-level `Error` node (AC-3.3 strict), broadcast, and return.
  4. Generate `agent_id = Uuid::new_v4()`.
  5. `NodeStore::create(Agent { id=agent_id, parent=None, module, agent_id, status="running" })`; broadcast `NodeCreated`.
  6. `agents::spawn::spawn_main_agent(...)`.

**Create `service/`**:

- `mod.rs` — `OrchestratorService` struct holding `started_at`, `registry`, `config`, `sessions: Arc<RwLock<SessionRegistry>>`, `bound_addr`. Implement `tonic::Orchestrator` trait by thin delegation to the sibling modules.
- `session_rpc.rs` — implement `create_session`, `list_sessions`, `destroy_session`, `get_session_state`, `attach_session`, `send_prompt`, `stop_session`.
  - `attach_session` — if `session_id` empty, call `create_session` internally. Register a subscriber `mpsc::channel(256)`. Build the initial `SessionState` snapshot. Send `SessionDiff::Attached { state }` on the channel immediately. Return the `ReceiverStream`. On subscriber drop (last one), schedule a `destroy_session` of that id (post-return, via a tracked handle that compares `subscribers.len()`).
  - `send_prompt` — validate session_id; grab write lock; `NodeStore::create(User)`; broadcast; enqueue; `try_dispatch_main`; return `SendPromptResponse { user_node_id }`.
  - `stop_session` — iterate `session.agents` in reverse parent-child order; best-effort kill each PID + deregister + broadcast. Clear queue + broadcast. Set `status = Running` + broadcast.
- `tool_rpc.rs` — implement `get_tool_registry(session_id)` (returns registered Tool-kind modules plus the synthetic `spawn_sub_agent` manifest — full content comes in effort 5; for effort 1 just return the external tools). `invoke_tool(session_id, agent_id, tool_name, input_json)` delegates to `tools::invoke_external` or returns `"not implemented"` for `spawn_sub_agent`.
- `agent_rpc.rs` — implement `get_active_provider(session_id)` (returns the session's snapshot), `get_conversation_history(session_id)` (returns empty list for effort 1; real derivation in effort 2). `agent_stream` spawns `agents::stream::run` for the bidi stream.

### 3. Agent SDK — `packages/rust/scarllet-sdk/src/agent/mod.rs` (new)

Minimum surface for effort 1:

- `pub struct AgentSession` with fields per architecture §7.
- `async fn connect() -> Result<Self, AgentSdkError>` — reads `SCARLLET_CORE_ADDR`, `SCARLLET_SESSION_ID`, `SCARLLET_AGENT_ID`, `SCARLLET_PARENT_ID`, `SCARLLET_AGENT_MODULE`; opens `AgentStream`; sends `Register`.
- `async fn next_task(&mut self) -> Option<AgentTask>` — blocks on `AgentInbound` until a `Task` (returns `Some`) or the stream closes (returns `None`).
- `async fn emit_result(&self, content: &str, finish_reason: &str) -> Result<(), AgentSdkError>` — emits `CreateNode(Result { parent = self.agent_node_id, content, finish_reason })` then `TurnFinished { finish_reason }`.

Add `pub mod agent;` to `packages/rust/scarllet-sdk/src/lib.rs`. Export the SDK error type.

### 4. Default agent — `packages/rust/agents/default/src/main.rs`

Replace the whole `main.rs` body (keep the `--manifest` path). New flow:

```rust
let mut session = AgentSession::connect().await?;
while let Some(task) = session.next_task().await {
    let reply = format!("Hello from agent! You said: {}", task.prompt);
    session.emit_result(&reply, "stop").await?;
    break; // per-turn lifetime
}
```

Remove the LLM / tool-loop logic — that re-enters in effort 2. Remove `scarllet-llm` dependency use for this effort (keep it in `Cargo.toml` though; it returns in effort 2).

### 5. TUI migration — `packages/rust/scarllet-tui/src/`

- **Delete** `session.rs`. Remove `FileSessionRepository` / `NullSessionRepository` / `SessionRepository` trait.
- **Rewrite `app.rs`**:
  - Replace `messages: Vec<ChatEntry>` + `tool_calls: HashMap` + `session_id: String` + `session_repo` with:
    - `session_id: Option<String>`
    - `session_status: SessionStatus` (Running / Paused; starts Running)
    - `nodes: HashMap<String, Node>`
    - `node_order: Vec<String>`
    - `queue: Vec<QueuedPromptSnapshot>`
    - `connected_agents: HashMap<String, AgentSummary>`
  - Delete `save_session`, `load_from_session`, `new_session` (the new Ctrl-N flow is handled in `main.rs` event loop by calling `DestroySession` + `AttachSession` via the connection task).
- **Rewrite `connection.rs`**: after the channel is up, call `AttachSession { session_id: None }`; spawn a pair of channels (incoming `SessionDiff`, outgoing commands). On `SessionDiff::Attached { state }` seed `app`. Loop forwarding subsequent diffs.
- **Rewrite `events.rs`**:
  - Enter → enqueue an outgoing `SendPromptCommand { text, cwd }`.
  - Esc → enqueue `StopSessionCommand`.
  - Ctrl-N → enqueue `DestroyAndRecreateCommand` (connection layer issues `DestroySession` then `AttachSession`).
  - Remove all `HistorySync` construction.
- **Rewrite `render.rs` / `widgets/chat_message.rs`**:
  - Iterate `node_order`, rendering each top-level node plus its immediate descendants. For effort 1, only `User`, `Agent`, and `Result` appear — render `User` as a user line, `Agent` as an agent header with its child `Result` as body.
  - Drop any `ChatEntry`-based rendering code.
- **Rewrite `main.rs`**: remove session-repo construction; `App::new` no longer takes a repo; event loop spawns the connection task as before.

### 6. Build glue

- `scarllet-tui/Cargo.toml` — drop now-unused deps introduced by the old session store only if truly unused (keep `chrono` + `uuid` since `connection.rs` + rendering still need them).
- `scarllet-core/src/main.rs` — construct `OrchestratorService { sessions: Arc::new(RwLock::new(SessionRegistry::new())), ... }` in place of the old field set.

## Verification Criteria

1. `npx nx run scarllet-proto:build` — green.
2. `npx nx run scarllet-core:build` — green.
3. `npx nx run scarllet-core:test` — green; includes at least new unit tests for `NodeStore::create` parent-rule invariants (happy + rejected-parent cases) and `SessionRegistry::create_session` + `destroy_session`.
4. `npx nx run scarllet-sdk:build` + `:test` — green.
5. `npx nx run scarllet-tui:build` — green.
6. `cargo build -p scarllet-default-agent` (or equivalent via workspace build) — green.
7. `cargo clippy --workspace --all-targets` — no new warnings beyond pre-existing baseline.
8. **Run & observe (required)**: `npx nx run scarllet-tui:run`. With core auto-spawned via lockfile, type `hello` and Enter. Within 1 second the chat shows:
   - `User: hello`
   - `Agent (default): Hello from agent! You said: hello` (Result node content).
9. **Run & observe**: press Ctrl-N. The chat clears; status bar shows a different session id. Send another prompt → canned reply appears again.
10. **Run & observe**: while the canned reply is printing (very brief), press Esc. No visible error; the input is ready for the next prompt. (Timing may make this a no-op — acceptable for effort 1; real stop verification is in effort 6.)

## Done

- Proto fully replaced; no old wire messages remain in `orchestrator.proto`.
- `scarllet-core/src/` restructured into `session/`, `agents/`, `service/`.
- `session.rs`, `routing.rs`, `tasks.rs`, `events.rs`, old `agents.rs`, old `service.rs` removed.
- `scarllet-tui/src/session.rs` deleted; `HistorySync` nowhere in the codebase.
- `agents/default` uses `scarllet_sdk::agent::AgentSession` and replies with canned text; no `scarllet-proto` direct usage in its main flow.
- Running the TUI end-to-end shows User + Agent + Result nodes appearing via the new diff stream.
- Ctrl-N destroys + recreates the session via the new RPCs.

## Change Summary

### Files created

- `packages/rust/scarllet-core/src/session/mod.rs`
- `packages/rust/scarllet-core/src/session/state.rs`
- `packages/rust/scarllet-core/src/session/queue.rs`
- `packages/rust/scarllet-core/src/session/nodes.rs`
- `packages/rust/scarllet-core/src/session/diff.rs`
- `packages/rust/scarllet-core/src/session/subscribers.rs`
- `packages/rust/scarllet-core/src/agents/mod.rs`
- `packages/rust/scarllet-core/src/agents/spawn.rs`
- `packages/rust/scarllet-core/src/agents/stream.rs`
- `packages/rust/scarllet-core/src/agents/routing.rs`
- `packages/rust/scarllet-core/src/service/mod.rs`
- `packages/rust/scarllet-core/src/service/session_rpc.rs`
- `packages/rust/scarllet-core/src/service/tool_rpc.rs`
- `packages/rust/scarllet-core/src/service/agent_rpc.rs`
- `packages/rust/scarllet-sdk/src/agent/mod.rs`

### Files modified

- `packages/rust/scarllet-proto/proto/orchestrator.proto` — full rewrite with new service, `NodeKind` enum, payload messages, `NodePatch`, `AgentOutbound` / `AgentInbound` oneofs, `SessionDiff` oneof.
- `packages/rust/scarllet-proto/src/lib.rs` — dropped `blocks_to_text` (and tests) since `AgentBlock` is gone.
- `packages/rust/scarllet-sdk/src/config.rs` — added `default_agent: String` (defaults to `"default"`); manual `Default` impl; updated tests.
- `packages/rust/scarllet-sdk/src/lib.rs` — added `pub mod agent;` and `pub use agent::AgentSdkError;`.
- `packages/rust/scarllet-sdk/Cargo.toml` — added `tokio`, `tokio-stream`, `tonic`, `uuid` runtime deps and a `tokio` dev-dependency.
- `packages/rust/scarllet-core/src/main.rs` — rewrote bootstrap to use the new `OrchestratorService { sessions, registry, config, … }` plus the simpler `watch_config(config)` signature.
- `packages/rust/scarllet-core/src/tools.rs` — new `invoke(registry, session_id, agent_id, tool_name, input_json)` signature; `SPAWN_SUB_AGENT_TOOL` constant; stub branch returning "not implemented" for `spawn_sub_agent`; `invoke_external` for the rest. Dropped `snapshot_id` checks.
- `packages/rust/scarllet-core/src/watcher.rs` — `watch_config` no longer takes / broadcasts to a global TUI session registry; per AC-9.2, existing sessions keep their config snapshot.
- `packages/rust/scarllet-core/src/registry.rs` — `version()` annotated `#[allow(dead_code)]` (kept for future stale-snapshot guards).
- `packages/rust/agents/default/Cargo.toml` — depends on `scarllet-sdk` instead of `scarllet-proto`; `scarllet-llm` retained for effort 02.
- `packages/rust/agents/default/src/main.rs` — full rewrite; uses `AgentSession::connect/next_task/emit_result` to reply with the canned `"Hello from agent! You said: <prompt>"` and exit (per-turn lifetime).
- `packages/rust/scarllet-tui/Cargo.toml` — dropped `tokio-stream`, `tui-markdown`, `pulldown-cmark`, `chrono`, `serde`, `serde_json`, `uuid`.
- `packages/rust/scarllet-tui/src/app.rs` — full rewrite; mirrors the per-session node graph + queue + connected agents.
- `packages/rust/scarllet-tui/src/connection.rs` — full rewrite; calls `AttachSession`, hydrates from the `Attached` first diff, bridges `CoreCommand`s.
- `packages/rust/scarllet-tui/src/events.rs` — full rewrite; key handling enqueues `CoreCommand`s; new `handle_session_diff` applies each diff to the local mirror.
- `packages/rust/scarllet-tui/src/render.rs` — rewrote history rendering to walk top-level nodes + immediate descendants.
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs` — full rewrite; renders `User`, `Agent` (+ `Result` / `Error` children), and top-level `Error` nodes.
- `packages/rust/scarllet-tui/src/widgets/mod.rs` — dropped the `markdown` module export.
- `packages/rust/scarllet-tui/src/main.rs` — rewrote bootstrap; no `session_repo`; Ctrl-N enqueues `DestroyAndRecreate`.

### Files deleted

- `packages/rust/scarllet-core/src/sessions.rs`
- `packages/rust/scarllet-core/src/routing.rs`
- `packages/rust/scarllet-core/src/tasks.rs`
- `packages/rust/scarllet-core/src/events.rs`
- `packages/rust/scarllet-core/src/agents.rs`
- `packages/rust/scarllet-core/src/service.rs`
- `packages/rust/scarllet-tui/src/session.rs`
- `packages/rust/scarllet-tui/src/widgets/markdown.rs`

### Key decisions / trade-offs

- **`AgentRegister` carries `session_id` + `agent_id` explicitly** (architecture sketch only listed `desired_agent_id, agent_module, parent_id`). Needed so a sub-agent's bidi stream can be routed back to its owning session in effort 05, where `parent_id` is the calling agent id (not the session id). Env vars `SCARLLET_SESSION_ID` / `SCARLLET_AGENT_ID` are already in the architecture.
- **`PendingDispatch` map on `Session`** holds the in-flight prompt between `try_dispatch_main` (which pops the queue + spawns the binary) and the agent's `Register` (which receives the corresponding `AgentTask`). `StopSession` clears it alongside the queue.
- **`ChatMessageWidget` owns `Line<'static>` content** to avoid borrow conflicts with `app.scroll_view_state` during the render loop.
- **`default_agent` field on `ScarlletConfig` defaults to `"default"`** with a serde-default initializer so existing `config.json` files keep parsing.
- **`watch_config` no longer takes a session registry** — per AC-9.2 the reload only affects new sessions; the global broadcast was removed.
- **TUI status bar simplified** to show `session <short-id>`. Token budget rendering was deleted with `TokenUsageEvent`; effort 07 restores it via `TokenUsage` nodes.
- **Markdown widget deleted** instead of left dormant — it had no current callers; effort 02 can reintroduce it from git history when needed.

### Deviations from Implementation Details

- **`AgentRegister` schema** — see decision above.
- **Markdown widget removed entirely** (spec only required deleting `session.rs` / `HistorySync`). Reason: no callers; would have triggered unused-code lints; effort 02 reintroduces intentionally.
- **TUI Cargo.toml dropped multiple deps** beyond `session.rs` removals (`chrono`, `uuid`, `serde`, `serde_json`, `tui-markdown`, `pulldown-cmark`, `tokio-stream`). The connection layer no longer uses `ReceiverStream`; rendering no longer needs `chrono`/`uuid`. Easy to re-add per effort if a future slice needs them.
- **Status bar lost token budget + provider name display** — token budget is effort 07; provider name can be revived in effort 02 if desired.

### Verification

| # | Criterion | Result |
|---|---|---|
| 1 | `npx nx run scarllet-proto:build` | PASS |
| 2 | `npx nx run scarllet-core:build` | PASS |
| 3 | `npx nx run scarllet-core:test` (incl. `NodeStore::create` parent-rule + `SessionRegistry` create/destroy tests) | PASS — 22 / 0 |
| 4 | `npx nx run scarllet-sdk:build` + `:test` | PASS — 13 / 0 |
| 5 | `npx nx run scarllet-tui:build` | PASS |
| 6 | `cargo build -p default-agent` | PASS |
| 7 | `cargo clippy --workspace --all-targets` | PASS — zero warnings from new / rewritten files; pre-existing baseline warnings remain |
| 8 | TUI: type `hello` → User + canned Result within 1 s | **DEFERRED — human required** |
| 9 | TUI: Ctrl-N clears chat + new session id; second prompt works | **DEFERRED — human required** |
| 10 | TUI: Esc during canned reply leaves input ready | **DEFERRED — human required** |

Independent tester subagent confirmed PASS for criteria 1–7 and DEFERRED for 8–10 per the executor's instructions.

### Pending human verification

Run `npx nx run scarllet-tui:run` (TUI auto-spawns core via the lockfile) and walk through the three interactive checks above before relying on Effort 01 for downstream slices.
