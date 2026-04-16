---
status: done
order: 2
created: 2026-04-16 09:56
title: "SRP: Split scarllet-core/src/main.rs into modules"
---

## Description

Split the 754-line `scarllet-core/src/main.rs` into 4 focused modules. Depends on Effort 1 (events.rs already exists). No behavior changes — pure structural refactor.

## Objective

After this effort, `scarllet-core/src/main.rs` is ~80 lines (Cli struct, main function, server bootstrap). All gRPC service methods live in `service.rs`, prompt routing in `routing.rs`, and event helpers in `events.rs` (from Effort 1). The crate compiles and all tests pass.

## Implementation Details

### service.rs

1. Create `packages/rust/scarllet-core/src/service.rs`.
2. Move `OrchestratorService` struct definition and the entire `#[tonic::async_trait] impl Orchestrator for OrchestratorService` block into `service.rs`.
3. Make `OrchestratorService` fields `pub(crate)` so `main.rs` can construct it.
4. Add necessary `use` imports at the top of `service.rs`.

### routing.rs

1. Create `packages/rust/scarllet-core/src/routing.rs`.
2. Move `route_prompt` function into `routing.rs` as `pub(crate)`.
3. `service.rs` calls `routing::route_prompt(...)` from the `attach_tui` handler.

### main.rs (slimmed)

1. Keep only: `mod` declarations, `Cli` struct, `main()` function.
2. `mod agents; mod events; mod registry; mod routing; mod sessions; mod service; mod tasks; mod tools; mod watcher;`
3. `main()` constructs `OrchestratorService` (via `service::OrchestratorService { ... }`) and starts the server.

### Import adjustments

Each new module needs its own `use` block. Shared types come from `scarllet_proto::proto::*`, `scarllet_sdk::config`, and crate-internal modules. Cross-module references use `crate::` paths.

## Verification Criteria

1. `npx nx run scarllet-core:build` succeeds.
2. `npx nx run scarllet-core:test` passes.
3. `npx nx run scarllet-core:lint` passes with no new warnings.
4. Run `.\release\core.exe`, connect TUI, send a prompt — full round-trip works (agent responds, tool calls execute, provider info shows).

## Done

- `main.rs` is ~80 lines: Cli + main + bootstrap.
- `service.rs` contains OrchestratorService and all Orchestrator trait methods.
- `routing.rs` contains route_prompt.
- `events.rs` contains build_provider_info_event (from Effort 1).
- Core starts, accepts TUI connections, routes prompts to agents, and returns responses.
