---
status: done
order: 2
created: 2026-04-16 12:35
title: "TUI sends history to Core on connect"
---

## Description

Wire the TUI to send a `HistorySync` message to the Core right after receiving the `Connected` event. This maps `app.messages` (User + completed Agent entries) to `HistoryEntry` values and sends them via the existing `message_tx` channel.

## Objective

When the TUI connects to Core (fresh start or reconnect), it sends the persisted conversation history so the Core can hydrate agents. Observable by checking Core logs for "Received N history entries" after TUI connects.

## Implementation Details

1. **`scarllet-tui/src/events.rs`** — In `handle_core_event`, add a case (or extend the existing `Connected` handler) to send history:
   - When `core_event::Payload::Connected(_)` is received:
     - Build `Vec<HistoryEntry>` from `app.messages`: User entries → `role: "user"`, completed Agent entries → `role: "assistant"` (join text blocks).
     - Construct `TuiMessage { payload: Some(tui_message::Payload::HistorySync(HistorySync { messages })) }`.
     - Send via `app.message_tx.try_send(msg)`.

2. **`handle_core_event` signature** — Currently takes `&mut App` and `CoreEvent`. The `message_tx` is on `app`, so no signature change needed.

3. **Core logging** — Add an `info!` log in `service.rs` when `HistorySync` is received, showing the count of entries. (This was part of Effort 1 but verify it's present.)

## Verification Criteria

- `cargo check -p scarllet-tui` compiles.
- `cargo test -p scarllet-tui` — all existing tests pass.
- Run the full stack (Core + TUI): open TUI, send a message, close TUI. Reopen TUI — Core logs show "Received N history entries" confirming history was sent on reconnect.

## Done

- TUI sends `HistorySync` with conversation history immediately after receiving `Connected` event.
- Core receives and stores the history (visible in Core logs).
