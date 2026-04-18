---
status: done
order: 6
created: 2026-04-17 13:18
title: "Session-wide Stop + Paused recovery"
---

## Description

Implement the full session-wide Stop flow with cascading cancellation (sub-agents first, then their parents, killed in reverse topological order) and proper handling for Paused state after an agent crash mid-turn. Earlier efforts already dropped dead agents on disconnect; this effort makes the state machine explicit — session flips to `Paused` so no new turns dispatch until the user acknowledges via `StopSession`, which clears the queue and returns to `Running`. Wire TUI `Esc` to `StopSession` and render a visible PAUSED indicator in the status bar.

## Objective

Two observable scenarios:

### A — Voluntary Stop during a turn

1. Send a prompt that produces a long answer (agent is still streaming Thought tokens).
2. Press `Esc`.
3. Observe: within ≤ 2 s the agent process exits cleanly, the Thought node's state reflects the cancel, the Agent node shows `failed` with an `Error` child `"cancelled by user"`, the queue is clear, the status bar returns to idle, and typing a new prompt works.
4. Repeat the same test while a `spawn_sub_agent` sub-agent is also running — observe sub-agent dies first, then parent; both marked `failed`.

### B — Involuntary crash → Paused state

1. Send a long-running prompt.
2. From a second terminal, hard-kill the running `scarllet-default-agent` process (`taskkill /F /IM scarllet-default-agent.exe` on Windows, `pkill -9 scarllet-default-agent` on Unix).
3. Observe in the TUI: within ≤ 2 s the agent's Agent node flips to `failed`, an `Error` node under it appears (`"agent disconnected unexpectedly"`), and the status bar shows `PAUSED`.
4. Type a new prompt and press Enter — it enqueues (User node appears, queue indicator `+1 queued`) but **no Agent is spawned**.
5. Press `Esc` — queue clears, status flips back to `Running`.
6. Send another prompt — dispatches normally.

## Implementation Details

### 1. Core — `session/mod.rs`

Ensure `SessionStatus` is `{ Running, Paused }`. Add a helper `Session::set_status(new_status) -> bool` that only broadcasts `StatusChanged` when the status actually changed (idempotent calls don't spam).

### 2. Core — `agents/stream.rs` — disconnect / failure path

Replace the effort-1 minimal disconnect handler with:

```
on stream close without TurnFinished, OR AgentOutbound::Failure, OR the stream returns Err:
  if agent is in sub_agent_waiters:
      fire waiter with Err("sub-agent terminated unexpectedly")
      (the parent's InvokeTool returns failure; parent decides what to do — typically it continues its turn)
  else (main agent):
      patch Agent node: agent_status = "failed"
      create Error node parented to the Agent node: source = agent_module, message = "<reason>"
      broadcast both
      session.set_status(Paused)  // blocks try_dispatch_main; AC-3.4
  deregister AgentRecord; broadcast AgentUnregistered
  kill PID if still alive
```

### 3. Core — `agents/routing.rs::try_dispatch_main`

Already short-circuits on `session.status != Running` (from effort 4). Keep that; no change needed.

### 4. Core — `service/session_rpc.rs::stop_session`

Implement the full cascade:

1. Acquire write lock on the session.
2. Build a topologically sorted list of agents: leaves first (sub-agents), roots last. Use `AgentRegistry::by_id` + the `Agent`-node parent chain.
3. For each agent in that order:
   - Send `AgentInbound::CancelNow` to its channel.
   - Spawn a detached task that waits `CANCEL_GRACE = 2 s`; if the PID is still alive, force-kill it (Unix: `SIGTERM` then `SIGKILL`; Windows: `taskkill /F`). This part can reuse / replace the existing Unix/Windows kill helpers from the old `tasks::cancel_task`.
   - Patch Agent node `agent_status = "failed"`; create an `Error` node parent = Agent node, message `"cancelled by user"`; broadcast both.
   - Deregister AgentRecord; broadcast `AgentUnregistered`.
   - If this agent has a waiting entry in `sub_agent_waiters`, fire the oneshot with `Err("cancelled by user")` so the parent's `InvokeTool` returns.
4. Clear `session.queue` (pop everything); broadcast one final `QueueChanged` with empty list.
5. If `session.status == Paused`, flip to `Running`; broadcast `StatusChanged`.
6. Return `StopSessionResponse { ok: true }`.

### 5. Core — `service/session_rpc.rs::destroy_session`

Use the same cascade (step 3) before removing the Session from `SessionRegistry`. After removal broadcast `SessionDestroyed`. Close all subscriber senders (or let them drain naturally when the session is dropped — either works; pick one and document).

### 6. TUI — `events.rs`

- Delete any leftover `Esc → CancelPrompt` code from effort 1.
- `Esc` while `app.session_status == Running` or `Paused` → send `StopSessionCommand` (connection layer issues the unary RPC).

### 7. TUI — `app.rs` + status bar render

- Store `session_status: SessionStatus` (updated via `StatusChanged` diffs).
- Status bar shows one of: `READY` (idle, Running, empty queue), `THINKING` (Running with a main agent alive), `+N queued` additions, `PAUSED` (red; overrides everything else until cleared).
- While `session_status == Paused`, the input area shows a hint `"Press Esc to resume"`.

### 8. Unit tests

- `stop_session` cascade correctness with a fake `SessionRegistry` containing parent + child agent records — verify kill order (leaves first) and final status flip.
- `set_status(Paused)` triggered by a disconnect broadcasts exactly one `StatusChanged`.
- `set_status` called twice with the same value does not double-broadcast (idempotence).

## Verification Criteria

1. All crates build clean; `cargo clippy --workspace --all-targets` clean.
2. `npx nx run scarllet-core:test` — new tests green.
3. **Run & observe (required, scenario A)**: execute Objective scenario A from start to finish with both `Esc`-mid-turn and `Esc`-during-sub-agent variants.
4. **Run & observe (required, scenario B)**: execute Objective scenario B, including the "new prompts queue but don't dispatch while Paused" check.
5. **Run & observe**: confirm `DestroySession` (fired via Ctrl-N) also cascades kills and broadcasts `SessionDestroyed`.

## Done

- `StopSession` cascades in reverse topological order and clears Paused + queue.
- Agent crash mid-turn flips the session to `Paused`; `try_dispatch_main` gates on status.
- TUI shows `PAUSED` indicator and prompts the user to press `Esc` to recover.
- `DestroySession` reuses the same cascade path.

## Change Summary

### Files modified

- `packages/rust/scarllet-core/src/session/mod.rs` — dropped `#[allow(dead_code)]` from `SessionStatus::Paused`; added `Session::set_status(new_status) -> bool` (idempotent, returns `true` only on real transition, touches `last_activity`); 3 new unit tests.
- `packages/rust/scarllet-core/src/session/diff.rs` — dropped `#[allow(dead_code)]` from `status_changed`; added `broadcast_status_changed(&mut Session)` paired with `Session::set_status`.
- `packages/rust/scarllet-core/src/agents/stream.rs` —
  - Split `CASCADE_KILL_GRACE_MS = 500` into `AC_8_4_KILL_GRACE_MS = 500` (invariant cascade) + `CASCADE_KILL_GRACE_MS = 2000` (session-wide stop); added `CANCEL_REASON = "cancelled by user"`.
  - `schedule_pid_kill(pid, grace_ms)` parameterised; `kill_pid_best_effort` elevated to `pub(crate)`.
  - `pub(crate) fn cascade_cancel(&mut Session, reason)` — sorts agents leaves-first via `agent_ids_leaves_first` + `depth_of`, sends `CancelNow`, schedules 2 s PID kill, fires every `sub_agent_waiter` with `Err(reason)`, patches Agent node `status="failed"`, emits per-agent `Error` child, deregisters + broadcasts `AgentUnregistered`.
  - Collapsed `handle_failure` + `handle_disconnect` into a shared sync `apply_agent_termination` helper that flips the session to `Paused` via `set_status` only for main agents (`has_sub_agent_waiter` is false) and only broadcasts on transition.
  - Disconnect message standardised to `"agent disconnected unexpectedly"` (matches Objective B.3).
  - 5 new unit tests.
- `packages/rust/scarllet-core/src/service/session_rpc.rs` — `stop_session` runs `cascade_cancel` first, clears queue + pending_dispatch + broadcasts `QueueChanged`, then flips `Paused → Running` via `set_status` (no longer spams `StatusChanged` on Running sessions). `destroy_session_inner` runs the same cascade before clearing queue/pending_dispatch and broadcasting `SessionDestroyed`.
- `packages/rust/scarllet-tui/src/app.rs` — dropped `#[allow(dead_code)]` from `SessionStatus::Paused`.
- `packages/rust/scarllet-tui/src/events.rs` — `Esc` sends `StopSessionCommand` when `session_status == Paused` **or** `(Running && is_streaming)`; preserves the existing `Focus::History → return_to_input` behaviour when neither trigger fires.
- `packages/rust/scarllet-tui/src/render.rs` — `draw_paused_hint` + `lifecycle_segment` helpers. 1-row hint slot (collapses to 0 rows when Running) above the input. Status bar renders `READY` / `THINKING` / `THINKING +N queued` / `+N queued` / `PAUSED`; PAUSED is red-bold and survives narrow-terminal truncation first.

### Files created / deleted

None.

### Key decisions / trade-offs

- **`cascade_cancel` lives in `agents/stream.rs`** as `pub(crate)`. Its helpers (`mark_agent_status`, `emit_error_under_agent`, `schedule_pid_kill`) already live there; moving to a new `agents/cascade.rs` would have been busywork.
- **Leaves-first ordering computed by depth** (HashMap walk computing depth from session root in O(n × d)), tie-broken on agent id for deterministic test ordering. Sufficient since `AgentRegistry` only stores flat `parent_id` links.
- **`set_status` is the only path that writes `session.status`.** Every `StatusChanged` broadcast is gated on `set_status` returning `true` — one invariant in one place.
- **`apply_agent_termination` is synchronous** so the Paused-flip + single-broadcast contract is unit-testable without plumbing a gRPC stream.
- **Paused input area keeps typing functional** — hint sits in a 1-row banner above the input (Objective B.4 needs typing-while-paused to enqueue).
- **Two grace constants over one parameter** (`AC_8_4_KILL_GRACE_MS = 500` vs `CASCADE_KILL_GRACE_MS = 2000`) — intent of each call site obvious at a glance.
- **Did not "fix" pre-existing workspace warnings** (`tools/*`, `scarllet-llm`, `scarllet-tui/{git_info,input}`); the effort's "clean of new warnings" bar is met. Workspace `-D warnings` is effort 08's job.

### Deviations from Implementation Details

- **Disconnect per-turn `Error` reads `"agent disconnected unexpectedly"`** rather than the effort's `"<reason>"` placeholder — matches Objective B.3 verbatim and gives TUIs a stable string to distinguish disconnect from in-band `AgentFailure`.
- **`destroy_session_inner` broadcasts `SessionDestroyed` after the cascade**, not before. Subscribers remain connected until the `Arc` releases, so they still receive every cascade broadcast plus the terminal `SessionDestroyed` diff.

### Verification

| # | Criterion | Result |
|---|---|---|
| 1 | All crates build; `cargo clippy --workspace --all-targets` clean of new warnings | PASS |
| 2 | `npx nx run scarllet-core:test` (incl. cascade order + final-status flip; `set_status(Paused)` from disconnect broadcasts exactly once; `set_status` idempotent under double-call) | PASS — 73 / 0 (+8 new) |
| 3 | Scenario A — `Esc` mid-turn (+ with sub-agent) → cascade kill, both agents `failed`, `Error("cancelled by user")`, queue clear | **DEFERRED — human required** |
| 4 | Scenario B — kill agent PID externally → Paused state; new prompt enqueues but doesn't dispatch; `Esc` clears + flips Running | **DEFERRED — human required** |
| 5 | Ctrl-N / `DestroySession` also cascades + broadcasts `SessionDestroyed` | **DEFERRED — human required** |

Independent tester invoked and confirmed PASS for criteria 1–2, DEFERRED for 3–5.

### Pending human verification

With a working LLM provider configured:
1. Send a prompt producing a long answer; while streaming press `Esc` → expect agent process exits within ≤2 s, Agent node `failed`, `Error` child `"cancelled by user"`, queue clear, status bar back to `READY`. Repeat with a `spawn_sub_agent` running — sub-agent dies first, then parent; both `failed`.
2. Send a long-running prompt; from a second terminal `taskkill /F /IM default-agent.exe`. TUI should flip the Agent node to `failed`, add `Error("agent disconnected unexpectedly")`, status bar `PAUSED`. Type a new prompt + Enter — User node + `+1 queued`, no Agent spawn. Press `Esc` → queue clears, status flips Running. Send another prompt — dispatches normally.
3. Press `Ctrl-N` (or call `DestroySession`) while a turn is running → cascade kills, `SessionDestroyed` diff fires, fresh empty session appears.
