---
status: closed
created: 2026-04-17 20:01
title: "TUI stays on `Working (press ESC to stop)...` forever; no state updates from core rendered"
---

## Symptom

After the 8-effort refactor, launching the TUI and sending a prompt lands the conversation in the `Agent (running)` state with the yellow banner `Working (press ESC to stop)…` and it **never advances**. No Thought tokens stream in, no Tool nodes appear, no Result lands. The user cannot see any state updates arriving from core.

## Expected behaviour

1. User types prompt → `User` node rendered (cyan "You:" line).
2. Core creates `Agent` top-level node with `status="running"` → "agent_module (id): " header rendered.
3. Agent process connects, streams Thought tokens via `CreateNode(Thought)` + `UpdateNode(thought_content)` — Thought line grows under the Agent header within < 1 s.
4. On stream end, `CreateNode(Result)` + `TurnFinished` → Result line replaces/augments the Thought block; Agent `status` patched to `finished`; `AgentUnregistered`; status bar returns to `READY`.

## Actual behaviour

Step 1 + 2 may be happening (user says they see the "Working..." banner, which requires the Agent node rendered with `status="running"` and no visible children — so the Agent NodeCreated diff did arrive). But steps 3 + 4 never become visible:

- No Thought content appears.
- No Result node appears.
- Status bar stays `THINKING` / `Working…` indefinitely.

## Reproduction

Windows 10, PowerShell, `scarllet-core` + `scarllet-tui` + `default-agent` built from the current HEAD of the atomic refactor branch (journal slug `core-transport-refactor`). A valid LLM provider is expected to be configured.

1. `npx nx run scarllet-tui:run`
2. Type any prompt, press Enter.
3. Observe the banner sticks.

## Initial hypotheses (executor triage, pre-debugger)

Ranked most-to-least likely based on code inspection:

1. **Agent `CreateNode` broadcasts missing from the diff stream.** The `agents::stream` handler receives the `CreateNode` but a validation or broadcast path silently drops it before `subscribers.broadcast` fires. In particular, the Thought node's `parent_id` is the Agent `agent_id` (a UUID), not the Agent **node** id; if `NodeStore::create` / `node_owned_by_agent` validate against the *Agent node id* while the agent passes the *agent id from env*, every Thought creation is rejected silently.
2. **`AgentTask` never reaches the connected agent, so it idles.** After registering, the agent's `next_task` loop sits on `AgentInbound::Task`. If `agents::routing::handle_register` fails to send the `PendingDispatch` prompt, the agent never starts the LLM stream — the `Agent` node stays `running` with no children.
3. **Agent process crashes pre-stream or exits without a `TurnFinished`.** In that case effort 06's `apply_agent_termination` should flip the session to `Paused` and create an `Error` node. If the TUI's `StatusChanged` handling is broken, the user only sees the stuck "Working…" banner instead of the pause indicator + Error.
4. **Diffs are delivered but the TUI's `handle_session_diff` mutates the wrong field** (e.g. a `NodeCreated` with `parent_id=Some(agent_node)` is being treated as top-level or vice-versa, so the visible-children filter never sees the new node).
5. **The diff channel saturates / panics.** `diff_tx` is a `mpsc::channel(256)`; if the first-diff `Attached` state is huge the backpressure could kill the connection task silently.

## Affected area

- `packages/rust/scarllet-core/src/agents/stream.rs` — `handle_create_node`, validation, broadcast.
- `packages/rust/scarllet-core/src/agents/routing.rs` — `handle_register` + `try_dispatch_main` → `AgentInbound::Task` send.
- `packages/rust/scarllet-core/src/session/diff.rs` — broadcast plumbing.
- `packages/rust/scarllet-sdk/src/agent/mod.rs` — `send_outbound`, `create_thought`, `append_thought`, `emit_result`.
- `packages/rust/agents/default/src/main.rs` — turn loop, stream handling.
- `packages/rust/scarllet-tui/src/connection.rs` — diff forwarding.
- `packages/rust/scarllet-tui/src/events.rs::handle_session_diff` — NodeCreated / NodeUpdated application.
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs::build_lines` — "Working…" banner + `immediate_children` filter.

## Out of scope for this bug

The parallel "TUI regressions" concern (user explicitly flagged this alongside the bug) is tracked separately — see decisions log entry of today. That's about the TUI features I deleted/simplified during effort 01's TUI rewrite (markdown rendering, token budget status, provider name display, extra deps), not about the state-update wiring this bug targets. Don't try to address both in one fix.
