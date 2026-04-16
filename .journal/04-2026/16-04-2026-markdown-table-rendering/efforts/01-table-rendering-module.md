---
status: done
order: 1
created: 2026-04-16 11:50
title: "Table rendering module with unit tests"
---

## Description

Add `pulldown-cmark` as a direct dependency to `scarllet-tui` and create a new `widgets/markdown.rs` module that implements `render_markdown()`. This function segments markdown text into table and non-table regions using pulldown-cmark's `into_offset_iter()` with `ENABLE_TABLES`, renders table regions as box-drawn `Vec<Line>` with column alignment, and delegates non-table regions to `tui_markdown::from_str()`. Register the module in `widgets/mod.rs`. Include unit tests proving correctness.

## Objective

A self-contained `markdown::render_markdown()` function exists in `scarllet-tui` that correctly converts markdown containing GFM tables into styled ratatui `Text`, verified by passing unit tests.

## Implementation Details

1. **Cargo.toml** — Add `pulldown-cmark = "0.13"` to `[dependencies]` in `packages/rust/scarllet-tui/Cargo.toml`.

2. **`widgets/mod.rs`** — Add `pub mod markdown;` and re-export `markdown::render_markdown`.

3. **`widgets/markdown.rs`** — Implement:

   - **`render_markdown(input: &str) -> Text<'static>`** (public):
     - Parse `input` with `pulldown_cmark::Parser::new_ext(input, opts)` where `opts` includes `Options::ENABLE_TABLES`.
     - Use `into_offset_iter()` to walk events and collect byte ranges for `Tag::Table` regions.
     - Build a `Vec<Segment>` where each segment is either `Text(Range<usize>)` or `Table(Range<usize>)`.
     - For `Text` segments: call `tui_markdown::from_str(&input[range])`, convert to owned via `.to_string()` on spans.
     - For `Table` segments: call internal `render_table()`.
     - Concatenate all `Text` outputs (blank line between segments).

   - **`render_table(input: &str) -> Vec<Line<'static>>`** (internal):
     - Re-parse the table slice with pulldown-cmark (`ENABLE_TABLES`).
     - Collect into `TableData { alignments, header, rows }` by walking `Tag::Table`, `Tag::TableHead`, `Tag::TableRow`, `Tag::TableCell` events.
     - Compute column widths: `max(header_width, max(row_widths)) + 2` per column.
     - Render lines with box-drawing characters:
       - Top border: `┌` + `─`×width + (`┬` + `─`×width)… + `┐`
       - Header row: `│` + padded/aligned cell + (`│` + cell)… + `│` — header cells styled **bold**
       - Separator: `├` + `─`×width + (`┼` + `─`×width)… + `┤`
       - Body rows: `│` + padded/aligned cell + (`│` + cell)… + `│`
       - Bottom border: `└` + `─`×width + (`┴` + `─`×width)… + `┘`
     - Box-drawing characters styled `Color::DarkGray`.
     - Alignment per cell: Left → pad right, Right → pad left, Center → pad both.

   - **`align_cell(text: &str, width: usize, alignment: Alignment) -> String`** (internal):
     - Pads content within `width` according to the alignment.

4. **Unit tests** (in `#[cfg(test)] mod tests` within `markdown.rs`):
   - `table_only` — single GFM table renders correct box-drawn output.
   - `text_only_passthrough` — plain markdown passes through identically to `tui_markdown::from_str`.
   - `mixed_text_and_table` — text before and after a table both render, table is box-drawn.
   - `column_alignment` — left, right, center alignment applied correctly.
   - `single_column` — table with one column renders properly.
   - `empty_cells` — cells with no content get padded spacing.

## Verification Criteria

- `cargo check -p scarllet-tui` compiles without errors.
- `cargo clippy -p scarllet-tui` produces no warnings from new code.
- `cargo test -p scarllet-tui` — all unit tests in `markdown.rs` pass:
  - Table-only input produces lines with `┌`, `│`, `├`, `└` box characters.
  - Text-only input matches `tui_markdown::from_str` output.
  - Mixed input preserves both table and non-table content.
  - Alignment padding is correct for all three variants.
  - Edge cases (single column, empty cells) handled.

## Done

- `widgets/markdown.rs` exists with `render_markdown()` and full unit test suite.
- Running `cargo test -p scarllet-tui -- markdown` shows all tests passing, demonstrating correct table rendering output.

## Change Summary

### Files created
- `packages/rust/scarllet-tui/src/widgets/markdown.rs` — `render_markdown()` public function + segmentation, table parsing, box-drawing rendering, alignment, 7 unit tests.

### Files modified
- `packages/rust/scarllet-tui/Cargo.toml` — Added `pulldown-cmark = "0.13"`.
- `packages/rust/scarllet-tui/src/widgets/mod.rs` — Added `pub mod markdown;` and re-export.
- `packages/rust/scarllet-tui/src/main.rs` — Fixed pre-existing compilation error: wired `session_repo` argument into `App::new()`.
- `packages/rust/scarllet-tui/src/app.rs` — Fixed pre-existing borrow-after-move on `m.content` by computing `char_count` before the move.

### Decisions
- Used `Cow::into_owned()` on each span to convert `Text<'_>` from `tui_markdown` into `Text<'static>`, preserving style and alignment metadata.
- Also enabled `ENABLE_STRIKETHROUGH` in the segmentation parser to avoid misidentifying `~~` as table-related syntax.

### Deviations
- Fixed two pre-existing compilation errors in `main.rs` and `app.rs` (from the session-persistence feature) that blocked the test binary from building. These are minimal, correct fixes.
