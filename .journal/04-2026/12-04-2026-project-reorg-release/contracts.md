# Contracts — Project Reorganization + Release Build

## 1. Release Folder Layout Contract

The release target MUST produce this exact structure:

```
release/
├── core.exe                  ← renamed from scarllet-core.exe
├── tui.exe                   ← renamed from scarllet-tui.exe
├── agents/
│   └── default.exe           ← renamed from default-agent.exe
├── tools/
│   └── echo-tool.exe         ← keeps its name
└── commands/                 ← empty dir (ready for future commands)
```

### Naming convention

| Crate name | Binary produced by cargo | Release folder name |
|------------|------------------------|-------------------|
| `scarllet-core` | `scarllet-core.exe` | `release/core.exe` |
| `scarllet-tui` | `scarllet-tui.exe` | `release/tui.exe` |
| `default-agent` | `default-agent.exe` | `release/agents/default.exe` |
| `echo-tool` | `echo-tool.exe` | `release/tools/echo-tool.exe` |

Future crates follow the pattern:
- Agent crates → `release/agents/<name>.exe`
- Tool crates → `release/tools/<name>.exe`
- Command crates → `release/commands/<name>.exe`

## 2. Core Watch Directories Contract

Core resolves directories in this order:

```
1. LOCAL dirs (sibling to core.exe binary):
     <binary_dir>/agents/
     <binary_dir>/tools/
     <binary_dir>/commands/

2. USER dirs (%APPDATA%/scarllet/):
     %APPDATA%/scarllet/agents/
     %APPDATA%/scarllet/tools/
     %APPDATA%/scarllet/commands/
```

### Override rule

When the same module `name` (from `--manifest` output) is registered from both a local and user dir:
- The **user dir version replaces** the local version.
- User dir is scanned **after** local dirs, so user modules naturally override.

### Resolution function signature

```rust
pub fn watched_dirs() -> Vec<PathBuf>
```

Returns all 6 directories (3 local + 3 user). Creates any missing dirs silently.

## 3. TUI Core Launch Contract

```rust
fn spawn_core() -> io::Result<()>
```

- Resolves `std::env::current_exe()` → parent directory
- Looks for sibling named `core` (+ `.exe` on Windows)
- If found → spawn it
- If NOT found → return `Err` (do NOT search other paths)

## 4. Workspace Members Contract

Root `Cargo.toml` `[workspace].members`:

```toml
[workspace]
resolver = "2"
members = [
  "packages/rust/scarllet-proto",
  "packages/rust/scarllet-sdk",
  "packages/rust/scarllet-core",
  "packages/rust/scarllet-tui",
  "packages/rust/scarllet-llm",
  "packages/rust/tools/echo-tool",
  "packages/rust/agents/default",
]
```
