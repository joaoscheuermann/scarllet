---
status: done
order: 2
created: 2026-04-17 13:18
title: "Streaming LLM output via Thought + partial UpdateNode"
---

## Description

Add the `UpdateNode` partial-patch path with the merge rules locked in the architecture (append for `thought_content` / `result_content`; replace everywhere else). Wire the default agent back through `scarllet-llm` so each turn spawns a real LLM request, streams its tokens into a single `Thought` node that grows in place, and emits a final `Result` node — the one-updated streaming model from AC-5.5. Implement `GetConversationHistory(session_id)` in core so the agent can reconstruct multi-turn context; implement `GetActiveProvider(session_id)` to return the session's snapshot.

This is the first effort where the TUI shows real streaming behaviour.

## Objective

Launch the TUI with a valid provider configured. Ask `What is 2 + 2?`. Observe:

1. A `User` node with the prompt.
2. An `Agent` node immediately below.
3. A `Thought` node whose text fills in character-by-character as the LLM streams tokens (visible word-by-word at ≥10 fps).
4. On stream end, a `Result` node appears with the final answer.
5. Ask a second follow-up (`and times 3?`) — the agent includes prior context in its LLM request and answers `12`.

## Implementation Details

### 1. Core — `session/nodes.rs`

Implement `NodeStore::update(id, patch)`:

- Reject if `id` is not in `by_id`.
- Apply per-field rules:
  - `thought_content` — append to the existing `ThoughtPayload::content`.
  - `result_content` — append to the existing `ResultPayload::content`.
  - `result_finish_reason` — replace.
  - `agent_status`, `tool_status`, `tool_duration_ms`, `tool_result_json` — replace (tool fields used from effort 3 but rules encoded now).
  - `error_message`, `token_total`, `token_window` — replace (used from effort 7).
- Update `updated_at` to `now_ms`.
- Return `&Node`.

Add unit tests for each append vs replace rule + update-unknown-id rejection.

### 2. Core — `session/diff.rs`

Add `broadcast_node_updated(&Session, id: &str, patch: &NodePatch, updated_at: u64)` — sends `SessionDiff::NodeUpdated`.

### 3. Core — `agents/stream.rs`

Handle `AgentOutbound::UpdateNode { node_id, patch }`:

- Validate the node exists and the calling agent owns it (node's `parent_id` chain root must be the agent's Agent node, or node's id is the agent's Agent node). `Agent`-node mutations are allowed only by core — reject agent `UpdateNode(AgentPayload)` attempts.
- Apply via `NodeStore::update`, broadcast `NodeUpdated`.

### 4. Core — `service/agent_rpc.rs`

- `get_active_provider(session_id)` — read `session.config.active_provider` and return `ActiveProviderResponse` (type, url, key, model, reasoning effort).
- `get_conversation_history(session_id)` — walk `NodeStore::all()` and emit `HistoryEntry`s:
  - Top-level `User` → `{ role: "user", content: user.text }`.
  - Top-level `Agent` → find its child `Result` node; if present emit `{ role: "assistant", content: result.content }` (ignore Agent nodes with no Result — still running or failed).
  - Tool nodes under Agents — not included yet (added in effort 3 once `Tool` payload carries LLM-formatted args + result).

### 5. SDK — `scarllet-sdk/src/agent/mod.rs`

Add:

- `get_provider(&mut self) -> Result<ActiveProviderResponse, _>`.
- `get_history(&mut self) -> Result<Vec<HistoryEntry>, _>`.
- `create_thought(&self, parent_agent_node_id: &str) -> Result<String, _>` — emits `CreateNode(Thought { parent = parent_agent_node_id, content = "" })` and returns the new node_id.
- `append_thought(&self, node_id: &str, chunk: &str) -> Result<(), _>` — emits `UpdateNode { node_id, patch: { thought_content: Some(chunk) } }`.
- `create_result(&self, content: &str) -> Result<String, _>` — emits `CreateNode(Result { parent = self.agent_node_id, content, finish_reason: "" })` and returns the new node_id.
- `append_result_content(&self, node_id: &str, chunk: &str)` — patches `result_content` with the delta (used in later efforts; emit nothing else for effort 2).
- Update `emit_result` behaviour: continue to exist but becomes a convenience wrapper that `create_result` + `TurnFinished`. Prefer streaming-friendly helpers going forward.

### 6. Default agent — `agents/default/src/main.rs`

Rewrite the turn handler. Keep `--manifest` path.

- After `next_task`, fetch provider via `session.get_provider()` and history via `session.get_history()`.
- Construct an `LlmClient` from provider (existing `scarllet-llm` logic).
- Build the chat request: system prompt + prior history + the new user message (from `task.prompt`). No tools yet (tools land in effort 3).
- Call `llm.chat_stream(request).await`. For each `ChatStreamEvent`:
  - At start, if no thought node has been created yet and the event carries `deltas`, `let thought_id = session.create_thought(task.agent_id).await?`.
  - For each `StreamDelta::Thought(t) | StreamDelta::Content(t)`: `session.append_thought(&thought_id, &t).await?`.
  - Track `finish_reason` and last `Usage` (usage emission lives in effort 7).
- When the stream ends: build the aggregated content string and emit the Result + TurnFinished via `session.emit_result(&content, &finish_reason).await?`.
- Agent process exits naturally after the stream closes (core already handles per-turn cleanup).

No tool-call branch yet — if the LLM emits tool_calls without tools defined, treat it as an error and emit failure.

### 7. TUI render — `packages/rust/scarllet-tui/src/render.rs` + `widgets/`

- Add a Thought-node renderer (dimmed styling, optional prefix `thought:`).
- Handle `SessionDiff::NodeUpdated` in `app.rs`: look up node, apply patch (mirror of core's merge rules), re-render.
- Keep typewriter animation optional — the stream itself paces the update now.

## Verification Criteria

1. `npx nx run scarllet-proto:build` / `scarllet-core:build` / `scarllet-sdk:build` / `scarllet-tui:build` — all green.
2. `npx nx run scarllet-core:test` — includes new tests for `NodeStore::update` (append rules, replace rules, unknown-id rejection) and `GetConversationHistory` derivation (user → Agent+Result → user → Agent).
3. `cargo clippy --workspace --all-targets` — clean.
4. **Run & observe (required)**: with a real provider configured in `config.json`, `npx nx run scarllet-tui:run`. Ask `What is 2 + 2?`. Observe the Thought node filling with streaming tokens over >500 ms (not all at once). When the stream ends a Result node appears with the final answer.
5. **Run & observe**: send a follow-up `and times 3?` in the same session. The next turn's Thought / Result references the prior `2 + 2 = 4` context, proving `GetConversationHistory` is wired. Expected final answer: `12`.
6. **Run & observe**: crash the provider (invalid API key in `config.json` for one send). The turn emits `AgentFailure`, the Agent node is marked `failed` (visible as a red status in the TUI), and the next prompt works after fixing the key (Paused-state recovery lands in effort 6; for now acceptable behaviour is "the agent process exits, the TUI returns to idle").

## Done

- `NodeStore::update` implements append-for-content vs replace-for-scalars.
- `UpdateNode` wire path works end-to-end.
- `GetActiveProvider` and `GetConversationHistory` return useful data.
- Default agent streams real LLM output into a single Thought node and emits a Result node at end.
- Multi-turn conversation context is preserved across turns.
- The TUI renders Thought nodes inline with Result nodes.

## Change Summary

### Files modified

- `packages/rust/scarllet-core/src/session/nodes.rs` — `NodeStore::update(id, patch, updated_at)` + `apply_patch` helper + `InvariantError::UnknownNode` variant + 7 unit tests for append / replace / unknown-id rules.
- `packages/rust/scarllet-core/src/session/diff.rs` — promoted `node_updated` + added `broadcast_node_updated(&mut Session, …)` convenience wrapper.
- `packages/rust/scarllet-core/src/session/state.rs` — pure `conversation_history(&NodeStore) -> Vec<HistoryEntry>` derivation + 5 unit tests covering empty / two-entry / multi-turn / agent-without-result / nested-payload-filtering.
- `packages/rust/scarllet-core/src/service/agent_rpc.rs` — `get_conversation_history` now derives via `state::conversation_history` instead of returning empty.
- `packages/rust/scarllet-core/src/agents/stream.rs` — `handle_update_node` (rejects empty / missing fields, unknown agent, unknown node, Agent-payload edits, ownership violations) + `node_owned_by_agent` chain walker; the `UpdateNode` arm in `drive_main_loop` dispatches to it.
- `packages/rust/scarllet-sdk/src/agent/mod.rs` — added `get_provider`, `get_history`, `create_thought`, `append_thought`, `create_result`, `append_result_content`, `emit_failure`, internal `send_outbound`. `emit_result` is now a wrapper around `create_result + UpdateNode(finish_reason) + TurnFinished`.
- `packages/rust/agents/default/src/main.rs` — full rewrite. Real LLM turn: provider fetch → history fetch → `LlmClient` → `chat_stream` → lazy `create_thought` + per-delta `append_thought` → final `emit_result`. Failures route through `emit_failure`. Rejects `tool_calls` until effort 03.
- `packages/rust/agents/default/Cargo.toml` — added `tokio-stream = "0.1"`.
- `packages/rust/scarllet-tui/src/app.rs` — `apply_node_patch(id, patch, updated_at)` mirrors core's merge rules; silently drops updates targeting unknown ids (Ctrl-N race tolerance).
- `packages/rust/scarllet-tui/src/events.rs` — `SessionDiff::NodeUpdated` arm applies the patch via `app.apply_node_patch`.
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs` — Thought renderer (italic + `DarkGray`, `thought:` prefix, multi-line aware) under the Agent header.

### Files created / deleted

None.

### Key decisions / trade-offs

1. **`NodeStore::update` takes `updated_at: u64` as a parameter** rather than calling `now_*()` internally. Keeps unit tests deterministic, lets the broadcast helper reuse the same timestamp, matches DI conventions.
2. **Time precision is seconds, not millis** — the rest of the codebase already uses Unix-epoch seconds (`now_secs()`, `to_unix_secs`); the proto field has no semantics attached.
3. **Ownership check walks the parent chain** (`node_owned_by_agent`) — direct parent suffices today but the chain walker generalises to sub-agents (effort 05).
4. **`apply_patch` ignores patch fields whose payload variant doesn't match the node's kind** — fail-soft against future protocol bugs; bad patches are already rejected upstream by the stream handler.
5. **`emit_result` patches `finish_reason` via `UpdateNode` after `create_result`** — `Result` content is created in full (no streaming Result text in this effort), but `finish_reason` lives in the same payload; a second `CreateNode` would violate the no-deletion / single-Result invariant.
6. **TUI `apply_node_patch` silently drops updates for unknown ids** — Ctrl-N race tolerance.
7. **Default agent treats `tool_calls` from the LLM as a turn failure** — per the effort: tool support is effort 03.

### Deviations from Implementation Details

| Deviation | Reason |
|---|---|
| `NodeStore::update` signature has an extra `updated_at: u64` parameter (spec showed two args). | Pure / testable / matches the diff timestamp explicitly. |
| `broadcast_node_updated` takes `&mut Session` (spec showed `&Session`). | Broadcasting requires `&mut` to call `subscribers.broadcast`; spec's `&Session` would not compile. |
| TUI thought renderer uses italic + `DarkGray` foreground (spec said "dimmed styling, optional prefix `thought:`"). | Matches existing `Color::DarkGray` usage; `Modifier::ITALIC` is the only "dimmed" modifier ratatui exposes that doesn't fade the prefix into invisibility on light themes. |
| No "typewriter animation" toggle in the TUI. | Effort allows "stream paces it" — defaulted to that; no extra animation code added. |

### Verification

| # | Criterion | Result |
|---|---|---|
| 1a | `npx nx run scarllet-proto:build` | PASS |
| 1b | `npx nx run scarllet-core:build` | PASS |
| 1c | `npx nx run scarllet-sdk:build` | PASS |
| 1d | `npx nx run scarllet-tui:build` | PASS |
| 1e | `cargo build -p default-agent` | PASS |
| 2 | `npx nx run scarllet-core:test` (incl. new `NodeStore::update` append/replace/unknown-id + `conversation_history` derivation tests) | PASS — 33 / 0 |
| 3 | `cargo clippy --workspace --all-targets` | PASS — zero new warnings; baseline warnings unchanged |
| 4 | TUI: `What is 2 + 2?` shows streaming Thought + final Result | **DEFERRED — human required** |
| 5 | Multi-turn: `and times 3?` → `12` (proves `GetConversationHistory` wiring) | **DEFERRED — human required** |
| 6 | Invalid API key → `AgentFailure`; recovery after fix | **DEFERRED — human required** |

Independent tester ran in a sandboxed read-only environment (Shell tool returned empty exit-0); it performed thorough static inspection and reported all touched files / new tests / wire path verified. The developer's own command runs (results above) cover what the tester sandbox could not execute. No defects identified by either party.

### Pending human verification

With a working LLM provider configured in `config.json`, run `npx nx run scarllet-tui:run` and walk:
1. `What is 2 + 2?` → expect a `thought:` block under the Agent header that grows token-by-token over >500 ms, followed by a `Result` line with the answer.
2. Same session, follow up with `and times 3?` → expect the answer `12`, proving server-derived history reached the LLM.
3. Edit `config.json` to inject a bad API key, send a prompt → expect either an `Error` line under the Agent header or a quick exit-to-idle (full Paused recovery is effort 06). Restore the key and confirm a normal turn works again.
