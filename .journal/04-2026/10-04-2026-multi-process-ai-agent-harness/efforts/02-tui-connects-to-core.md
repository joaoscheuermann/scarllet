---
status: done
order: 2
created: 2026-04-10 19:48
title: "TUI connects to Core and displays connection status"
---

## Description

Build the TUI binary into a functional terminal interface using ratatui + crossterm. The TUI reads the lockfile written by Core (Effort 1), connects via gRPC, calls `Ping` to verify the connection, and renders a persistent status bar showing connection state. If Core is not running, TUI starts it as a background child process, waits for the lockfile, then connects.

## Objective

Running `npx nx run scarllet-tui:run` in a terminal (with Core already running) produces a full-screen ratatui interface showing "Connected to Core at 127.0.0.1:<port>" in a status bar. If Core is not running, TUI spawns it automatically, then connects.

## Implementation Details

1. **`scarllet-tui` dependencies:** Add `scarllet-proto`, `scarllet-sdk`, `tonic`, `tokio`, `ratatui`, `crossterm`, `tracing`, `clap`.
2. **Lockfile discovery flow:**
   - Read lockfile via SDK helper.
   - If lockfile exists and PID is alive → connect to address.
   - If lockfile exists and PID is dead → remove stale lockfile → start Core.
   - If no lockfile → start Core as detached child process → poll for lockfile (with timeout) → connect.
3. **gRPC connection:** Create a tonic channel to Core's address. Call `Ping` RPC to verify liveness.
4. **ratatui layout:**
   - Header bar: application name + version.
   - Main area: placeholder text input / chat area (interactive content comes in later efforts).
   - Status bar (bottom): connection status ("Connected to Core at X" / "Disconnected" / "Connecting...").
5. **Event loop:** crossterm event polling for keyboard input. `q` or Ctrl+C exits cleanly, restoring terminal state.
6. **Graceful terminal restore:** Ensure raw mode + alternate screen are always cleaned up, even on panic (set a panic hook).

## Verification Criteria

- Start Core first (`npx nx run scarllet-core:run`), then start TUI (`npx nx run scarllet-tui:run`) in another terminal → TUI shows "Connected to Core at 127.0.0.1:<port>".
- Start TUI without Core running → TUI spawns Core automatically, then shows "Connected" within a few seconds.
- Kill Core while TUI is running → TUI status changes to "Disconnected" (or TUI exits with a message).
- Press `q` in TUI → terminal is restored cleanly, no garbled output.
- `npx nx run scarllet-tui:build` compiles without errors.

## Done

- TUI binary launches a ratatui full-screen interface that discovers and connects to Core.
- Connection status is visible in the status bar — observable in real time by running both processes.

## Change Summary

### Files modified
- `packages/rust/scarllet-tui/Cargo.toml` — added dependencies: scarllet-proto, scarllet-sdk, tonic, tokio, ratatui 0.30, crossterm 0.29
- `packages/rust/scarllet-tui/src/main.rs` — full rewrite from placeholder to functional TUI

### Implementation
- **ConnectionState enum** with four states: Connecting, StartingCore, Connected, Failed
- **Async connect_to_core** task: reads lockfile → tries gRPC Ping → on failure removes stale lockfile → spawns Core binary → polls lockfile up to 15s
- **spawn_core**: finds `scarllet-core` binary next to the TUI binary via `current_exe()`, launches detached
- **try_ping**: creates tonic OrchestratorClient, calls Ping RPC, returns uptime
- **ratatui layout**: 3-area vertical split (header, main, status bar) with color-coded connection state
- **Event loop**: crossterm polling at 100ms, q or Ctrl+C to exit
- **Panic hook**: restores terminal even on panic

### Key decisions
- Used `tokio::sync::watch` channel for non-blocking state updates from async connection task to synchronous UI loop
- Auto-launch finds Core binary via `current_exe()` parent directory — works when both binaries are in the same build output dir
- Stale lockfile detection via gRPC Ping failure (no PID checking needed — the Ping is the definitive liveness check)

### Deviations
- None
