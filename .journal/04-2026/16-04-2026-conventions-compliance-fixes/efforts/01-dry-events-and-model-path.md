---
status: done
order: 1
created: 2026-04-16 09:55
title: "DRY: Extract events.rs in core + normalize_model_path in gemini"
---

## Description

Eliminate two DRY violations before any structural splits. Create `scarllet-core/src/events.rs` with a single `build_provider_info_event` function used by 3 call sites. Extract `normalize_model_path` helper in `scarllet-llm/src/gemini.rs` used by 2 call sites.

## Objective

After this effort, the provider-info event construction logic exists in exactly one place (`events.rs`), and the Gemini model path normalization exists in exactly one place (`normalize_model_path`). Both crates compile and all existing tests pass.

## Implementation Details

### events.rs (scarllet-core)

1. Create `packages/rust/scarllet-core/src/events.rs`.
2. Move `build_provider_info_event(cfg: &ScarlletConfig) -> CoreEvent` from `main.rs` into `events.rs` as `pub(crate)`.
3. In `main.rs`: replace the inline function with `use crate::events::build_provider_info_event;`.
4. In `watcher.rs` (`watch_config` function, lines 206-224): replace the inline event construction with a call to `events::build_provider_info_event(&*cfg)`.
5. Add `mod events;` to `main.rs`.

### normalize_model_path (scarllet-llm)

1. In `packages/rust/scarllet-llm/src/gemini.rs`, add a private helper:
   ```rust
   fn normalize_model_path(model: &str) -> String {
       if model.starts_with("models/") { model.to_string() } else { format!("models/{model}") }
   }
   ```
2. Replace the duplicated 5-line blocks in `create_client()` (line 20) and `get_context_window()` (line 319) with calls to `normalize_model_path(model)`.

## Verification Criteria

1. `npx nx run scarllet-core:build` succeeds.
2. `npx nx run scarllet-core:test` passes (existing tests).
3. `npx nx run scarllet-llm:build` succeeds.
4. `npx nx run scarllet-llm:test` passes (existing tests in `openai.rs`).
5. Run `.\release\core.exe`, connect with TUI, verify provider info still appears in status bar — confirms the extracted event function works at runtime.

## Done

- `events.rs` exists in `scarllet-core/src/` with `build_provider_info_event`.
- `main.rs` and `watcher.rs` both import and call it instead of inline construction.
- `gemini.rs` has `normalize_model_path` used by both `create_client` and `get_context_window`.
- Core + TUI start and display provider info correctly.
