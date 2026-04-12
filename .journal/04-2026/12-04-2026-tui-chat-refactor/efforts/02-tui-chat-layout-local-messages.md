---
status: done
order: 2
created: 2026-04-12 14:16
title: "TUI two-section chat layout with local message entry"
---

## Description

Build the full chat UI on top of the empty chat screen from Effort 1. Implement the two-section layout (history ~95%, input ~5%), the `ChatEntry` state model, Tab-based focus switching with visual cues, arrow-key scrolling in history, user message entry with `You` label, auto-scroll, and "exit" quit command. Messages are local only — no Core dispatch yet.

## Objective

User can type messages and see them appear in chat history labeled `You`. Tab switches focus between history and input (with highlighted borders). Arrow keys scroll history when focused. Auto-scroll to the latest message on new entries. Typing "exit" and pressing Enter quits the TUI. Input area shows a cursor and prompt indicator.

## Implementation Details

### TUI state model additions (`packages/rust/scarllet-tui/src/main.rs`)

Add to the existing `App` struct from Effort 1:

```rust
enum Focus { Input, History }

enum ChatEntry {
    User { text: String },
    Agent { name: String, task_id: String, content: String, done: bool },
    System { text: String },
}
```

Fields to add to `App`:
- `messages: Vec<ChatEntry>` — the chat history.
- `focus: Focus` — which section is focused (default: `Input`).
- `scroll_offset: usize` — how many lines scrolled up from the bottom in history.
- `input_locked: bool` — whether input accepts keystrokes (default: `false`, wired but not triggered in this effort).

### Layout (`draw` function)

Replace the empty chat screen from Effort 1 with:

```rust
let [history_area, input_area] = Layout::vertical([
    Constraint::Min(0),       // history ~95%
    Constraint::Length(3),    // input ~5%
]).areas(frame.area());
```

### History rendering

- Iterate `app.messages` and render each `ChatEntry`:
  - `ChatEntry::User { text }` → Line styled `"You: {text}"` in cyan.
  - `ChatEntry::Agent { name, task_id, content, done: false }` → Line styled `"{name} ({task_id_short}): {content} ⟳"` in green.
  - `ChatEntry::Agent { name, task_id, content, done: true }` → Line styled `"{name} ({task_id_short}): {content}"` in green.
  - `ChatEntry::System { text }` → Line styled `"System: {text}"` in dark gray.
  - `task_id_short` = first 8 chars of `task_id`.
- Insert a blank line between entries for readability.
- Wrap text: `Wrap { trim: false }`.
- If `messages` is empty, show a welcome message: `"Welcome to Scarllet. Type a message to begin."` centered, in dark gray.
- Apply `scroll_offset` to the `Paragraph` widget using `.scroll((offset, 0))`.

### Input rendering

- Block with `Borders::ALL` and title `" Input "`.
- When `input_locked`: render `"  Waiting for agent..."` in dark gray, no cursor.
- When unlocked: render `"  > {input_text}"` with a blinking cursor (use `SetCursorPosition` via crossterm).
- Border color:
  - `Focus::Input` → cyan border.
  - `Focus::History` → dark gray (dim) border.

### History area border

- Block with `Borders::ALL` and title `" Chat "`.
- Border color:
  - `Focus::History` → cyan border.
  - `Focus::Input` → dark gray (dim) border.

### Input handling

- `Tab` → toggle `app.focus` between `Input` and `History`.
- `Char(c)` when `focus == Input && !input_locked` → `app.input.push(c)`.
- `Backspace` when `focus == Input && !input_locked` → `app.input.pop()`.
- `Enter` when `focus == Input && !input_locked`:
  - If `input.trim() == "exit"` → quit.
  - If `input.trim()` is non-empty → push `ChatEntry::User { text: input.clone() }`, clear input, reset `scroll_offset` to 0 (auto-scroll).
  - If empty → do nothing.
- `Up` when `focus == History` → increment `scroll_offset` (clamped to max scrollable lines).
- `Down` when `focus == History` → decrement `scroll_offset` (clamped to 0).
- `Ctrl+C` → quit from any state.

### Auto-scroll

When a new `ChatEntry` is pushed, set `scroll_offset = 0` so the latest message is visible. If the user has manually scrolled up (offset > 0), don't auto-scroll — only auto-scroll when the user was already at the bottom.

## Verification Criteria

1. `npx nx run scarllet-tui:build` — builds successfully.
2. Start Core, then TUI → connects → chat screen appears with two sections.
3. Type "hello" + Enter → "You: hello" appears in history area.
4. Type multiple messages → all visible, auto-scrolls to latest.
5. Press Tab → focus moves to history (border color changes to cyan), input border dims.
6. Press Up/Down in history → scrolls through messages.
7. Press Tab again → focus returns to input.
8. Type "exit" + Enter → TUI exits cleanly.
9. Press Ctrl+C → TUI exits cleanly.
10. When `messages` is empty → welcome message is displayed.

## Done

- Two-section layout renders correctly (history ~95%, input ~5%).
- User messages appear labeled "You" in the history.
- Tab-based focus switching with visual border cues works.
- Arrow-key scrolling in history works.
- Auto-scroll to latest message on new entry.
- "exit" and Ctrl+C quit cleanly.

## Change Summary

**Files modified:**
- `packages/rust/scarllet-tui/src/main.rs` — Added `Focus` enum (Input/History), `ChatEntry` enum (User/Agent/System), full App state model with messages, input, input_locked, focus, scroll_offset. Implemented two-section layout (Min(0) + Length(3)). History rendering with color-coded labels (You=cyan, Agent=green, System=gray), welcome message when empty, scroll support. Input rendering with cursor, disabled state, focus border highlighting. Tab focus switching, arrow key scrolling, Enter for message entry, "exit" to quit. Auto-scroll on new messages.

**Decisions:** Used `u16` for scroll_offset (matches ratatui's `Paragraph::scroll` API). Used `#[allow(dead_code)]` on Agent/System variants and message_tx since Effort 3 will construct them. Cursor position computed from input length + fixed prefix offset.

**Deviations:** None — followed Implementation Details as specified.
