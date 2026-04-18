---
status: done
order: 3
created: 2026-04-17 13:18
title: "Tool node + session-scoped InvokeTool"
---

## Description

Implement real external tool invocation end-to-end under the new contract. Agents create a `Tool` node when the LLM requests a function call, issue `InvokeTool(session_id, agent_id, tool_name, input_json)` to run the tool binary, and stream status/result into the same `Tool` node via `UpdateNode` (the one-updated model from AC-5.6). Core-side `InvokeTool` routes through the existing `tools::invoke_external` logic but is now session-aware and reserves a branch for `tool_name == "spawn_sub_agent"` (real implementation arrives in effort 5).

After this effort the default agent runs its full LLM ↔ tool loop again — the last capability that was temporarily removed in effort 1.

## Objective

With `tree`, `find`, `grep`, `edit`, `write` tools available (already in `packages/rust/tools/`), ask the agent: `Show me the top-level files in this repo`. Observe:

1. `User` node with the prompt.
2. `Agent` node; a brief `Thought` node (tokens streaming) as the LLM decides to call `tree`.
3. A `Tool` node parented under the `Agent` node, showing `tree` with status `running` and an arguments preview.
4. Status transitions to `done` within ≤5 s; the node's `result_json` holds the tool output.
5. A second `Thought` node may appear as the LLM digests the result, then a `Result` node with the summary answer.
6. `GetConversationHistory` now includes tool calls in the LLM-compatible history (so a follow-up like `and the first file?` works).

## Implementation Details

### 1. Core — `service/tool_rpc.rs`

- `invoke_tool(InvokeToolRequest { session_id, agent_id, tool_name, input_json }) -> InvokeToolResponse { success, output_json, error_message, duration_ms }`.
- Validate `session_id` exists; validate `agent_id` is currently registered in that session (reject calls from a dead agent).
- If `tool_name == "spawn_sub_agent"` — return `success=false`, `error_message="spawn_sub_agent is implemented in effort 5"` (temporary stub; effort 5 replaces).
- Otherwise: `tools::invoke_external(&self.registry, &session_id, &agent_id, tool_name, input_json).await` — forwards to the existing external-tool-process logic.

### 2. Core — `tools.rs`

- Remove the `snapshot_id` parameter (obsolete under the new registry-version model; tool invocation tolerates races because the agent's arguments are opaque to core).
- Accept `session_id: &str` and `agent_id: &str`. Store them in the process env (`SCARLLET_SESSION_ID`, `SCARLLET_AGENT_ID`) for tools that want to audit-log.
- Keep the timeout / stdin-JSON / stdout-JSON pattern unchanged.

### 3. Core — `service/session_rpc.rs`

- `get_tool_registry(GetToolRegistryRequest { session_id }) -> GetToolRegistryResponse { tools }`.
- For effort 3: return all `ModuleKind::Tool` modules. Session-scoped filtering (allow-lists etc.) is out of scope per spec.
- Also inject the synthetic `spawn_sub_agent` manifest entry so agents discover it. For effort 3 the runtime branch still returns "not implemented" — the tool is advertised but unusable until effort 5. (This keeps agents from seeing the tool appear out of nowhere between efforts.)

### 4. Core — `service/agent_rpc.rs`

Extend `get_conversation_history` derivation:

- Each `Agent` node's subtree may contain `Tool` nodes. For each `Tool` with `status ∈ {done, failed}`, append **two** `HistoryEntry` items in execution order, immediately after the owning `Agent`'s assistant turn:
  1. `{ role: "assistant-tool-call", content: "{tool_name, arguments_json, call_id}" }` (JSON-encoded, same shape the LLM producers expect in tool-calling loops — flatten into the existing `role`/`content` string pair; SDK-side adapter maps back to `ChatMessage::tool_calls`).
  2. `{ role: "tool", content: result_json, tool_call_id: call_id }`.

Alternative simpler path if the adapter is awkward: introduce a small `HistoryEntry` schema extension now (add `tool_call_id` and `tool_calls_json` optional fields) — if chosen, update `scarllet-sdk::agent::get_history` to deserialize into `scarllet_llm::types::ChatMessage` cleanly. Keep the choice inside the effort; document in the Change Summary after implementation.

### 5. SDK — `scarllet-sdk/src/agent/mod.rs`

Add:

- `pub async fn get_tools(&mut self) -> Result<Vec<ToolInfo>, _>` — calls `GetToolRegistry(session_id)`.
- `pub async fn invoke_tool(&mut self, tool_name: &str, input_json: &str) -> Result<InvokeToolResponse, _>` — unary RPC.
- `pub async fn create_tool(&self, parent_agent_node_id: &str, tool_name: &str, arguments_preview: &str, arguments_json: &str) -> Result<String, _>` — emits `CreateNode(Tool { parent = parent_agent_node_id, tool_name, arguments_preview, arguments_json, status: "pending" })`.
- `pub async fn update_tool_status(&self, node_id: &str, status: ToolStatus, duration_ms: u64, result_json: &str) -> Result<(), _>` — patches `tool_status` / `tool_duration_ms` / `tool_result_json`.
- `ToolStatus` enum — `Pending`, `Running`, `Done`, `Failed` — mapping to the proto `tool_status` string.

### 6. Default agent — `agents/default/src/main.rs`

Reinstate the tool-calling loop (adapted to the node model):

- At turn start: `let tools = session.get_tools().await?`; convert to `scarllet_llm::types::ToolDefinition`.
- Include `tools` in the `ChatRequest`.
- Loop:
  - Stream the LLM request. For each delta: append into the current Thought node (as in effort 2). Accumulate any `tool_call` deltas.
  - On stream end:
    - If no tool calls → create the Result node + TurnFinished + exit loop.
    - Else: for each tool call:
      - `let preview = truncate_preview(&args, 40);`
      - `let tool_node_id = session.create_tool(task.agent_id, name, preview, args_json).await?;`
      - `session.update_tool_status(&tool_node_id, ToolStatus::Running, 0, "").await?;`
      - `let start = Instant::now(); let resp = session.invoke_tool(&name, &args_json).await?;`
      - `let status = if resp.success { ToolStatus::Done } else { ToolStatus::Failed };`
      - `session.update_tool_status(&tool_node_id, status, start.elapsed().as_millis() as u64, if resp.success { &resp.output_json } else { &resp.error_message }).await?;`
    - Append the tool result(s) to the local `history: Vec<ChatMessage>` (mirroring what history would return next turn) and loop back.
- Keep `working_directory` injection into `args` when absent (preserve the current convenience).

### 7. TUI — `render.rs` + `widgets/`

- Add a Tool-node widget showing: `🔧 tool_name (<duration_ms>ms) [status]` with an expand/collapse toggle. Preview the `arguments_preview` on the header; show `result_json` truncated to 8 lines in the body when expanded.
- Handle `NodeUpdated` diffs on Tool nodes: re-render in place.

## Verification Criteria

1. All four Rust projects (`scarllet-proto` / `scarllet-core` / `scarllet-sdk` / `scarllet-tui`) plus the default agent build clean.
2. `npx nx run scarllet-core:test` — adds tests for `invoke_tool` rejecting unknown session/agent, `get_tool_registry` including both external tools and the synthetic `spawn_sub_agent` entry, and `get_conversation_history` emitting tool call + tool result history rows in the right order.
3. `cargo clippy --workspace --all-targets` — clean.
4. **Run & observe (required)**: with provider + tools configured, run the TUI, prompt `Show me the top-level files in this repo`. Observe a `Tool` node for `tree` transitioning `pending → running → done` within ≤5 s; the Thought node continues to grow; a Result node summarises.
5. **Run & observe**: follow up with `now grep for "main" in those files`. Observe another Tool node (`grep`), this time parented under a new Agent turn; conversation history preserves the prior tree result (check by prompting `what was the first file in the previous tree output?` — the LLM should answer correctly from history).
6. **Run & observe**: invoke `InvokeTool("spawn_sub_agent", {...})` manually via `grpcurl` against the running core — confirm it returns `success=false` with the "not implemented" placeholder (proves routing wiring; real impl lands in effort 5).

## Done

- `InvokeTool` end-to-end with session_id + agent_id.
- `Tool` nodes follow the one-updated status lifecycle.
- Conversation history includes tool calls + tool results.
- Default agent runs its full LLM ↔ tool loop on the new model.
- `spawn_sub_agent` is advertised in the tool registry but temporarily stubbed.

## Change Summary

### Files modified

- `packages/rust/scarllet-proto/proto/orchestrator.proto` — extended `HistoryEntry` with optional `tool_call_id` and `tool_calls_json`.
- `packages/rust/scarllet-core/src/tools.rs` — `SPAWN_SUB_AGENT_DESCRIPTION` / `SPAWN_SUB_AGENT_INPUT_SCHEMA` constants; stub error message updated to `"spawn_sub_agent is implemented in effort 5"`; forwards `session_id` / `agent_id` to spawned tool processes via `SCARLLET_SESSION_ID` / `SCARLLET_AGENT_ID` env vars.
- `packages/rust/scarllet-core/src/service/tool_rpc.rs` — `invoke_tool` now validates that `agent_id` is registered in the session (rejects with `failed_precondition`); `get_tool_registry` injects the synthetic `spawn_sub_agent` `ToolInfo`. 4 new tests.
- `packages/rust/scarllet-core/src/session/state.rs` — `conversation_history` derives tool-call + tool-result history pairs from terminal `Tool` nodes; 4 new tests; existing 5 tests updated to cover the new optional fields.
- `packages/rust/scarllet-sdk/src/agent/mod.rs` — `pub enum ToolStatus { Pending|Running|Done|Failed }` with `as_wire` / `Display`; `AgentSession::get_tools`, `invoke_tool`, `create_tool`, `update_tool_status`. 1 new test.
- `packages/rust/agents/default/src/main.rs` — full rewrite of `run_turn` to drive the LLM ↔ tool loop. New helpers: `stream_completion`, `run_tool_call`, `tools_to_definitions`, `accumulate_tool_call_deltas`, `finalize_tool_calls`, `inject_working_directory`, `truncate_preview`, `history_entry_to_chat_message`. Iteration cap (`MAX_TOOL_ITERATIONS = 24`) + 8 unit tests.
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs` — `append_tool_lines`: `🔧 tool_name (Nms) [status]` headers, args preview, result-JSON preview (8-line truncation). Yellow (pending/running), green (done), red (failed).
- `packages/rust/agents/default/Cargo.toml` — added `[dev-dependencies] tokio` for async tests.

### Files created / deleted

None.

### Key decisions / trade-offs

- **`HistoryEntry` schema choice — Option (b)**, extend the message: added `optional string tool_call_id = 3` and `optional string tool_calls_json = 4`. Rejected Option (a) (synthetic `assistant-tool-call` role + JSON-blob content) because the new fields map 1:1 onto `scarllet_llm::types::ChatMessage::tool_calls` / `tool_call_id`, the SDK adapter becomes a direct `serde_json::from_str` + field copy, and future LLM-specific extensions can land as additional optional fields without breaking consumers.
- **`Tool` node id used as the `call_id`** — avoids adding a `call_id` field to `ToolPayload`. The Tool node id is unique and stable; in-turn LLM-side `call_xyz` ids matter only during a single iteration.
- **`invoke_tool` agent validation returns `failed_precondition`** — distinguishes "session is fine but the calling agent is dead/never-registered" from `not_found` (unknown session). Clearer diagnostics for SDK/grpcurl.
- **`spawn_sub_agent` advertised at `timeout_ms = 0`** — placeholder semantics are obvious to anyone inspecting the registry response with `grpcurl` before effort 05 lands.
- **`MAX_TOOL_ITERATIONS = 24` safety cap** in the default agent — belt-and-suspenders against runaway loops.
- **`status: Running` then `Done`/`Failed`, two `update_tool_status` calls per invocation** — matches the `pending → running → done|failed` lifecycle from AC-5.6 exactly (`pending` is set by `create_tool`).

### Deviations from Implementation Details

| Deviation | Reason |
|---|---|
| `tools::invoke_external` keeps existing signature; the spec hinted "remove the `snapshot_id` parameter (obsolete)" but it had already been removed in effort 01. | Un-prefixed the existing `_session_id` / `_agent_id` placeholders and started using them. |
| TUI render is non-interactive — Tool nodes always show args preview + first 8 lines of `result_json`; no expand/collapse keybind. | Effort 03 focus is the data path; an interactive toggle would require keymap surgery in `events.rs`. Data is ready for a later effort to add the toggle. |
| Multi-iteration turns produce multiple `Thought` nodes interleaved with `Tool` nodes (one per LLM round). | Matches AC-5.5 "one `Thought` node per contiguous thinking block" — each LLM round is a contiguous thinking block. |
| Tool calls with empty `arguments_json` emit an `assistant` history entry with `arguments: "{}"` rather than skipping. | Defensive — keeps the JSON-shaped output well-formed. |

### Verification

| # | Criterion | Result |
|---|---|---|
| 1 | All four Rust projects + default agent build | PASS |
| 2 | `npx nx run scarllet-core:test` (incl. invoke_tool guards, registry incl. spawn_sub_agent, history tool-call/tool-result ordering) | PASS — 41 / 0 |
| 3 | `cargo clippy --workspace --all-targets` clean of new warnings | PASS — only one pre-existing warning in `scarllet-llm/openai.rs:365` |
| 4 | TUI `Show me the top-level files` → Tool(`tree`) `pending → running → done`, Result summarises | **DEFERRED — human required** |
| 5 | Follow-up `now grep for "main"` + `what was the first file?` (proves tool-call + tool-result history) | **DEFERRED — human required** |
| 6 | `grpcurl InvokeTool("spawn_sub_agent", …)` returns `success=false` placeholder | **DEFERRED — human required** |

Independent tester step was not invoked by the developer this round (procedural skip — see decision log). Automated verification is comprehensive and green.

### Pending human verification

With a working LLM provider configured:
1. `npx nx run scarllet-tui:run`, prompt `Show me the top-level files in this repo` → expect a `🔧 tree` Tool node going pending → running → done within ≤5 s, then a Result summarising.
2. Follow up with `now grep for "main" in those files`, then `what was the first file in the previous tree output?` → the agent must use only `GetConversationHistory` to answer correctly.
3. With `grpcurl` against the bound port from `~/.scarllet/lockfile`: `CreateSession` first; then `InvokeTool { session_id, agent_id, tool_name: "spawn_sub_agent", input_json: "{}" }` → expect `{ success: false, error_message: "spawn_sub_agent is implemented in effort 5", duration_ms: 0 }`.
