---
status: done
order: 6
created: 2026-04-16 10:00
title: "KISS: Replace polling with Notify for agent registration"
---

## Description

Replace the polling loop in `route_prompt` (20 iterations x 500ms) with a `tokio::sync::Notify`-based mechanism. When an agent registers via `AgentStream`, it triggers a notify. `route_prompt` awaits the notify with a timeout instead of polling. This is a behavior change — same 10s window but deterministic instead of polling.

## Objective

After this effort, `route_prompt` no longer polls the agent registry in a loop. Instead, it awaits a `Notify` signal (with 10s timeout) triggered by agent registration. The agent responds to prompts faster (no 500ms polling delay) and the code is simpler.

## Implementation Details

### Add Notify to AgentRegistry

In `packages/rust/scarllet-core/src/agents.rs`:

1. Add a `notify: Arc<tokio::sync::Notify>` field to `AgentRegistry`.
2. In `AgentRegistry::new()`, create the notify: `notify: Arc::new(tokio::sync::Notify::new())`.
3. In `AgentRegistry::register()`, after inserting the agent sender, call `self.notify.notify_waiters()`.
4. Add `pub fn notifier(&self) -> Arc<tokio::sync::Notify>` to expose a clone of the notify.

### Update route_prompt

In `packages/rust/scarllet-core/src/routing.rs` (after Effort 2 split):

1. Before spawning the agent process, get the notifier: `let notify = agent_registry.read().await.notifier()`.
2. Replace the polling `tokio::spawn` block (lines 630-675) with:
   ```rust
   tokio::spawn(async move {
       let timeout = tokio::time::timeout(
           std::time::Duration::from_secs(10),
           async {
               loop {
                   notify.notified().await;
                   let ar = agent_registry.read().await;
                   if let Some(sender) = ar.get(&a_name) {
                       let task = AgentTask { ... };
                       let _ = sender.try_send(Ok(task));
                       return;
                   }
               }
           }
       ).await;
       if timeout.is_err() {
           // Handle timeout — check if task already completed/failed
           // (same fallback logic as before)
       }
   });
   ```
3. Add a doc comment explaining:
   - Why Notify is used (deterministic, no polling overhead)
   - The 10s timeout matches the previous behavior
   - **Alternative B for future**: buffered task queue where agents pull tasks after registration, eliminating the need for the spawn-then-wait pattern entirely.

### Update OrchestratorService

Pass the `agent_registry` (which now contains the Notify) through the existing `Arc<RwLock<AgentRegistry>>` — no new fields needed on `OrchestratorService`.

## Verification Criteria

1. `npx nx run scarllet-core:build` succeeds.
2. `npx nx run scarllet-core:test` passes.
3. Start core + TUI. Send a prompt when no agent is running — verify the agent spawns, registers, and receives the task (previously this relied on polling; now it uses Notify).
4. Time the response: first token should arrive faster than before (no 500ms polling intervals).
5. Send a prompt when agent is already registered (warm path) — verify it still works immediately (this path was not changed).
6. Kill the agent process, send a prompt — verify the 10s timeout fires and the error message appears in the TUI.

## Done

- `AgentRegistry` has a `Notify` field triggered on registration.
- `route_prompt` awaits the Notify with a 10s timeout instead of polling.
- A doc comment describes the Notify approach and references Alternative B (buffered task queue) for future consideration.
- Agent registration and task delivery work correctly in both cold-start and warm-agent scenarios.
