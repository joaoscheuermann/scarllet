# Copilot Instructions — Project Reorganization + Release Build

## General rules

- Follow early returns and guard clauses.
- Pass dependencies explicitly.
- Do NOT delete old files until the new location is verified to build.
- Update all `$schema` relative paths when project.json files move deeper.
- Run `cargo check` after each structural change to catch broken paths.

## Effort 1: Move echo-tool to tools/ subdirectory

1. Create directory `packages/rust/tools/echo-tool/`.
2. Move `packages/rust/echo-tool/src/`, `Cargo.toml`, `project.json` to the new location.
3. Update `project.json`:
   - Fix `$schema` path: `"../../../../node_modules/nx/schemas/project-schema.json"` (4 levels up now).
   - Fix `sourceRoot`: `"packages/rust/tools/echo-tool/src"`.
4. Update root `Cargo.toml`: change `"packages/rust/echo-tool"` → `"packages/rust/tools/echo-tool"`.
5. Delete the old `packages/rust/echo-tool/` directory.
6. Verify: `cargo check -p echo-tool` passes.

## Effort 2: Move chat agent to agents/default

1. Create directory `packages/rust/agents/default/src/`.
2. Move (or recreate) the chat agent source into `packages/rust/agents/default/`.
3. Write `Cargo.toml` with `name = "default-agent"`. Dependency paths use `../../scarllet-proto` etc. (2 levels up from `agents/default/` to `packages/rust/`).
4. Write `project.json` with `name: "default-agent"`, `$schema` 4 levels up, `sourceRoot` at `packages/rust/agents/default/src`.
5. Update root `Cargo.toml`: remove `"packages/rust/scarllet-chat-agent"`, add `"packages/rust/agents/default"`.
6. Update the agent manifest in `main.rs`: `"name": "default"` (was `"chat"`).
7. Delete old `packages/rust/scarllet-chat-agent/`.
8. Verify: `cargo check -p default-agent` passes.

## Effort 3: Update Core watcher — dual directories

**File**: `packages/rust/scarllet-core/src/watcher.rs`

Update `watched_dirs()`:

```rust
pub fn watched_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Local dirs — sibling to the binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            dirs.push(bin_dir.join("commands"));
            dirs.push(bin_dir.join("tools"));
            dirs.push(bin_dir.join("agents"));
        }
    }

    // User dirs — %APPDATA%/scarllet/
    let user_base = dirs::config_dir()
        .expect("could not determine OS config directory")
        .join("scarllet");
    dirs.push(user_base.join("commands"));
    dirs.push(user_base.join("tools"));
    dirs.push(user_base.join("agents"));

    dirs
}
```

**Override behavior**: User dirs are listed AFTER local dirs. When `handle_file_added` registers a module, it calls `registry.register(path, manifest)` which uses `HashMap::insert` — later registrations with the same name overwrite earlier ones. Since user dirs are scanned after local dirs during the initial scan loop, user modules naturally win.

Verify this by checking the initial scan order in `run()`: the `for d in &dirs` loop processes dirs in order, so local dirs (indices 0-2) are scanned before user dirs (indices 3-5).

## Effort 4: Update TUI spawn_core — sibling binary name

**File**: `packages/rust/scarllet-tui/src/main.rs`

Change `spawn_core()`:

```rust
fn spawn_core() -> io::Result<()> {
    let self_path = std::env::current_exe()?;
    let dir = self_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine binary dir"))?;
    let mut core_path = dir.join("core");
    if cfg!(windows) {
        core_path.set_extension("exe");
    }
    if !core_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Core binary not found at {}", core_path.display()),
        ));
    }
    std::process::Command::new(&core_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
```

Only change: `dir.join("scarllet-core")` → `dir.join("core")`.

## Effort 5: Create release build target + copy script

**File**: `scripts/release.ps1` (new)

```powershell
$ErrorActionPreference = "Stop"

$releaseDir = Join-Path $PSScriptRoot ".." "release"
$cargoRelease = Join-Path $PSScriptRoot ".." "target" "release"

# Clean and create structure
if (Test-Path $releaseDir) { Remove-Item -Recurse -Force $releaseDir }
New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $releaseDir "agents") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $releaseDir "tools") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $releaseDir "commands") | Out-Null

# Copy binaries with rename
Copy-Item (Join-Path $cargoRelease "scarllet-core.exe") (Join-Path $releaseDir "core.exe")
Copy-Item (Join-Path $cargoRelease "scarllet-tui.exe") (Join-Path $releaseDir "tui.exe")
Copy-Item (Join-Path $cargoRelease "default-agent.exe") (Join-Path $releaseDir "agents" "default.exe")
Copy-Item (Join-Path $cargoRelease "echo-tool.exe") (Join-Path $releaseDir "tools" "echo-tool.exe")

Write-Host "Release folder created at: $releaseDir"
```

**File**: `project.json` (root level — new)

```json
{
  "name": "scarllet",
  "$schema": "node_modules/nx/schemas/project-schema.json",
  "targets": {
    "release": {
      "executor": "nx:run-commands",
      "options": {
        "commands": [
          "cargo build --release",
          "powershell -ExecutionPolicy Bypass -File scripts/release.ps1"
        ],
        "parallel": false
      }
    }
  }
}
```

Run via: `npx nx run scarllet:release`

## Effort 6: Verify end-to-end

1. `npx nx run scarllet:release` — builds and copies.
2. `ls release/` — verify layout matches the contract.
3. `cd release && ./core.exe` — Core starts, watches `release/agents/`, `release/tools/` + `%APPDATA%` dirs.
4. `./tui.exe` — TUI finds `core.exe` as sibling, connects.
5. Type a prompt — if Gemini key is set, default agent responds.

## Do NOT

- Do not rename the library crates (scarllet-proto, scarllet-sdk, scarllet-llm) — they are not binaries.
- Do not change the proto definitions or gRPC contracts.
- Do not modify the TUI layout or chat rendering code.
- Do not add cross-platform shell scripts yet (PowerShell only for MVP).
- Do not create a generic "binary discovery" framework — hardcode the copy list in the release script.
