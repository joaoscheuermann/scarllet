---
status: done
order: 2
created: 2026-04-16 12:00
title: "Filtering and edge cases"
---

## Description

Add all filtering logic (gitignore, hidden files, symlinks, user exclude globs) and remaining edge cases (empty directories, permission denied, exclude-matches-root) on top of the working tree binary from Effort 1.

## Objective

The `tree-tool` binary fully satisfies the approved spec:
- Respects `.gitignore` (including nested gitignore files)
- Excludes hidden files/directories (names starting with `.`)
- Skips symbolic links
- Supports `exclude` parameter (array of glob patterns)
- Marks empty directories (or directories whose children were all filtered out) with `(empty)`
- Handles permission denied gracefully (skip and continue)
- Returns error when exclude glob matches the root directory

## Implementation Details

1. **Add dependencies to `Cargo.toml`:**
   - `ignore = "0.4"` — for `gitignore::GitignoreBuilder`
   - `glob = "0.3"` — for matching user `exclude` patterns

2. **Extend `TreeInput`:**
   - Add `exclude: Option<Vec<String>>` field

3. **Update `print_manifest()`:**
   - Add `exclude` to `input_schema.properties`

4. **Gitignore support:**
   - At each directory level, check for `.gitignore` file
   - Use `ignore::gitignore::GitignoreBuilder` to build matchers
   - Pass parent gitignore matcher into recursive calls so rules cascade
   - Filter out entries matched by gitignore rules

5. **Hidden file/directory filtering:**
   - Skip any entry whose name starts with `.`
   - Apply before sorting (so hidden dirs don't appear)

6. **Symlink filtering:**
   - Check `entry.file_type()` / `metadata.file_type().is_symlink()`
   - Use `std::fs::symlink_metadata` (not `metadata`, which follows symlinks)
   - Skip symlinks entirely

7. **User `exclude` globs:**
   - Compile each pattern in `exclude` into `glob::Pattern`
   - Match against both the entry name and relative path
   - If any pattern matches, skip the entry
   - Before tree walk: check if any exclude pattern matches the root directory name → return error

8. **Empty directory handling:**
   - After filtering a directory's children, if none remain, append ` (empty)` to the directory line
   - This includes the case where all children were excluded by filters (E-3)

9. **Permission denied:**
   - Catch `std::io::ErrorKind::PermissionDenied` from `read_dir`
   - Skip the directory silently and continue

10. **Update `build_tree` signature:**
    - Accept gitignore matcher, exclude patterns, and propagate through recursion

## Verification Criteria

- Create a test directory with a `.gitignore` that ignores `*.log` → `tree-tool` excludes `.log` files
- Create a directory with `.hidden-dir/` and `.hidden-file` → both excluded from output
- Create a symlink in a test directory → symlink does not appear in output
- `echo '{"path": ".", "exclude": ["*.toml"]}' | tree-tool.exe` → no `.toml` files in output
- Create an empty directory → appears in tree with `(empty)` suffix
- Create a directory where all children match an exclude glob → directory shows `(empty)`
- `echo '{"path": "src", "exclude": ["src"]}' | tree-tool.exe` → returns error
- Run against a directory with nested `.gitignore` files → inner rules override/extend outer rules
- Run against repo root → `node_modules`, `.git`, `target`, and other gitignored paths excluded

## Done

- All filtering (gitignore, hidden, symlinks, exclude) works correctly
- Empty directories display `(empty)` suffix
- Permission denied is handled gracefully (no crash)
- Exclude-matches-root returns a clear error
- Running against the full repo root produces a clean, filtered tree

## Change Summary

### Files modified
- `packages/rust/tools/tree/Cargo.toml` — added `ignore = "0.4"` and `glob = "0.3"` dependencies
- `packages/rust/tools/tree/src/main.rs` — full rewrite adding filtering, `exclude` parameter, empty dir detection

### Key changes
- Added `exclude: Option<Vec<String>>` to `TreeInput` and manifest `input_schema`
- Gitignore: `maybe_push_gitignore` loads `.gitignore` per directory, `is_gitignored` checks from most-specific to least-specific supporting negation patterns
- Hidden files: entries starting with `.` skipped in `filtered_sorted_entries`
- Symlinks: detected via `symlink_metadata` and skipped
- Exclude: `glob::Pattern` matching against both entry name and path
- Empty dirs: tracked via output length before/after recursion — if no children written, truncate and append `(empty)`
- Permission denied: `read_dir` errors silently skipped
- Exclude-matches-root: checked before traversal, returns error

### Deviations from Implementation Details
- None — all 10 implementation steps followed as specified
