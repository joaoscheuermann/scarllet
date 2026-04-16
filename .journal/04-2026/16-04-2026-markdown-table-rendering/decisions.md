# Decision Log: markdown-table-rendering

### 2026-04-16 11:45 - Architect

**Context**: Need to detect GFM table regions within markdown text to render them separately from non-table content.
**Decision**: Use `pulldown-cmark` 0.13 as a direct dependency with `ENABLE_TABLES` and `into_offset_iter()` for table region detection.
**Rationale**: Proper GFM parser handles edge cases (escaped pipes, inline code with pipes, alignment syntax). Version 0.13.3 is already resolved in Cargo.lock via tui-markdown — zero additional binary cost.
**Alternatives considered**: Regex-based `|...|` line detection — fragile, must reimplement parsing, misidentifies code blocks containing pipes, no alignment info.

### 2026-04-16 11:45 - Architect

**Context**: Need to render detected table regions as terminal output that fits the existing `Vec<Line>` → `Paragraph::new()` pipeline.
**Decision**: Render tables as `Vec<Line>` using Unicode box-drawing characters (`┌─┬┐│├┼┤└┴┘`) with computed column widths and alignment.
**Rationale**: Drops into the existing rendering pipeline unchanged — no architecture changes to chat_message.rs or the scroll view. Box-drawing characters are widely supported in modern terminals.
**Alternatives considered**: ratatui `Table` widget — built-in column layout but requires fundamentally different rendering pipeline; cannot be embedded inline within a `Paragraph`.

### 2026-04-16 11:50 - Decomposer

**Context**: Breaking the approved architecture into incremental deliverables for implementation.
**Decision**: Two efforts — (1) table rendering module with unit tests, (2) wire into chat_message and verify end-to-end.
**Rationale**: Effort 1 creates the core module as a testable, self-contained unit. Effort 2 is the thin integration layer with end-to-end verification. Each effort is independently verifiable. Kept to two efforts because the total scope is ~150 new lines + 3 changed lines — further splitting would create artificial boundaries without independent value.
**Alternatives considered**: Single monolithic effort — viable given the small scope, but separating the module from the wiring gives cleaner test isolation and a natural review boundary.

### 2026-04-16 12:00 - Executor

**Context**: Effort 1 (Table rendering module with unit tests) completed.
**Decision**: Marked done — verification met and change summary appended.
**Rationale**: All 7 unit tests pass, cargo check and clippy produce no new warnings from the module. Pre-existing compilation errors in main.rs/app.rs were fixed as part of implementation to unblock the test binary.

### 2026-04-16 12:05 - Executor

**Context**: Effort 2 (Wire into chat_message and verify end-to-end) completed.
**Decision**: Marked done — all 3 call sites replaced, all 10 tests pass, no new warnings.
**Rationale**: Wiring is straightforward — `render_markdown()` returns `Text<'static>` which is compatible with the existing `Vec<Line<'a>>` pipeline since `'static` outlives any `'a`.
