---
status: done
created: 2026-04-16 12:00
slug: tree-tool
---

## Prompt

I want to create a new tool, this tool should be responsible for returning a `bash tree` like structure of all files in the current codebase.

## Research

(empty)

## Architecture

### Category

Tool / CLI binary — standalone executable following the scarllet tool contract (`--manifest` → JSON, stdin JSON → stdout plain text). Lives in `packages/rust/tools/tree` alongside `find`, `grep`, `edit`, `write`, `terminal`.

### Crate layout

```
packages/rust/tools/tree/
├── Cargo.toml        # tree-tool, edition 2021
├── project.json      # @monodon/rust:build + run targets
└── src/
    └── main.rs       # single-file binary
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `serde` + `serde_json` | Deserialize `TreeInput` from stdin |
| `ignore` | `gitignore::GitignoreBuilder` for `.gitignore` rule matching |
| `glob` | Match user-provided `exclude` patterns |

### Design decisions

- **`read_dir`-based traversal** (not `ignore::WalkBuilder`) — we need full control over ordering (directories first, then files, case-insensitive alphabetical). `WalkBuilder` walks in arbitrary order.
- **`ignore::gitignore::GitignoreBuilder`** — loads and applies `.gitignore` rules at each directory level, handling nested gitignore files.
- **Plain text output** — not JSON-wrapped. The core's `output_json` string field tolerates non-JSON strings.

### Input schema

```json
{
    "type": "object",
    "properties": {
        "path": {
            "type": "string",
            "description": "Root directory to display (default: current directory)"
        },
        "exclude": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Glob patterns to exclude from the tree"
        }
    }
}
```

### Core data flow

```
stdin (JSON) → deserialize TreeInput
            → validate path (exists, is dir, not excluded)
            → recursive walk:
                read_dir → filter (hidden, symlinks, gitignore, exclude)
                         → sort (dirs first, case-insensitive alpha)
                         → for each entry:
                             if dir → print with "/" suffix, recurse
                                      if empty after filtering → append "(empty)"
                             if file → print name
                         → use box-drawing prefix based on position (last vs not-last)
            → print full tree string to stdout
```

### Output format

Plain text with box-drawing connectors (`├──`, `└──`, `│`). Directories suffixed with `/`. Empty directories marked `(empty)`. Root node shows full path.

Example:
```
/home/user/my-project
├── src/
│   ├── components/
│   │   ├── Button.tsx
│   │   └── Modal.tsx
│   ├── utils/ (empty)
│   ├── App.tsx
│   └── index.ts
├── tests/
│   └── app.test.ts
├── package.json
└── tsconfig.json
```

### Error handling

All errors printed to stdout as text, exit 0 (so agents see error messages):
- Path doesn't exist → error message
- Path is a file → error message
- Exclude matches root → error message
- Permission denied on subdir → skip, continue
- Invalid stdin JSON → error message

### Registration

1. Add `"packages/rust/tools/tree"` to root `Cargo.toml` workspace members
2. `project.json` with `@monodon/rust:build` and `@monodon/rust:run` targets
3. Binary auto-discovered by `scarllet-core` watcher

### Impacted paths

| Path | Change |
|------|--------|
| `packages/rust/tools/tree/` | New crate |
| `Cargo.toml` (root) | Add workspace member |

### Verification

```powershell
npx nx build tree-tool
.\dist\target\tree-tool\debug\tree-tool.exe --manifest
echo '{}' | .\dist\target\tree-tool\debug\tree-tool.exe
echo '{"path": "packages/rust/tools"}' | .\dist\target\tree-tool\debug\tree-tool.exe
echo '{"path": ".", "exclude": ["*.toml"]}' | .\dist\target\tree-tool\debug\tree-tool.exe
echo '{"path": "nonexistent"}' | .\dist\target\tree-tool\debug\tree-tool.exe
echo '{"path": "Cargo.toml"}' | .\dist\target\tree-tool\debug\tree-tool.exe
```
