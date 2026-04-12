---
status: done
order: 1
created: 2026-04-10 19:48
title: "Scaffold workspace crates and Core daemon boot"
---

## Description

Bootstrap the entire Nx + Cargo workspace from empty to five compilable crates. The Core daemon starts a tonic gRPC server on a random localhost port and writes a lockfile so future clients can discover it. This is the foundation every subsequent effort builds on.

## Objective

Running `npx nx run scarllet-core:run` starts the Core daemon, which binds to a random port on `127.0.0.1`, writes a JSON lockfile to the OS-standard config directory, logs its listening address, and serves a `Ping` health-check RPC. Ctrl+C exits cleanly and removes the lockfile.

## Implementation Details

1. **Scaffold 5 Nx Rust projects** using `npx nx generate @monodon/rust:library` (for `scarllet-proto`, `scarllet-sdk`, `scarllet-llm`) and `npx nx generate @monodon/rust:binary` (for `scarllet-core`, `scarllet-tui`) under `packages/rust/`.
2. **Update root `Cargo.toml`** to list all 5 crate paths in `[workspace].members`.
3. **`scarllet-proto`:**
   - Add `proto/orchestrator.proto` with a single `Ping` RPC: `rpc Ping(PingRequest) returns (PingResponse)`.
   - Add `build.rs` using `tonic-build` to compile the proto at build time.
   - `Cargo.toml` dependencies: `tonic`, `prost`; build-deps: `tonic-build`.
4. **`scarllet-sdk`:**
   - Re-export proto-generated types.
   - Add lockfile types: `CoreLockfile { pid, address, started_at }`.
   - Add lockfile read/write helpers using `dirs::config_dir()` → `scarllet/core.lock`.
   - `Cargo.toml` dependencies: `scarllet-proto`, `serde`, `serde_json`, `dirs`.
5. **`scarllet-core`:**
   - `main.rs`: Initialize `tokio` runtime + `tracing` subscriber.
   - Bind tonic server to `127.0.0.1:0` (random port).
   - Implement `Ping` handler (returns `PingResponse` with uptime).
   - Write lockfile after bind (using SDK helpers).
   - Register `ctrlc` handler that removes lockfile and shuts down gracefully.
   - `Cargo.toml` dependencies: `scarllet-proto`, `scarllet-sdk`, `tonic`, `tokio`, `tracing`, `tracing-subscriber`, `ctrlc`, `clap`.
6. **`scarllet-tui`:** Minimal `main.rs` with `fn main() { println!("tui placeholder"); }` — just enough to compile. Real TUI work is Effort 2.
7. **`scarllet-llm`:** Minimal `lib.rs` placeholder — just enough to compile. Real LLM work is Effort 8.
8. **Configure `project.json`** for each crate following `@monodon/rust` template (build, test, lint, clean targets; `target-dir` under `dist/target/<crate-name>`).

## Verification Criteria

- `npx nx run scarllet-proto:build` compiles proto codegen successfully.
- `npx nx run scarllet-sdk:build` compiles with proto types available.
- `npx nx run scarllet-core:run` starts the daemon, prints `Listening on 127.0.0.1:<port>` to stdout, and creates `scarllet/core.lock` in the OS config directory.
- Lockfile contains valid JSON with `pid`, `address`, and `started_at` fields.
- A manual `grpcurl` or test client calling `Ping` returns a valid `PingResponse`.
- Ctrl+C in the Core terminal → lockfile is removed, process exits with code 0.
- `npx nx run-many -t build` compiles all 5 crates without errors.
- `npx nx run-many -t lint` passes clippy for all crates.

## Done

- All 5 Nx Rust projects exist under `packages/rust/` with `project.json` and `Cargo.toml`.
- Core daemon runs, listens on gRPC, writes and cleans up lockfile — observable in terminal output and filesystem.

## Change Summary

### Files created
- `packages/rust/scarllet-proto/Cargo.toml` — proto crate manifest with tonic, tonic-prost, prost deps; protox + tonic-prost-build as build deps
- `packages/rust/scarllet-proto/build.rs` — protox-based proto compilation (pure Rust, no external protoc required)
- `packages/rust/scarllet-proto/proto/orchestrator.proto` — Ping RPC definition
- `packages/rust/scarllet-proto/src/lib.rs` — re-exports generated proto types under `proto` module
- `packages/rust/scarllet-proto/project.json` — Nx library project with check/test/lint targets
- `packages/rust/scarllet-sdk/Cargo.toml` — SDK crate manifest
- `packages/rust/scarllet-sdk/src/lib.rs` — re-exports proto and declares lockfile module
- `packages/rust/scarllet-sdk/src/lockfile.rs` — CoreLockfile struct, write/read/remove helpers, OS config dir resolution
- `packages/rust/scarllet-sdk/project.json` — Nx library project
- `packages/rust/scarllet-core/Cargo.toml` — Core binary manifest with full dependency set
- `packages/rust/scarllet-core/src/main.rs` — Core daemon: tokio runtime, tonic server on random port, Ping handler, lockfile write, ctrlc shutdown
- `packages/rust/scarllet-core/project.json` — Nx application project with build/test/lint/run targets
- `packages/rust/scarllet-tui/Cargo.toml` — placeholder binary
- `packages/rust/scarllet-tui/src/main.rs` — placeholder main
- `packages/rust/scarllet-tui/project.json` — Nx application project
- `packages/rust/scarllet-llm/Cargo.toml` — placeholder library
- `packages/rust/scarllet-llm/src/lib.rs` — placeholder with one test
- `packages/rust/scarllet-llm/project.json` — Nx library project

### Files modified
- `Cargo.toml` (root) — populated `[workspace].members` with all 5 crate paths

### Key decisions
- **Manual crate creation instead of `@monodon/rust` generators:** The generator snake_cases directory names and prefixes Nx project names with directory paths (e.g., `rust_scarllet_proto`). Created files directly for clean names (`scarllet-proto`).
- **`protox` instead of external `protoc`:** System `protoc` was not installed and couldn't be installed without admin rights. Used `protox` (pure Rust protobuf compiler, 9M+ downloads) as a build dependency — zero system requirements.
- **`tonic-prost-build` instead of `tonic-build`:** In tonic 0.14, the proto compilation API moved from `tonic_build::compile_protos()` to `tonic_prost_build::compile_protos()` (and `tonic-prost` is now a separate runtime dependency).
- **`started_at` as unix timestamp (u64):** Avoids chrono dependency; lockfile is machine-read.

### Deviations from Implementation Details
- Effort spec said "use `npx nx generate @monodon/rust:library/binary`" — deviated to manual file creation because the generator's naming conventions conflict with desired project names. Functionally identical.
- Effort spec said `tonic-build` as build dep — changed to `tonic-prost-build` + `protox` due to tonic 0.14 API changes and missing system protoc.
