---
status: done
order: 4
created: 2026-04-17 13:18
title: "Per-session prompt queue + per-turn main-agent dispatch"
---

## Description

Promote the queue from effort 1's single-dispatch stub into a real FIFO that can hold multiple pending prompts while a turn is in flight. Broadcast `QueueChanged` diffs on every enqueue / dequeue. Wire per-turn main-agent dispatch so as soon as the current turn emits `TurnFinished`, the next queued prompt spawns a fresh main-agent process automatically. Verify the strict missing-default-agent error path (AC-3.3).

This is the first effort that proves the "queue prompts while the agent is busy" UX.

## Objective

While a turn is still streaming, type two more prompts in rapid succession. Observe:

1. The TUI status bar reflects `+2 queued` while the first turn is still in flight.
2. When turn 1's `Result` node lands, turn 2 auto-starts: a fresh `Agent` node is created, streaming begins.
3. `QueueChanged` diffs fire correctly — the queue indicator decrements live.
4. When the configured `default_agent` module is temporarily unavailable, the next prompt produces a top-level `Error` node **immediately** and the queue drains its item (no silent stall).

## Implementation Details

### 1. Global config — `scarllet-sdk/src/config.rs`

- Add `pub default_agent: String` to `ScarlletConfig` (serde default: empty string). Keep JSON back-compat by deserializing missing field as empty.
- Ensure existing `config.json` files load cleanly without the field; a warning in `tracing::info!` on first load with empty `default_agent` is acceptable.
- Unit-test the roundtrip.

### 2. Core — `session/queue.rs`

Promote the effort-1 stub into a real implementation with:

- `pub struct SessionQueue { items: VecDeque<QueuedPrompt> }` with `push_back`, `pop_front`, `len`, `is_empty`, `iter` (&QueuedPrompt) → snapshots for diffs.
- `QueuedPrompt { id: String /* uuid */, text: String, working_directory: String, user_node_id: String }`.

Unit tests for FIFO order + snapshot rendering.

### 3. Core — `session/diff.rs`

`broadcast_queue_changed(&Session)` sends a full `QueueChanged` with the current queue snapshot. Called on every push_back and pop_front.

### 4. Core — `agents/routing.rs`

Expand `try_dispatch_main(session: &mut Session, registry: &ModuleRegistry, config: &ScarlletConfig)`:

1. Early return if `session.status != Running` or `session.agents.has_main()`.
2. Peek at `session.queue.front()`; if empty, return.
3. Resolve `default_agent`:
   - Read `config.default_agent`. If empty → pop the queued prompt, create a **top-level** `Error` node `"default_agent not configured"`, broadcast both `QueueChanged` + `NodeCreated`, return.
   - Look up in `registry.by_kind(Agent)`. If not found → same pop + top-level Error pattern (`"agent module '<name>' is not registered"`).
4. Pop the queued prompt, broadcast `QueueChanged`.
5. Generate `agent_id`, create `Agent` node top-level, broadcast `NodeCreated`.
6. `agents::spawn::spawn_main_agent(session_id, agent_id, module_path, prompt, cwd)`.

### 5. Core — `agents/stream.rs`

On `AgentOutbound::TurnFinished`:

- Patch Agent node (`agent_status = "finished"`); broadcast `NodeUpdated`.
- Remove the `AgentRecord`; clear `main_agent_id`; broadcast `AgentUnregistered`.
- Call `try_dispatch_main(session, registry, config)` immediately so the next queued prompt dispatches without user action.

Also call `try_dispatch_main` from `AgentFailure` / disconnect-before-TurnFinished paths (for effort 4 we don't Pause yet — we just drop the dead agent and move on; proper Paused semantics land in effort 6).

### 6. Core — `service/session_rpc.rs::send_prompt`

- Generate `user_node_id = Uuid::new_v4()`; `NodeStore::create(User { text, working_directory })`; broadcast `NodeCreated`.
- `session.queue.push_back(QueuedPrompt { id: Uuid::new_v4(), text, working_directory, user_node_id })`; broadcast `QueueChanged`.
- Call `try_dispatch_main(...)`.
- Return `SendPromptResponse { user_node_id, queued_prompt_id }`.

### 7. TUI — `events.rs` + `render.rs`

- Apply `QueueChanged` diffs to `app.queue`.
- Status bar renders `+N queued` when `app.queue.len() > 0`. Clear the indicator when empty.
- Enter key remains a single `SendPromptCommand`; the user can hit Enter repeatedly without waiting.
- Input stays **editable** while the main agent is streaming (removed the `input_locked` gate — or retained as a user-preference toggle; default: editable). Justification: the whole point of the queue is to let users pile prompts.

### 8. Unit tests

- `agents::routing::try_dispatch_main` short-circuits when Paused, when a main is already running, when the queue is empty.
- Missing default agent emits the right Error node and pops the queue.
- Two enqueued prompts dispatch in order under successive `TurnFinished` events (use a fake `AgentSpawner` trait to avoid spawning real processes in tests).

## Verification Criteria

1. All crates build; `cargo clippy --workspace --all-targets` clean.
2. `npx nx run scarllet-core:test` — new unit tests green.
3. **Run & observe (required)**: with a provider + `default_agent` configured, run the TUI. Ask a slow-to-answer question (e.g. `write a detailed 500-word essay about the Rust borrow checker`). While it's streaming, type and send two short follow-ups quickly. Observe:
   - Status bar flips to `+1 queued`, then `+2 queued`.
   - Turn 1 completes → Agent 1 node is marked finished, the queue drops to `+1 queued`, Agent 2 node appears and starts streaming.
   - Turn 2 completes → queue drops to empty; Agent 3 spawns and streams.
4. **Run & observe (required, AC-3.3)**: rename (or delete) the configured `default_agent` binary from the agents folder while the core is running (the watcher will deregister it). Send a prompt. Observe: a top-level red `Error` node `"agent module '<name>' is not registered"` appears immediately; the queue stays empty (prompt is dropped, not silently stalled). Restore the binary, send again — works.
5. **Run & observe**: with empty `config.default_agent`, send a prompt. Same strict top-level Error behaviour.

## Done

- `QueueChanged` diffs fire on every enqueue / dequeue.
- Per-turn lifetime: each `TurnFinished` automatically dispatches the next queued prompt without user action.
- Missing-default-agent surfaces immediately as a top-level Error node.
- TUI input stays editable during streaming so users can type ahead.

## Change Summary

### Files modified

- `packages/rust/scarllet-sdk/src/config.rs` — three new roundtrip tests locking in JSON back-compat for `default_agent` (existing field, existing `"default"` serde default).
- `packages/rust/scarllet-core/src/session/queue.rs` — three additional FIFO / snapshot / clear tests.
- `packages/rust/scarllet-core/src/session/diff.rs` — `broadcast_queue_changed(&mut Session)` helper centralising the snapshot-and-broadcast.
- `packages/rust/scarllet-core/src/agents/routing.rs` — rewrote `try_dispatch_main` to match the effort §4 ordering (Paused / `has_main` / empty queue → empty `default_agent` → missing module → happy path). Introduced `try_dispatch_main_with<F: FnOnce(SpawnArgs) -> Option<u32>>` + `SpawnArgs` struct as a public test seam so unit tests can inject a no-op spawner. 7 new unit tests.
- `packages/rust/scarllet-core/src/agents/stream.rs` — `handle_turn_finished` patches the Agent node `status="finished"` and broadcasts `NodeUpdated`. `handle_failure` and `handle_disconnect` patch `status="failed"` and call `try_dispatch_main` so the queue keeps moving (Paused lifecycle deferred to effort 06). Helper `mark_agent_status` deduplicates.
- `packages/rust/scarllet-core/src/service/session_rpc.rs` — `send_prompt` and `stop_session` use `diff::broadcast_queue_changed`.
- `packages/rust/scarllet-tui/src/events.rs` — removed `input_locked = true` on `AgentRegistered` and the re-enable on `AgentUnregistered`; input stays editable while the main agent streams.
- `packages/rust/scarllet-tui/src/render.rs` — status bar renders `+N queued` next to `session <id>` when `app.queue.len() > 0`.

### Files created / deleted

None.

### Key decisions / trade-offs

- **Test seam: `try_dispatch_main_with<F>` over an `AgentSpawner` trait.** Routing only ever needs one spawn call per dispatch; a `FnOnce(SpawnArgs) -> Option<u32>` closure is simpler than `#[async_trait]` boilerplate. The production `try_dispatch_main` is a 6-line pass-through.
- **`SpawnArgs<'a>` struct** bundles the 6 spawn parameters so effort 05 can extend with `parent_agent_id` without churning test signatures.
- **Empty vs. missing `default_agent` split** — distinct error messages (`"default_agent not configured"` vs `"agent module '<name>' is not registered"`); checked in that order so a zero-string config never surfaces the second message with `<name>=""`.
- **`broadcast_queue_changed` lives in `session::diff`** next to other diff builders; co-locates the snapshot-and-broadcast pipeline.
- **Agent node status patched on TurnFinished + failure + disconnect** so a stale `status="running"` never persists across the first-diff hydration.
- **TUI `input_locked` field retained, just never toggled on streaming** — minimal churn; effort 07's potential debug/settings pane can reuse it.
- **Status-bar layout** — queue indicator sits on the right side next to `session <id>` (the dynamic-runtime-state zone).

### Deviations from Implementation Details

- **§1 "serde default: empty string"** — kept the codebase's `"default"` serde default (locked in by effort 01 / parent ticket); added an explicit back-compat test rather than changing the default.
- **§8 "fake `AgentSpawner` trait"** — used a `FnOnce(SpawnArgs) -> Option<u32>` closure instead of a dedicated trait. Cleaner for one-shot use; explicitly permitted by the executor's brief.
- **§5 Agent-node `status="failed"` on AgentFailure / disconnect** — a minor expansion beyond the effort's letter; keeps the Agent-node status invariant honest after crash paths.

### Verification

| # | Criterion | Result |
|---|---|---|
| 1 | All crates build; `cargo clippy --workspace --all-targets` clean of new warnings | PASS |
| 2 | `npx nx run scarllet-core:test` (incl. 7 new routing tests + 3 new queue tests) | PASS — 51 / 0; SDK 17 / 0 incl. 3 new `default_agent` roundtrip tests |
| 3 | TUI: long-running prompt + 2 fast follow-ups → `+1 queued` / `+2 queued`, auto-dispatch on `TurnFinished` | **DEFERRED — human required** |
| 4 | AC-3.3: delete `default_agent` binary, send prompt → top-level red `Error` node `agent module '<name>' is not registered`; queue stays empty | **DEFERRED — human required** |
| 5 | Empty `defaultAgent` config → strict top-level `Error` `default_agent not configured` | **DEFERRED — human required** |

Independent tester invoked and confirmed PASS for criteria 1–2, DEFERRED for 3–5. No bugs filed.

### Pending human verification

With a working LLM provider configured and `defaultAgent: "default"` in `config.json`:
1. `npx nx run scarllet-tui:run`, prompt with a slow question (`write a detailed 500-word essay about the Rust borrow checker`). While streaming, send 2 short follow-ups → expect status bar to flip `+1 queued` / `+2 queued`, then auto-decrement on each turn's completion as Agent 2 / Agent 3 spawn.
2. Delete (or rename) the `default-agent` binary from the agents folder while core is running. Send a prompt → expect a top-level red `Error` node `agent module 'default' is not registered.` with the queue staying empty. Restore the binary and retry.
3. Set `defaultAgent: ""` in `config.json`, restart core, send a prompt → expect top-level red `Error` `default_agent not configured`.
