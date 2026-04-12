---
status: done
order: 3
created: 2026-04-10 19:48
title: "Module discovery and hot-reload via directory watching"
---

## Description

Implement the Core's directory watcher using the `notify` crate to observe `commands/`, `tools/`, and `agents/` directories. When a new executable appears, Core runs it with `--manifest`, parses the JSON manifest, and registers the module. Invalid files are silently ignored. File removal deregisters the module. Add the manifest JSON schema to `scarllet-sdk` and the `ListCommands` / `GetToolRegistry` gRPC RPCs to `scarllet-proto`.

## Objective

While Core is running, dropping a valid executable into the configured `tools/` directory causes Core to log its registration within seconds. Removing it causes deregistration. Dropping a non-executable file produces no error. The TUI (or a test gRPC client) can call `ListCommands` or `GetToolRegistry` and see the currently registered modules.

## Implementation Details

1. **`scarllet-sdk` manifest types:**
   - Define `ModuleManifest` struct: `name`, `kind` (enum: `Command`, `Tool`, `Agent`), `version`, `description`, `input_schema` (optional JSON Schema value), `timeout_ms` (optional), `capabilities` (optional vec), `aliases` (optional vec).
   - Serde JSON deserialization for manifest parsing.
2. **`scarllet-proto` additions:**
   - `rpc ListCommands(ListCommandsRequest) returns (ListCommandsResponse)` — returns registered command names + aliases.
   - `rpc GetToolRegistry(ToolRegistryQuery) returns (ToolRegistryResponse)` — returns registered tools with their schemas and timeouts.
3. **`scarllet-core` directory watcher:**
   - On startup, resolve watched directories from config (default: `dirs::config_dir()/scarllet/commands`, `tools`, `agents`). Create directories if they don't exist.
   - Use `notify::RecommendedWatcher` with debouncing to watch all three directories.
   - On file create/modify: spawn the file with `--manifest`, capture stdout with a 5-second timeout. Parse JSON → register on success, silent ignore on failure (log at debug).
   - On file delete: deregister the module by path.
4. **Module registry:** In-memory `HashMap<PathBuf, ModuleManifest>` behind an `Arc<RwLock<...>>`. Shared with gRPC handlers.
5. **Test binary:** Create a minimal Rust binary (`packages/rust/test-fixtures/echo-tool/`) that responds to `--manifest` with a valid tool manifest JSON, and otherwise echoes stdin to stdout. This binary is for manual testing, not shipped.

## Verification Criteria

- Start Core → directories `commands/`, `tools/`, `agents/` are created under the config path.
- Copy the echo-tool binary into `tools/` → Core log shows "Registered tool: echo-tool".
- Call `GetToolRegistry` via grpcurl or test client → response includes "echo-tool" with its schema.
- Remove echo-tool from `tools/` → Core log shows "Deregistered tool: echo-tool".
- Call `GetToolRegistry` again → "echo-tool" no longer listed.
- Copy a plain text file into `tools/` → no error logged at default log level; module not registered.
- `npx nx run scarllet-core:test` passes unit tests for manifest parsing and registry logic.

## Done

- Core watches three directories and dynamically registers/deregisters modules based on `--manifest` protocol.
- Observable via Core logs and gRPC state queries.

## Change Summary

### Files created
- `packages/rust/scarllet-sdk/src/manifest.rs` — ModuleManifest, ModuleKind types with serde JSON deserialization + 2 unit tests
- `packages/rust/scarllet-core/src/registry.rs` — ModuleRegistry (HashMap<PathBuf, ModuleManifest>) with register/deregister/by_kind/version + 3 unit tests
- `packages/rust/scarllet-core/src/watcher.rs` — directory watcher using notify 7, probe_manifest via tokio::process with 5s timeout, initial scan + live events
- `packages/rust/echo-tool/` — test fixture binary that responds to --manifest with tool JSON, otherwise echoes stdin

### Files modified
- `packages/rust/scarllet-proto/proto/orchestrator.proto` — added ListCommands and GetToolRegistry RPCs with CommandInfo/ToolInfo messages
- `packages/rust/scarllet-sdk/src/lib.rs` — added manifest module
- `packages/rust/scarllet-core/Cargo.toml` — added notify, serde_json, dirs
- `packages/rust/scarllet-core/src/main.rs` — wired registry + watcher + new gRPC handlers
- `Cargo.toml` (root) — added echo-tool to workspace members

### Key decisions
- Used `dirs::config_dir()/scarllet/{commands,tools,agents}` as watched directories
- Initial scan of existing files on startup (before watching for events)
- ManifestExt trait for kind_str() display helper
- Registry version counter reserved for Effort 5 point-in-time snapshots

### Deviations
- None
