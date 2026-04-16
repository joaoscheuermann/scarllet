# Decision Log: tree-tool

### 2026-04-16 12:00 - Architect

**Context**: Designing traversal strategy for tree tool that needs directory-first, alphabetically-sorted output while respecting .gitignore rules.
**Decision**: Use `std::fs::read_dir` for traversal with manual sorting, and `ignore::gitignore::GitignoreBuilder` for gitignore filtering. Output plain text (not JSON).
**Rationale**: `ignore::WalkBuilder` walks in arbitrary order, making it impossible to produce tree-ordered output (dirs first, alpha within groups). Manual `read_dir` gives full control over ordering. Plain text output is simpler and matches the spec; the core's `output_json` field tolerates non-JSON strings.
**Alternatives considered**: (1) Use `WalkBuilder` and collect/sort all entries in memory — adds unnecessary complexity and memory for large trees. (2) JSON-wrapped output — rejected per spec requirement.

### 2026-04-16 12:00 - Decomposer

**Context**: Breaking the tree-tool architecture into incremental deliverables.
**Decision**: Two efforts — (1) Scaffold crate + basic tree output with sorting and formatting, (2) All filtering (gitignore, hidden, symlinks, exclude) and edge cases.
**Rationale**: The tool is simple enough that two vertical slices cover it cleanly. Effort 1 delivers a fully runnable binary with correct output formatting. Effort 2 layers all filtering on top. Splitting further would create artificial type-only efforts with nothing new to run. Both efforts are independently runnable and observable.
**Alternatives considered**: (1) Three efforts separating gitignore from exclude/edge-cases — rejected because filtering logic is tightly coupled (all filters apply in the same read_dir loop) and splitting would create an awkward partial state. (2) Single effort — rejected because scaffolding + formatting is independently valuable and testable.

### 2026-04-16 12:00 - Executor

**Context**: Effort 1 "Scaffold crate and basic tree output" completed.
**Decision**: Marked done — all verification criteria met.
**Rationale**: Binary builds, manifest is valid JSON, tree output uses correct box-drawing connectors with dirs-first sorting, directory `/` suffix, full path root label, and all error cases return readable messages.

### 2026-04-16 12:00 - Executor

**Context**: Effort 2 "Filtering and edge cases" completed.
**Decision**: Marked done — all verification criteria met.
**Rationale**: Gitignore filtering correctly excludes ignored paths on repo root (node_modules, dist, target all absent). Hidden files excluded. Exclude globs work for both file patterns (*.toml) and directory names. Empty directories show (empty) suffix including when children are filtered out. Exclude-matches-root returns error. All 10 implementation steps followed without deviation.
