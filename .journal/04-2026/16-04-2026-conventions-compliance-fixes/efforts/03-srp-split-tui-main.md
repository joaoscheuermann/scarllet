---
status: done
order: 3
created: 2026-04-16 09:57
title: "SRP: Split scarllet-tui/src/main.rs + extract scroll helper"
---

## Description

Split the 1,075-line `scarllet-tui/src/main.rs` into 5 focused modules. Also extract the DRY scroll helper (Fix 3) that appears 3 times in the input handler. No behavior changes — pure structural refactor.

## Objective

After this effort, `scarllet-tui/src/main.rs` is ~80 lines (main function, terminal setup, event loop). State, events, rendering, and networking each have their own module. The duplicated PageUp/PageDown scroll logic is a single helper function. The TUI compiles, starts, and works identically.

## Implementation Details

### app.rs

1. Create `packages/rust/scarllet-tui/src/app.rs`.
2. Move: `Focus`, `ToolCallStatus`, `ToolCallData`, `DisplayBlock`, `ChatEntry`, `Route`, `App` struct, `App::new`, `App::refresh_env`, `App::advance_tick`, `App::is_streaming`, `App::push_message`, `total_block_chars`, constants (`TYPEWRITER_CHARS_PER_TICK`, `ENV_REFRESH_INTERVAL`).
3. Make fields and types `pub(crate)` as needed by other modules.

### events.rs

1. Create `packages/rust/scarllet-tui/src/events.rs`.
2. Move: `handle_core_event`, `find_agent_entry`, `proto_blocks_to_display`, `handle_input`, `handle_paste`, `find_running_task_id`, `return_to_input`, `enter_history`, `insert_text_at_cursor`.

### render.rs

1. Create `packages/rust/scarllet-tui/src/render.rs`.
2. Move: `routes`, `draw_connecting`, `draw_chat`, `draw_history`, `draw_input`, `draw_status_bar`, `compute_input_height`, `format_git_segment`, `format_compact`, `format_token_budget`, `token_budget_style`, `INPUT_PREFIX_WIDTH`.

### connection.rs

1. Create `packages/rust/scarllet-tui/src/connection.rs`.
2. Move: `connect_and_stream`, `connect_to_core`, `find_core_address`, `spawn_core`.

### Scroll helper (DRY Fix 3)

Add to `app.rs` or `events.rs`:
```rust
pub(crate) fn scroll_page(state: &mut widgets::ScrollViewState, direction: i16, page_height: u16) {
    if direction < 0 {
        state.offset_y = state.offset_y.saturating_sub(page_height);
    } else {
        state.offset_y = state.offset_y.saturating_add(page_height);
    }
}
```
Replace all 3 occurrences in `handle_input` with calls to `scroll_page`.

### main.rs (slimmed)

Keep only: `mod` declarations, `main()` function with terminal init, event loop, and teardown.

## Verification Criteria

1. `npx nx run scarllet-tui:build` succeeds.
2. `npx nx run scarllet-tui:test` passes.
3. `npx nx run scarllet-tui:lint` passes with no new warnings.
4. Start TUI, connect to core, verify: typing works, PageUp/PageDown scrolls, sending prompts works, agent responses render, tool calls show, status bar displays provider + git info.

## Done

- `main.rs` is ~80 lines.
- `app.rs` contains all state types and the App struct.
- `events.rs` contains all event and input handlers.
- `render.rs` contains all draw functions.
- `connection.rs` contains gRPC connection and core spawning.
- Scroll logic is a single `scroll_page` helper called from 3 places.
- TUI starts, renders, scrolls, and handles prompts correctly.
