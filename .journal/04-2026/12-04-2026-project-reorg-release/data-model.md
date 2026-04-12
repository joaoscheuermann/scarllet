# Data Model — Project Reorganization + Release Build

## 1. Workspace Layout (after reorganization)

```
packages/rust/
├── scarllet-proto/          crate: scarllet-proto       (library)
├── scarllet-sdk/            crate: scarllet-sdk         (library)
├── scarllet-core/           crate: scarllet-core        (binary)
├── scarllet-tui/            crate: scarllet-tui         (binary)
├── scarllet-llm/            crate: scarllet-llm         (library)
├── agents/
│   └── default/             crate: default-agent        (binary)
└── tools/
    └── echo-tool/           crate: echo-tool            (binary)
```

## 2. Crate Manifests (Cargo.toml changes)

### `packages/rust/agents/default/Cargo.toml`

```toml
[package]
name = "default-agent"
version = "0.1.0"
edition = "2021"

[dependencies]
scarllet-proto = { path = "../../scarllet-proto" }
scarllet-llm = { path = "../../scarllet-llm" }
tonic = "0.14"
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
```

Note: `path` references go up two levels from `agents/default/` to reach `scarllet-proto/`.

### `packages/rust/tools/echo-tool/Cargo.toml`

```toml
[package]
name = "echo-tool"
version = "0.1.0"
edition = "2021"

[dependencies]
serde_json = "1"
```

Unchanged except path references if any (echo-tool has none).

## 3. Nx Project Configs (project.json)

### `packages/rust/agents/default/project.json`

```json
{
  "name": "default-agent",
  "$schema": "../../../../node_modules/nx/schemas/project-schema.json",
  "projectType": "application",
  "sourceRoot": "packages/rust/agents/default/src",
  "targets": {
    "build": {
      "executor": "@monodon/rust:build",
      "outputs": ["{options.target-dir}"],
      "options": {
        "target-dir": "dist/target/default-agent"
      }
    },
    "run": {
      "executor": "@monodon/rust:run",
      "options": {
        "target-dir": "dist/target/default-agent"
      }
    }
  },
  "tags": []
}
```

### `packages/rust/tools/echo-tool/project.json`

```json
{
  "name": "echo-tool",
  "$schema": "../../../../node_modules/nx/schemas/project-schema.json",
  "projectType": "application",
  "sourceRoot": "packages/rust/tools/echo-tool/src",
  "targets": {
    "build": {
      "executor": "@monodon/rust:build",
      "outputs": ["{options.target-dir}"],
      "options": {
        "target-dir": "dist/target/echo-tool"
      }
    },
    "run": {
      "executor": "@monodon/rust:run",
      "options": {
        "target-dir": "dist/target/echo-tool"
      }
    }
  },
  "tags": []
}
```

## 4. Root Release Target

### `project.json` (root level — new file)

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

## 5. Binary → Release Path Mapping

| Source (cargo --release output) | Destination |
|------|-------------|
| `target/release/scarllet-core.exe` | `release/core.exe` |
| `target/release/scarllet-tui.exe` | `release/tui.exe` |
| `target/release/default-agent.exe` | `release/agents/default.exe` |
| `target/release/echo-tool.exe` | `release/tools/echo-tool.exe` |

Note: `cargo build --release` (no `--target-dir`) uses the default `target/release/` directory in the workspace root.
