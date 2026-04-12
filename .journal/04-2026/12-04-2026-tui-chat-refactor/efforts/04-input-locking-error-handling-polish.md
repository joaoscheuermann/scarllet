---
status: done
order: 4
created: 2026-04-12 14:18
title: "Input locking during agent processing + error handling + polish"
---

## Description

Complete the MVP by wiring input locking while an agent is processing, handling all error paths (Core unreachable at send time, agent failure, stream disconnection), and polishing the UX: thinking animation in the agent label, visual disabled state for the input bar, graceful reconnection message on stream drop.

Builds on Effort 3's functional pipeline to add the behavioral safeguards from the requirements spec.

## Objective

Input is disabled (visually greyed, keystrokes ignored) while an agent is thinking. Input re-enables when the agent completes or fails. Connection errors and agent errors render as system messages in chat. The thinking indicator animates. Stream disconnection shows a system message rather than crashing.

## Implementation Details

### TUI — Input locking (`packages/rust/scarllet-tui/src/main.rs`)

Wire the `input_locked` field (added in Effort 2 but unused):

1. When `AgentStartedEvent` is received → set `app.input_locked = true`.
2. When `AgentResponseEvent` is received (for the active task) → set `app.input_locked = false`, set `app.focus = Focus::Input`.
3. When `AgentErrorEvent` is received (for the active task) → set `app.input_locked = false`, set `app.focus = Focus::Input`.
4. In input handling: all character input, backspace, and Enter are guarded by `!app.input_locked`.

### TUI — Input bar disabled state

In the `draw` function, when `app.input_locked`:
- Render `"  Waiting for agent..."` in dark gray italic.
- Border color: dark gray regardless of focus.
- Do not render cursor position.

### TUI — Thinking animation

For `ChatEntry::Agent { done: false, .. }`:
- Append a cycling indicator to the rendered text: `⟳ thinking` with dots cycling (`·`, `··`, `···`) based on a tick counter.
- The tick counter increments on each draw cycle (every ~200ms poll).

### TUI — Error handling: Core unreachable at send time

When sending a `TuiMessage` via the stream sender:
- If the channel send fails (Core disconnected): push `ChatEntry::System { text: "Connection lost. Please restart the TUI." }`.
- Set `app.input_locked = false` so the user can still type "exit" to quit.

### TUI — Error handling: stream disconnection

In the main event loop, if `event_rx` is closed (Core dropped the stream):
- Push `ChatEntry::System { text: "Disconnected from Core." }`.
- Do not crash — let the user see the message and quit with Ctrl+C or "exit".

### TUI — Error handling: agent failure

Already handled in Effort 3 (AgentErrorEvent renders as system message + unlocks input). Ensure the error message includes enough context: `"Error from <agent_name> (<task_id_short>): <error>"`.

### TUI — Edge case: empty prompt while locked

Already handled — `Enter` is guarded by `!input_locked` (Effort 2 wiring). Verify this works when the lock is active.

### TUI — Terminal resize

Ratatui handles this natively via `crossterm`'s resize events. Verify the layout adapts correctly. No code changes expected — the constraint-based layout from Effort 2 is already responsive.

## Verification Criteria

1. `npx nx run scarllet-tui:build` — builds successfully.
2. Start Core with a registered agent → start TUI → type a prompt:
   - Input bar shows "Waiting for agent..." and is visually greyed.
   - Typing characters does nothing while locked.
   - Thinking indicator animates (`⟳ thinking·`, `⟳ thinking··`, `⟳ thinking···`).
   - When agent responds: input unlocks, cursor returns, focus moves to input.
3. Start Core with no agents → type a prompt → system message appears, input remains unlocked.
4. Start Core → start TUI → kill Core process → type a prompt → system message "Connection lost" appears, TUI does not crash.
5. Start Core → start TUI → register an agent → type a prompt → agent fails → error message appears, input unlocks.
6. Resize the terminal during a chat session → layout adapts without visual glitches.
7. Press Ctrl+C while input is locked → TUI exits cleanly.
8. Type "exit" when input is unlocked → TUI exits cleanly.

## Done

- Input is disabled while agent is processing — visually greyed, keystrokes blocked.
- Input re-enables when agent completes or fails.
- Thinking indicator animates in the chat history.
- Connection loss and agent errors render as system messages.
- TUI does not crash on stream disconnection.
- Terminal resize adapts the layout dynamically.

## Change Summary

**Files modified:**
- `packages/rust/scarllet-tui/src/main.rs` — Added `tick` and `stream_closed` fields to App. Input locks on AgentStarted event (set `input_locked = true`). Thinking animation uses cycling dots (·/··/···) via `thinking_dots(tick)` function. Send failure on try_send shows "Connection lost" system message. Stream disconnect detection: when event_rx channel closes, shows "Disconnected from Core" system message and unlocks input. Advanced tick counter renamed to `advance_tick()` and increments global tick for animation.

**Decisions:** Used global tick counter for both connecting screen and thinking animation — simpler than per-entry timers. Used `try_recv` with `TryRecvError::Disconnected` pattern for stream disconnect detection — clean separation from normal "no events" case.

**Deviations:** None — followed Implementation Details as specified.
