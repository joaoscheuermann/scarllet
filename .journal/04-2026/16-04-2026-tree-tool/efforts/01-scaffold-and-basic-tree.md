---
status: done
order: 1
created: 2026-04-16 12:00
title: "Scaffold crate and basic tree output"
---

## Description

Create the `tree-tool` crate from scratch and implement the core tree-walking logic with formatted output. This is the foundational effort — after this, the binary is runnable and produces a correctly formatted directory tree with proper ordering and box-drawing connectors.

## Objective

A runnable `tree-tool` binary that:
- Responds to `--manifest` with a valid tool manifest JSON
- Accepts `{"path": "..."}` on stdin and prints a plain-text directory tree to stdout
- Sorts directories first, then files, both case-insensitive alphabetical
- Uses box-drawing connectors (`├──`, `└──`, `│`)
- Suffixes directory names with `/`
- Shows full path as root node
- Handles basic errors (path not found, path is file, bad JSON input)

## Implementation Details

1. **Scaffold files:**
   - `packages/rust/tools/tree/Cargo.toml` — name `tree-tool`, edition 2021, deps: `serde` (with derive), `serde_json`
   - `packages/rust/tools/tree/project.json` — `@monodon/rust:build` and `@monodon/rust:run` targets, same pattern as `find-tool`
   - `packages/rust/tools/tree/src/main.rs` — main binary
   - Add `"packages/rust/tools/tree"` to root `Cargo.toml` workspace members

2. **`main.rs` structure (following `find` tool pattern):**
   - `TreeInput` struct: `path: Option<String>`  (no `exclude` yet — Effort 2)
   - `print_manifest()` — JSON manifest to stdout on `--manifest`
   - `main()` — arg check → manifest or stdin parse → execute → print
   - `execute(input: TreeInput) -> String` — returns the full tree string
   - `build_tree(dir: &Path, prefix: &str, is_last: bool, output: &mut String)` — recursive tree builder

3. **Tree walk logic:**
   - `std::fs::read_dir` to list entries
   - Sort: partition into dirs and files, each sorted case-insensitive alphabetically, dirs first
   - For each entry, compute the correct prefix using `├──` (non-last) or `└──` (last)
   - Recurse into directories with updated prefix (`│   ` for non-last parent, `    ` for last parent)
   - Directories get `/` suffix

4. **Error handling:**
   - Path not found → return error string
   - Path is a file → return error string
   - Empty path string → treat as `"."`
   - Invalid stdin JSON → print error, exit 0

## Verification Criteria

- `npx nx build tree-tool` compiles successfully
- `tree-tool.exe --manifest` prints valid JSON with name `"tree"`, kind `"tool"`, and input_schema
- `echo '{}' | tree-tool.exe` prints tree of current directory with box-drawing connectors
- `echo '{"path": "packages/rust/tools"}' | tree-tool.exe` shows only the tools subdirectory tree
- Directories appear before files at each level, both groups sorted alphabetically
- All directory names end with `/`
- Root node shows the full resolved path
- `echo '{"path": "nonexistent"}' | tree-tool.exe` prints an error message (not a crash)
- `echo '{"path": "Cargo.toml"}' | tree-tool.exe` prints "path is not a directory" error

## Done

- Binary compiles and runs
- `--manifest` returns valid tool manifest
- Running against a known directory produces correct, sorted tree output with box-drawing connectors and `/` suffixed directories
- Error cases return readable messages instead of panics

## Change Summary

### Files created
- `packages/rust/tools/tree/Cargo.toml` — crate manifest (serde, serde_json deps)
- `packages/rust/tools/tree/project.json` — Nx project config with build/run targets
- `packages/rust/tools/tree/src/main.rs` — full tool implementation

### Files modified
- `Cargo.toml` (root) — added `packages/rust/tools/tree` to workspace members

### Decisions
- Used `trim_start_matches("//?/")` to strip Windows extended-length path prefix from `canonicalize()` output
- Kept `sorted_entries()` as a standalone function returning `Vec<(PathBuf, bool)>` for clean separation of sorting from tree building
- All errors print to stdout and exit 0, following existing tool convention
