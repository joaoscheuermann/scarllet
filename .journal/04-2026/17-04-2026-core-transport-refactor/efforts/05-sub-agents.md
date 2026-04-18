---
status: done
order: 5
created: 2026-04-17 13:18
title: "Sub-agents via built-in spawn_sub_agent tool"
---

## Description

Replace the effort-3 stub with the real core-internal `spawn_sub_agent` implementation. When an agent invokes the tool, core spawns the requested agent module as a child process with `parent_id = <calling_agent_id>`, creates the nested `Agent` node under the parent's `Tool` node, and parks the parent's `InvokeTool` call on a `oneshot` channel until the sub-agent emits its `Result` node. On completion, the child process exits, the parent's Tool node is updated with the summarised result, and control returns to the parent agent. Enforce AC-8.4 — a parent cannot emit `TurnFinished` while any of its sub-agents are still running. Add the TUI truncation/expand behaviour from AC-11.5.

## Objective

With two agent modules available (the existing `default` plus another module that handles the inner task), ask the parent agent:

> Use spawn_sub_agent to run a 'default' agent that summarises this repo, then reply with its summary.

Observe:

1. Parent's `Agent` node begins streaming Thought tokens.
2. A `Tool` node appears under the parent with `tool_name = "spawn_sub_agent"` and status `running`.
3. A nested `Agent` subtree appears under that `Tool` node, with its own Thought / Tool / Result nodes streaming live. While the sub-agent is running, the TUI renders this nested subtree in a **truncated / collapsed** form (e.g. last 3 lines + spinner).
4. When the sub-agent emits `Result + TurnFinished`, the nested subtree collapses to a single summary line; the parent's `Tool` node updates to `status = done` with `result_json` containing the sub-agent's `Result.content`.
5. Parent agent continues, produces its own Thought/Result incorporating the summary.
6. Pressing `Esc` while the sub-agent is streaming kills the sub-agent first, then the parent — verifying cascade cancellation.

## Implementation Details

### 1. Core — `tools.rs`

Replace the effort-3 stub branch with a real dispatch:

```rust
if tool_name == "spawn_sub_agent" {
    return agents::spawn::handle_spawn_sub_agent(sessions, session_id, agent_id, input_json).await;
}
```

`handle_spawn_sub_agent` returns a `ToolResult` structure compatible with `InvokeToolResponse`.

### 2. Core — `agents/spawn.rs::handle_spawn_sub_agent`

Signature: `async fn handle_spawn_sub_agent(sessions: &Arc<RwLock<SessionRegistry>>, session_id: &str, parent_agent_id: &str, input_json: &str) -> ToolResult`.

Steps:

1. Parse `input_json` for `{ agent_module: String, prompt: String }`. On parse error return `ToolResult { success: false, error_message: "invalid spawn_sub_agent args: …" }`.
2. Look up `agent_module` in `ModuleRegistry` (Agent kind). If missing → `ToolResult { success: false, error_message: "agent module '<name>' not registered" }`.
3. Find the parent agent's **most recent `Tool` node** whose `tool_name == "spawn_sub_agent"` and whose `arguments_json` matches — that is the node the parent just created before issuing the `InvokeTool` call. Use it as the parent for the sub-agent's `Agent` node. If not found (defensive) — create a new `Tool` node ourselves under the parent agent's Agent node and proceed.
4. Generate `child_agent_id = Uuid::new_v4()`.
5. Acquire write lock on session; `NodeStore::create(Agent { id = child_agent_id, parent = Some(parent_tool_node_id), agent_module, agent_id = child_agent_id, status = "running" })`; broadcast `NodeCreated`.
6. Create `(tx, rx) = oneshot::channel::<Result<ResultPayload, String>>()`.
7. `session.agents.sub_agent_waiters.insert(child_agent_id, tx)`.
8. Release the lock. Call `spawn_sub_agent_process(session_id, child_agent_id, parent_agent_id, agent_module_path, prompt)` — same env-based launch as `spawn_main_agent`, but with `SCARLLET_PARENT_ID = parent_agent_id`.
9. `let result = rx.await;`
10. Map:
    - `Ok(Ok(result_payload))` → `ToolResult { success: true, output_json: json!({ "content": result_payload.content, "finish_reason": result_payload.finish_reason }).to_string(), duration_ms }`.
    - `Ok(Err(msg))` → `ToolResult { success: false, error_message: msg }`.
    - `Err(_)` (oneshot dropped = child never finished) → `ToolResult { success: false, error_message: "sub-agent terminated unexpectedly" }`.

### 3. Core — `agents/mod.rs::AgentRegistry`

- Add `sub_agent_waiters: HashMap<String /* child_agent_id */, oneshot::Sender<Result<ResultPayload, String>>>`.
- Add `fn any_descendant_running(&self, agent_id: &str, nodes: &NodeStore) -> bool` — returns true if any node in the sub-tree of `agent_id` corresponds to a currently-registered agent. Used for the AC-8.4 invariant.

### 4. Core — `agents/stream.rs`

Extend handling:

- On `AgentOutbound::CreateNode(Result)` from an agent whose `agent_id` is in `sub_agent_waiters`:
  1. Store the node as normal + broadcast.
  2. The waiter is **not** fired yet — wait for `TurnFinished` so that we fire with the final state.
- On `AgentOutbound::TurnFinished` for a sub-agent (detected by `sub_agent_waiters.contains_key(agent_id)`):
  1. Mark the sub-agent's Agent node `agent_status = "finished"`.
  2. Find the most recent Result node parented to the sub-agent's Agent node; extract its `ResultPayload`.
  3. `if let Some(tx) = waiters.remove(agent_id) { let _ = tx.send(Ok(result_payload)); }`
  4. Deregister from AgentRegistry, broadcast `AgentUnregistered`.
- On unexpected disconnect / `AgentFailure` for a sub-agent: fire the waiter with `Err(message)`; otherwise same deregister path.
- On `AgentOutbound::TurnFinished` for a parent agent: **before** applying it, call `AgentRegistry::any_descendant_running(parent_agent_id, &nodes)`. If true:
  1. Create a top-level `Error` node `"invariant violation: agent '<id>' tried to finish with running sub-agents"`.
  2. Send `CancelNow` to the parent and all descendants; kill their PIDs after grace period.
  3. Mark all their Agent nodes `agent_status = "failed"`.
  4. Reject the `TurnFinished`.

### 5. Core — synthetic manifest visible in `GetToolRegistry`

(Already advertised in effort 3; now the runtime backing is real.) Ensure the manifest's `input_schema_json` matches what agents expect:

```json
{
  "type": "object",
  "properties": {
    "agent_module": { "type": "string", "description": "name of an installed agent module" },
    "prompt": { "type": "string", "description": "instruction for the sub-agent" }
  },
  "required": ["agent_module", "prompt"]
}
```

### 6. SDK — `scarllet-sdk/src/agent/mod.rs`

Convenience wrapper:

- `pub async fn spawn_sub_agent(&mut self, agent_module: &str, prompt: &str) -> Result<ResultPayload, AgentSdkError>` — constructs the input JSON, calls `self.invoke_tool("spawn_sub_agent", ...)`, parses the output_json into a `ResultPayload`. Propagates errors as `AgentSdkError::SubAgent(message)`.

The default agent does **not** need to know the wrapper exists — the normal tool-calling loop routes through `InvokeTool` with `tool_name == "spawn_sub_agent"`. But the wrapper is handy for human-authored agents and future commands.

### 7. TUI — render.rs / widgets/chat_message.rs (AC-11.5)

- When rendering a `Tool` node whose `tool_name == "spawn_sub_agent"`, detect child nodes (via `parent_id`) that represent the sub-agent subtree.
- While the Tool node's `tool_status ∈ {pending, running}`:
  - Render a compact card: `🧬 spawn_sub_agent('<module>') [running] … last 3 lines of the subtree …` with a spinner.
  - Hide the full subtree from the flat list (don't render its children at top level).
- When the Tool node's `tool_status ∈ {done, failed}`:
  - Render only: `🧬 spawn_sub_agent('<module>') [done in Nms] → <summary excerpt>` from `tool_result_json.content`.
  - Provide an `expand` toggle (e.g. `Enter` while the card is focused, or a persistent `+` marker) — expanded shows the full nested subtree.

### 8. Example scenario (for verification)

Use a simple secondary agent. Options:

- (A) Reuse `default` twice — parent calls `spawn_sub_agent('default', 'summarise <X>')`. Simplest, no new code.
- (B) Add a dedicated `packages/rust/agents/summariser/` — outside the core refactor scope; decline unless free.

Go with option A for verification.

## Verification Criteria

1. All crates build; `cargo clippy --workspace --all-targets` clean.
2. `npx nx run scarllet-core:test` — new tests cover:
   - `AgentRegistry::any_descendant_running` happy + unhappy cases.
   - `handle_spawn_sub_agent` returning correct `ToolResult` for: missing module, happy path (using a fake spawn channel), and sub-agent-failure path.
   - Invariant: a parent `TurnFinished` with a registered descendant produces the reject + cascade path.
3. **Run & observe (required)**: launch TUI, use prompt from §Objective. Observe the full sequence: parent Thought → Tool(running) + nested Agent subtree (truncated) → sub-agent streams → Tool(done) with summary → parent resumes → parent Result.
4. **Run & observe (required)**: while the sub-agent is streaming, press `Esc`. Observe both processes exit (confirm via `Get-Process scarllet-default-agent` — both PIDs gone). Both `Agent` nodes show `failed`.
5. **Run & observe**: manually craft an agent (or tweak the default agent temporarily) that emits `TurnFinished` while its sub-agent is still running. Observe the invariant top-level Error node and cascade kill of both.
6. **Run & observe**: expand toggle — after the sub-agent finishes, press the expand key on the spawn_sub_agent Tool node and confirm the full nested subtree becomes visible.

## Done

- `spawn_sub_agent` runs real sub-agent processes with correct parent/child node wiring.
- AC-8.4 invariant enforced: no parent `TurnFinished` while descendants are running.
- Session-wide Stop cascades to sub-agents first.
- TUI truncation + expand behaviour matches AC-11.5.
- SDK has a `spawn_sub_agent` convenience wrapper available to future agent authors.

## Change Summary

### Files modified

- `packages/rust/scarllet-core/src/tools.rs` — `invoke()` gained `sessions: &Arc<RwLock<SessionRegistry>>` + `core_addr` params; `spawn_sub_agent` branch delegates to `agents::spawn::handle_spawn_sub_agent`. `SPAWN_SUB_AGENT_DESCRIPTION` / `SPAWN_SUB_AGENT_INPUT_SCHEMA` refreshed.
- `packages/rust/scarllet-core/src/agents/mod.rs` — `AgentRegistry` gained `register_sub_agent_waiter`, `take_sub_agent_waiter`, `has_sub_agent_waiter`, `any_descendant_running`, `descendant_agent_ids`, `set_pid`. 6 new unit tests.
- `packages/rust/scarllet-core/src/agents/spawn.rs` — `handle_spawn_sub_agent` (+ `handle_spawn_sub_agent_with` test seam), `spawn_sub_agent_process`, `SubAgentSpawnArgs`, `SpawnSubAgentInput::parse`. Refactored `spawn_main_agent` + sub-agent spawn to share `spawn_agent_process`. 5 new unit tests.
- `packages/rust/scarllet-core/src/agents/stream.rs` — extracted sync `process_turn_finished(&mut Session, …) -> TurnFinishedOutcome` with sub-agent / AC-8.4 / main branches; helpers `finish_sub_agent`, `enforce_ac_8_4_invariant`, `emit_top_level_error`, `schedule_pid_kill`, `kill_pid_best_effort`. `handle_failure` / `handle_disconnect` fire sub-agent waiters with `Err(message)` before deregister. 4 new unit tests.
- `packages/rust/scarllet-core/src/agents/routing.rs` — `PendingDispatch { prompt, pid }` so `handle_register` can propagate real PIDs to `AgentRecord`.
- `packages/rust/scarllet-core/src/service/tool_rpc.rs` — threads `svc.sessions` + `svc.bound_addr` into `tools::invoke`.
- `packages/rust/scarllet-sdk/src/agent/mod.rs` — `AgentSdkError::SubAgent(String)` + `spawn_sub_agent(agent_module, prompt) -> Result<ResultPayload, AgentSdkError>` convenience wrapper.
- `packages/rust/scarllet-tui/src/app.rs` — `expanded_tools: HashSet<String>`, `descendants_of`, `toggle_spawn_sub_agent_expand`, `is_spawn_sub_agent_tool`; removed unused `children_of`.
- `packages/rust/scarllet-tui/src/events.rs` — `Enter` in `Focus::History` toggles expand state for `spawn_sub_agent` Tool nodes under the focused top-level.
- `packages/rust/scarllet-tui/src/render.rs` — passes full `descendants_of(...)` list + `expanded_tools` into widget constructor.
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs` — rewritten for AC-11.5: `spawn_sub_agent` Tool renders as compact running card (last-3-lines preview + spinner) while `pending`/`running`; terminal summary line with excerpt while `done`/`failed`; full nested subtree when expanded (with `[+]` / `[-]` marker).
- `packages/rust/scarllet-tui/Cargo.toml` — added `serde_json = "1"` so the widget can parse `arguments_json` / `tool_result_json`.

### Files created / deleted

None.

### Key decisions / trade-offs

- **Sub-agent dispatch reuses `pending_dispatch` + `handle_register`** for DRY. `QueuedPrompt.user_node_id` stores the parent Tool node id for traceability (no real User node drives the sub-agent).
- **`SpawnSubAgentInput` parsed via `serde_json::Value` + manual extraction** — avoids adding a `serde` dep to `scarllet-core` (which only depends on `serde_json` directly).
- **PID kill uses `taskkill /F /PID` (Windows) / `kill -KILL` (Unix) shelled out from `schedule_pid_kill`** behind a 500 ms grace period — best-effort last resort; avoids `libc` / `windows-sys` deps.
- **Sync `process_turn_finished` returning `TurnFinishedOutcome`** so AC-8.4 invariant cascade + happy paths are unit-testable without a real gRPC `StreamDeps`. The async wrapper releases the session lock before scheduling PID kills.
- **`find_parent_tool_node_id` falls back to "most recent spawn_sub_agent Tool child"** when `arguments_json` doesn't match exactly — defensive against future argument-injection variations.
- **TUI `Enter` from history focus toggles expand state for all `spawn_sub_agent` Tool nodes under the focused top-level** — minimum surface change, since the TUI focuses top-level messages, not individual Tool children. `[+]` / `[-]` marker provides the persistent affordance.
- **Sub-agent crash path reuses `handle_failure` / `handle_disconnect`**, just detects `has_sub_agent_waiter(agent_id)` and fires `Err(message)` before the same deregister path.

### Deviations from Implementation Details

| Deviation | Reason |
|---|---|
| `handle_spawn_sub_agent` signature is `(sessions, registry, core_addr, session_id, parent_agent_id, input_json)` (effort showed 4 args). | The agent module path and the loopback address must be threaded explicitly. |
| Sub-agent `QueuedPrompt.working_directory` is empty (falls back to core's cwd at spawn time). | Parent's cwd is not persisted on `AgentRecord`; inheriting would require threading the parent's task cwd into the record. Sub-agents still see a valid cwd via `current_dir()` fallback. Future follow-up. |
| `AgentRegistry` exposes helper methods (`register_sub_agent_waiter` / `take_sub_agent_waiter` / `has_sub_agent_waiter`) instead of making the field directly `pub`. | Narrower API; matches existing register/deregister pattern. |
| Added `descendant_agent_ids` alongside `any_descendant_running`. | Cascade path needs to iterate the subtree once; `any_descendant_running` stays as the fast bool check. |
| `tools::invoke` grew `sessions` + `core_addr` params. | Matches how `service::tool_rpc::invoke_tool` already has access via `svc`; explicit params keep the function pure. |
| `expanded_tools: HashSet<String>` lives on `App`, not on the per-frame widget. | Widgets are rebuilt every frame; expand state must survive a frame. |
| `serde_json = "1"` added to `scarllet-tui/Cargo.toml`. | Widget now parses `arguments_json` / `tool_result_json`. |

### Verification

| # | Criterion | Result |
|---|---|---|
| 1 | All crates build + clippy clean of new warnings | PASS |
| 2 | `npx nx run scarllet-core:test` (incl. `any_descendant_running` happy + unhappy + finished-skipped, `handle_spawn_sub_agent` missing-module / happy / waiter-failure, AC-8.4 cascade) | PASS — 65 / 0 |
| 3 | TUI: `Use spawn_sub_agent to run a 'default' agent that summarises this repo …` → parent Thought → spawn card running → sub-agent streams → terminal summary → parent Result | **DEFERRED — human required** |
| 4 | TUI: `Esc` mid-sub-agent → both processes die, both Agent nodes `failed` | **DEFERRED — human required** |
| 5 | AC-8.4: parent emits `TurnFinished` with running sub-agent → top-level invariant `Error` + cascade kill | **DEFERRED — human required (requires temporarily mis-built parent agent)** |
| 6 | TUI: expand toggle on completed spawn card reveals nested subtree | **DEFERRED — human required** |

Independent tester invoked and confirmed PASS for criteria 1–2, DEFERRED for 3–6. No bugs filed.

### Pending human verification

With a working LLM provider configured:
1. `npx nx run scarllet-tui:run`. Prompt: `Use spawn_sub_agent to run a 'default' agent that summarises this repo, then reply with its summary.` → expect a `🧬 spawn_sub_agent('default') [running]` card with last-3-lines preview, then `[done in Nms] → …` summary on completion, then the parent's own Result.
2. Repeat scenario 1; while the sub-agent is streaming press `Esc`. Run `Get-Process default-agent` in another PowerShell — expect zero matches. Both Agent nodes show `failed`.
3. Temporarily modify `packages/rust/agents/default/src/main.rs` so `emit_result` fires before awaiting the sub-agent's `InvokeTool` response. Re-run scenario 1. Expect a top-level red `Error` node `"invariant violation: agent '…' tried to finish with running sub-agents"`, both Agent nodes `failed`, both processes exited. Revert the modification afterwards.
4. After scenario 1 completes, press `Up` to focus the message with the `[+] 🧬 spawn_sub_agent(...)` card, press `Enter` — expect the marker to flip to `[-]` and the full nested subtree to render. Press `Enter` again to collapse.
