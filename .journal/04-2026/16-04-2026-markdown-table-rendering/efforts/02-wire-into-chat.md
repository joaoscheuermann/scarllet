---
status: done
order: 2
created: 2026-04-16 11:50
title: "Wire into chat_message and verify end-to-end"
---

## Description

Replace the three `tui_markdown::from_str()` call sites in `chat_message.rs` with `markdown::render_markdown()` from Effort 1, completing the integration. After this effort, GFM tables output by the LLM render as bordered, aligned tables in the TUI.

## Objective

LLM responses containing GFM tables display as box-drawn tables in the Scarllet TUI chat view, with all non-table markdown continuing to render identically to before.

## Implementation Details

1. **`widgets/chat_message.rs`** — Three replacements:

   - **Line 92** (user messages):
     Replace `tui_markdown::from_str(text)` with `super::markdown::render_markdown(text)`.

   - **Line 131** (agent thought blocks):
     Replace `tui_markdown::from_str(visible)` with `super::markdown::render_markdown(visible)`.

   - **Line 152** (agent text blocks):
     Replace `tui_markdown::from_str(visible)` with `super::markdown::render_markdown(visible)`.

2. All three call sites already consume the result as `.lines` (iterating `Vec<Line>`), so no type changes are needed — `render_markdown()` returns `Text<'static>` which has the same `.lines` field.

3. No other files change. The `tui-markdown` dependency can remain in `Cargo.toml` since `render_markdown()` delegates non-table content to it internally.

## Verification Criteria

- `cargo check -p scarllet-tui` compiles without errors.
- `cargo clippy -p scarllet-tui` produces no warnings.
- `cargo test -p scarllet-tui` — all existing tests still pass.
- **End-to-end smoke test**: Run the TUI (`cargo run -p scarllet-tui`), send a prompt to the LLM that elicits a table response (e.g. "List 3 programming languages with their year of creation and typing discipline in a table"), and observe:
  - The table renders with `┌─┬─┐`, `│`, `├─┼─┤`, `└─┴─┘` box-drawing characters.
  - Header row is visually bold.
  - Text before and after the table renders normally (headings, lists, bold, code).
  - Thought blocks containing tables render dimmed with border, same as before but now with table support.

## Done

- All 3 call sites in `chat_message.rs` use `markdown::render_markdown()`.
- Running the TUI and prompting for tabular output displays a properly bordered, aligned table in the chat view.

## Change Summary

### Files modified
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs` — Replaced 3x `tui_markdown::from_str()` → `super::markdown::render_markdown()` (lines 92, 131, 152).
- `packages/rust/scarllet-tui/src/widgets/mod.rs` — Removed unused `pub use markdown::render_markdown` re-export (not needed; `chat_message.rs` uses `super::markdown` path).

### Decisions
- Kept `tui-markdown` in `Cargo.toml` since `render_markdown()` delegates non-table content to it internally.
- Removed the `pub use` re-export since `scarllet-tui` is a binary crate and the direct `super::markdown` path suffices for intra-crate use.

### Deviations
- None.
