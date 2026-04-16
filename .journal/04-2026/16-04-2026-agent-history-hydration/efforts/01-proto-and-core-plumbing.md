---
status: done
order: 1
created: 2026-04-16 12:35
title: "Proto changes and Core plumbing"
---

## Description

Add the new proto messages (`HistoryEntry`, `HistorySync`, `AgentHistorySync`, `AgentInstruction`), modify `TuiMessage` and the `AgentStream` RPC return type. Update all Core code that touches the agent channel type (`agents.rs`, `routing.rs`, `service.rs`) so the full stack compiles. Add `conversation_history` storage to the service and send hydration on agent register. Handle `HistorySync` from TUI. Core also appends to stored history on prompt routing and agent result.

## Objective

The proto schema is updated, the Core compiles with the new types, and agent hydration is sent on registration. Running `cargo check` across the workspace succeeds (proto, core, TUI, agent all compile). Core tests pass.

## Implementation Details

1. **`orchestrator.proto`** — Add:
   ```proto
   message HistoryEntry { string role = 1; string content = 2; }
   message HistorySync { repeated HistoryEntry messages = 1; }
   message AgentHistorySync { repeated HistoryEntry messages = 1; }
   message AgentInstruction {
     oneof payload {
       AgentTask task = 1;
       AgentHistorySync history_sync = 2;
     }
   }
   ```
   Modify `TuiMessage` oneof: add `HistorySync history_sync = 3`.
   Change `AgentStream` RPC: `returns (stream AgentInstruction)`.

2. **`scarllet-core/src/agents.rs`** — Change `AgentRegistry` channel type from `mpsc::Sender<Result<AgentTask, Status>>` to `mpsc::Sender<Result<AgentInstruction, Status>>`. Update `register`, `get`, type aliases. Update tests.

3. **`scarllet-core/src/service.rs`**:
   - Add `conversation_history: Arc<RwLock<Vec<HistoryEntry>>>` field to `OrchestratorService`.
   - Initialize it in `main.rs` service construction.
   - Change `AgentStreamStream` type to `ReceiverStream<Result<AgentInstruction, Status>>`.
   - In `attach_tui` spawned task: handle `tui_message::Payload::HistorySync` — replace stored history.
   - In `agent_stream` handler: after `AgentRegister`, read stored history and send `AgentInstruction::HistorySync` as first message on `task_tx`.
   - In `attach_tui` prompt handling and `agent_stream` result handling: append user/assistant entries to conversation_history.

4. **`scarllet-core/src/routing.rs`** — When sending to agent channel, wrap `AgentTask` in `AgentInstruction { payload: Some(agent_instruction::Payload::Task(task)) }`.

5. **`scarllet-core/src/main.rs`** — Add `conversation_history: Arc::new(RwLock::new(Vec::new()))` to service construction.

6. **`agents/default/src/main.rs`** — Change `task_stream.message()` to receive `AgentInstruction`. Match on payload:
   - `agent_instruction::Payload::HistorySync(sync)` → map entries to `ChatMessage` and set `history`.
   - `agent_instruction::Payload::Task(task)` → existing task processing logic (unchanged).

7. **`scarllet-tui/src/events.rs`** — Stub: no changes yet (TUI history sync wired in Effort 2). Proto compiles without TUI sending history.

## Verification Criteria

- `cargo check` passes for all workspace members (proto, core, TUI, agent).
- `cargo test -p scarllet-core` — all existing + updated agent registry tests pass.
- `cargo clippy -p scarllet-core` — no new warnings.
- Run Core + agent manually: agent connects, registers, receives an empty `AgentHistorySync` (no history stored yet), then receives tasks normally. Existing chat functionality works unchanged.

## Done

- Proto schema updated with `AgentInstruction` wrapper and `HistorySync` messages.
- All 4 crates compile (`proto`, `core`, `tui`, `agent`).
- Agent receives `AgentInstruction::HistorySync` on registration and `AgentInstruction::Task` for prompts.
- Core accumulates history from prompts and responses.
