---
status: done
order: 1
created: 2026-04-12 14:15
title: "Proto streaming contract + Core AttachTui stub + TUI connecting screen"
---

## Description

Establish the end-to-end connection lifecycle: add the bidirectional streaming proto contract, implement a minimal Core handler that accepts the stream and sends a `ConnectedEvent`, add PID liveness checking to the SDK, and rewrite the TUI connection flow to show an animated connecting screen that transitions to an empty chat screen upon receiving `ConnectedEvent`.

This effort lays the communication foundation. All old TUI UI elements (task tree, output panel, autocomplete, slash commands, header, status bar) are removed. The TUI becomes a blank slate with only two states: connecting and chat (empty).

## Objective

TUI launches, displays an animated "Connecting to agent core..." screen, connects to Core via `AttachTui` bidirectional gRPC stream, and transitions to an empty chat screen when `ConnectedEvent` is received. Stale lockfiles (dead PID) are detected and cleaned up before attempting to connect or launch Core.

## Implementation Details

### Proto (`packages/rust/scarllet-proto/proto/orchestrator.proto`)

Add to the `Orchestrator` service (keep all existing RPCs unchanged):

```protobuf
rpc AttachTui(stream TuiMessage) returns (stream CoreEvent);
```

Add new message types:

- `TuiMessage` with `oneof payload { PromptMessage prompt = 1; }`
- `PromptMessage` with `string text = 1; string working_directory = 2;`
- `CoreEvent` with `oneof payload { ConnectedEvent connected = 1; AgentStartedEvent agent_started = 2; AgentThinkingEvent agent_thinking = 3; AgentResponseEvent agent_response = 4; AgentErrorEvent agent_error = 5; SystemEvent system = 6; }`
- `ConnectedEvent` with `uint64 uptime_secs = 1;`
- `AgentStartedEvent` with `string task_id = 1; string agent_name = 2;`
- `AgentThinkingEvent` with `string task_id = 1; string agent_name = 2; string content = 3;`
- `AgentResponseEvent` with `string task_id = 1; string agent_name = 2; string content = 3;`
- `AgentErrorEvent` with `string task_id = 1; string agent_name = 2; string error = 3;`
- `SystemEvent` with `string message = 1;`

### SDK (`packages/rust/scarllet-sdk/src/lockfile.rs`)

Add `pub fn is_pid_alive(pid: u32) -> bool`:
- Windows: use `windows-sys` or `std::process::Command` with `tasklist` to check PID existence.
- Unix: `libc::kill(pid as i32, 0) == 0`.

### Core (`packages/rust/scarllet-core/src/main.rs`)

Add a minimal `attach_tui` implementation to the `Orchestrator` trait:
1. Accept the incoming stream.
2. Create an `mpsc::channel` for outgoing events.
3. Send a `ConnectedEvent` immediately with `uptime_secs`.
4. Spawn a task to read from the incoming stream (for now, just drain/ignore messages).
5. Return the receiver side as the response stream.
6. On stream close: log the disconnection.

### TUI (`packages/rust/scarllet-tui/src/main.rs`)

Full rewrite of the connection flow and UI:

1. **State**: `enum Screen { Connecting { dots: usize, tick: u64 }, Chat }` — start in `Connecting`.
2. **Connection flow**:
   - Read lockfile → if exists, check `is_pid_alive(pid)` and port reachability.
   - If stale (dead PID or unreachable port): remove lockfile.
   - If no lockfile: spawn Core binary.
   - Poll for lockfile, then open `AttachTui` bidirectional stream.
   - On `ConnectedEvent`: transition to `Screen::Chat`.
3. **Connecting screen**: Centered `Paragraph` with text `"Connecting to agent core..."` and cycling dots (update every ~500ms tick).
4. **Chat screen**: Empty for now — just a `Paragraph` saying `"Connected. Type a message below."` filling the area, with a placeholder input bar at the bottom.
5. **Quit**: Ctrl+C from any screen exits cleanly.
6. **Remove**: All old draw logic (header, tree, output, autocomplete, status bar), all old input handling (slash commands, task selection), all old state fields (`commands`, `tasks`, `selected_task`, `showing_autocomplete`).

## Verification Criteria

1. `npx nx run-many -t build -p scarllet-proto,scarllet-sdk,scarllet-core,scarllet-tui` — all crates build successfully with new proto codegen.
2. `npx nx run scarllet-core:run` — Core starts and logs its address.
3. `npx nx run scarllet-tui:run` — TUI shows "Connecting to agent core..." with animated dots.
4. With Core already running: TUI connects, animated screen transitions to the empty chat screen.
5. Kill Core process, leave stale lockfile, start TUI → TUI detects stale PID, removes lockfile, spawns new Core, connects.
6. Ctrl+C exits TUI cleanly from both connecting and chat screens.

## Done

- TUI launches and displays animated "Connecting to agent core..." screen.
- TUI connects to Core via `AttachTui` stream and transitions to an empty chat screen.
- Stale lockfiles are cleaned up automatically.
- All old UI elements are removed.

## Change Summary

**Files modified:**
- `packages/rust/scarllet-proto/proto/orchestrator.proto` — Added `AttachTui` RPC + 8 new message types (TuiMessage, PromptMessage, CoreEvent, ConnectedEvent, AgentStartedEvent, AgentThinkingEvent, AgentResponseEvent, AgentErrorEvent, SystemEvent).
- `packages/rust/scarllet-sdk/src/lockfile.rs` — Added `is_pid_alive(pid)` function with Windows (tasklist) and Unix (libc::kill) implementations.
- `packages/rust/scarllet-sdk/Cargo.toml` — Added `libc` as a Unix-only dependency.
- `packages/rust/scarllet-core/src/main.rs` — Added minimal `attach_tui` implementation: creates mpsc channel, sends ConnectedEvent, spawns reader task to drain incoming messages, returns ReceiverStream.
- `packages/rust/scarllet-tui/src/main.rs` — Full rewrite: removed all old UI (tree, output, autocomplete, slash commands, header, status bar). New two-screen model (Connecting/Chat). Background connection task finds Core via lockfile+PID check, opens AttachTui stream, forwards events to main loop. Animated connecting screen with cycling dots.
- `packages/rust/scarllet-tui/Cargo.toml` — Added `tokio-stream` dependency.

**Decisions:** Used `std::process::Command` (tasklist on Windows, kill -0 on Unix via libc) for PID check instead of adding `windows-sys` — simpler, no new Windows deps. Removed `input` field from App struct (deferred to Effort 2).

**Deviations:** Screen::Connecting uses `tick: u64` only (not `dots: usize` separately) — dots computed from tick, simpler state.
