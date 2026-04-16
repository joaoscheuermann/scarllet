---
title: Hook save triggers in events.rs
phase: 3
status: done
depends_on: 02-integrate-into-app
---

## Objective

Modify `packages/rust/scarllet-tui/src/events.rs` to call `app.save_session()` at the correct moments.

## Triggers to Add

| # | Trigger | Location in `events.rs` | Call |
|---|---------|------------------------|------|
| 1 | User submits prompt | In `handle_input()` after `app.push_message(ChatEntry::User { .. })` | `app.save_session()` |
| 2 | Agent response done | In `handle_core_event()` after `AgentResponse` case | `app.save_session()` |
| 3 | CTRL+N pressed | In `handle_input()` key handling (add CTRL+N case) | `app.save_session()` then `app.new_session()` |

## Files

### Modified: `packages/rust/scarllet-tui/src/events.rs`

**Trigger 1 — After user prompt:**
```rust
// In handle_input(), after:
app.push_message(ChatEntry::User {
    text: trimmed.clone(),
});
app.input_state.set_text(String::new());
app.save_session();  // ← ADD THIS
```

**Trigger 2 — After agent response:**
```rust
// In handle_core_event(), in the AgentResponse arm:
core_event::Payload::AgentResponse(e) => {
    if let Some(entry) = find_agent_entry(&mut app.messages, &e.task_id) {
        if let ChatEntry::Agent {
            blocks,
            visible_chars,
            done,
            ..
        } = entry
        {
            *blocks = proto_blocks_to_display(&e.blocks);
            *visible_chars = total_block_chars(blocks);
            *done = true;
        }
    }
    app.save_session();  // ← ADD THIS
    app.input_locked = false;
    app.focus = Focus::Input;
    app.focused_message_idx = None;
}
```

**Trigger 3 — CTRL+N shortcut:**
```rust
// In handle_input(), add to the key handling section.
// Insert after the existing CTRL+C exit check:
if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) {
    app.save_session();
    app.new_session();
    return false;
}
```

## Note on Error Handling

`app.save_session()` already handles errors silently via `tracing::warn!`. No additional error handling needed here.

## Verification

```powershell
cd packages/rust/scarllet-tui
cargo check
cargo test --lib
```
