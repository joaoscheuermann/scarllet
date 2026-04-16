---
status: done
created: 2026-04-16 11:39
slug: markdown-table-rendering
---

## Prompt

Sometimes the LLM output tables, our Markdown parser doesnt handle it really well, search the web for ways we can handle it correctly to render it for the end user!

Implement option 2: Use `pulldown-cmark` directly for tables. Since `tui-markdown` already pulls in `pulldown-cmark` transitively, add it as a direct dependency (version-matched). Before calling `tui_markdown::from_str`, split the markdown into table and non-table segments. For table segments, parse with `pulldown-cmark` (with `ENABLE_TABLES`) and emit `Vec<Line>` using box-drawing characters with proper column-width alignment.

## Research

### Root cause

`tui-markdown` v0.3 does **not** support GFM tables. Its `from_str_with_options` function does not enable `ParseOptions::ENABLE_TABLES`, and all table-related pulldown-cmark tags (`Tag::Table`, `Tag::TableHead`, `Tag::TableRow`, `Tag::TableCell`) are handled with `warn!("... not yet supported")` — the content is silently dropped.

### Options evaluated

| Option | Approach | Verdict |
|--------|----------|---------|
| 1 | Pre-process pipe tables into ASCII art before `tui_markdown::from_str` | Loses inline styling; fragile regex parsing |
| 2 | Use `pulldown-cmark` directly for table segments, `tui-markdown` for the rest | **Selected** — best quality, no fork, reuses transitive dep |
| 3 | Fork/patch `tui-markdown` | Maintenance burden of a fork |
| 4 | Use `termimad` for table segments | Two parsers, ANSI conversion overhead |

## Architecture

### Overview

Add a new module `widgets/markdown.rs` in `scarllet-tui` that wraps `tui_markdown::from_str` with a table-aware pre-processing step. All three existing call sites switch to the new function — no other rendering code changes.

### Public API

```rust
/// Renders markdown text to ratatui Text, with GFM table support.
pub fn render_markdown(input: &str) -> Text<'static>
```

### Internal Pipeline (3 stages)

**Stage 1 — Segmentation**: Parse the full input with `pulldown_cmark::Parser::new_ext(input, opts)` (with `ENABLE_TABLES`) using `into_offset_iter()`. Walk events, tracking `Event::Start(Tag::Table(_))` / `Event::End(TagEnd::Table)` to collect table byte ranges. Everything outside those ranges is a text segment.

**Stage 2a — Text segments**: Pass `&input[range]` to `tui_markdown::from_str()`. Convert to owned `Text<'static>` via `.into_owned()`.

**Stage 2b — Table segments**: Re-parse the table slice with pulldown-cmark. Collect into:

```rust
struct TableData {
    alignments: Vec<Alignment>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}
```

Render with Unicode box-drawing characters:

```
┌──────────┬────────┬──────────┐
│ Name     │ Status │ Duration │
├──────────┼────────┼──────────┤
│ build    │ pass   │ 12s      │
│ test     │ fail   │ 45s      │
└──────────┴────────┴──────────┘
```

Column widths: `max(header_cell_width, max(body_cell_widths)) + 2` (1 space padding each side).

Alignment within cells:
- Left (default): pad right
- Right: pad left
- Center: pad both sides

Styling:
- Box-drawing characters: `Color::DarkGray`
- Header cells: `Modifier::BOLD`
- Body cells: default style

**Stage 3 — Concatenation**: Collect all `Text` outputs into a single `Text<'static>` with blank line between segments.

### Impacted Files

| File | Change |
|------|--------|
| `packages/rust/scarllet-tui/Cargo.toml` | Add `pulldown-cmark = "0.13"` |
| `packages/rust/scarllet-tui/src/widgets/markdown.rs` | **New** — `render_markdown()`, segmentation, table rendering (~150 lines) |
| `packages/rust/scarllet-tui/src/widgets/mod.rs` | Add `pub mod markdown;` |
| `packages/rust/scarllet-tui/src/widgets/chat_message.rs` | Replace 3x `tui_markdown::from_str(x)` → `markdown::render_markdown(x)` |

### Design Decisions

1. **Table detection via pulldown-cmark** (not regex) — proper GFM parser, handles edge cases, version 0.13.3 already in Cargo.lock.
2. **Box-drawing characters as Vec<Line>** (not ratatui Table widget) — integrates with existing Paragraph pipeline without rendering architecture changes.

### Known Limitations (V1)

- Wide tables wrap via Paragraph's Wrap — same behavior as long code lines.
- Inline formatting inside table cells (bold, code) is flattened to plain text.

### Principles Applied

- **SRP**: Table rendering isolated in `markdown.rs`; `chat_message.rs` stays focused on layout.
- **OCP**: New module extends pipeline without modifying `tui-markdown` or `chat_message.rs` internals.
- **KISS**: Thin wrapper + segmentation, no custom markdown parser, no intermediate AST.
- **DRY**: Reuses `pulldown-cmark` (already transitive) and `tui-markdown` (for non-table content).
