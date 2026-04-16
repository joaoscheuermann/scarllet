---
status: done
created: 2026-04-16 09:45
slug: conventions-compliance-fixes
---

## Prompt

@.agents/skills/architect/SKILL.md write a plan for the fixes. I also need you to add comments in all methods to explain what they do.

## Research

(empty)

## Architecture

### Overview

Refactor the Scarllet Rust monorepo to comply with coding-conventions. No new crates. No proto schema changes. No external behavior changes. All fixes are module-level splits and extractions within existing crates, plus doc comments on all 31 `.rs` files.

### Fix Plan (Approved)

#### Fix 1: DRY — Provider Info Event (3 duplicates → 1)
Extract `build_provider_info_event(cfg: &ScarlletConfig) -> CoreEvent` into `scarllet-core/src/events.rs`. Used by `main.rs`, `watcher.rs`, and `service.rs` (after SRP split).

#### Fix 2: DRY — Gemini Model Path Normalization (2 duplicates → 1)
Extract `fn normalize_model_path(model: &str) -> String` in `gemini.rs`. Used by `create_client()` and `get_context_window()`.

#### Fix 3: DRY — Scroll Logic in TUI (3 duplicates → 1)
Extract `fn scroll_page(state: &mut ScrollViewState, direction, page_height)` helper after SRP split.

#### Fix 4: SRP — Split `scarllet-core/src/main.rs` (754 → 4 modules)
- `main.rs` — Cli, main(), bootstrap (~80 lines)
- `service.rs` — OrchestratorService + Orchestrator trait impl (~400 lines)
- `routing.rs` — route_prompt (~160 lines)
- `events.rs` — build_provider_info_event (~25 lines, from Fix 1)

#### Fix 5: SRP — Split `scarllet-tui/src/main.rs` (1075 → 5 modules)
- `main.rs` — main(), terminal setup/teardown, event loop (~80 lines)
- `app.rs` — App struct, ChatEntry, DisplayBlock, Focus, ToolCallData (~200 lines)
- `events.rs` — handle_core_event, find_agent_entry, proto_blocks_to_display (~130 lines)
- `render.rs` — routes, draw_connecting, draw_chat, draw_history, draw_input, draw_status_bar (~400 lines)
- `connection.rs` — connect_and_stream, connect_to_core, find_core_address, spawn_core (~100 lines)

#### Fix 6: ISP — Extract AgentContext struct
Group 11 params of `run_tool_loop` into `AgentContext` struct. Remove `#[allow(clippy::too_many_arguments)]`.

#### Fix 7: DIP — Make Hidden Dependencies Explicit
- 7a: `GeminiProvider` — store `http: reqwest::Client`, accept optional `api_base_url`
- 7b: `config::config_path()` — add `config_path_with_override(Option<&Path>)` variant
- 7c: `App::new` — pass `cwd: PathBuf` and `debug_enabled: bool` as parameters

#### Fix 8: OCP — String-based Status Dispatch (DEFERRED)
Leave as-is. Add doc comment explaining the protocol contract. Proto schema change required for full fix.

#### Fix 9: KISS — Notify-based Agent Registration
Replace polling loop in `route_prompt` with `tokio::sync::Notify`. Agent registration handler triggers notify, `route_prompt` awaits with timeout. Add comment about alternative B (buffered task queue) for future reference.

#### Fix 10: Doc Comments on All Methods
All 31 `.rs` files. Every fn/pub fn/async fn gets `///` doc comment. Done last, after all structural refactors.

### Implementation Order

1. DRY: events.rs + normalize_model_path (Fixes 1, 2)
2. SRP: Split scarllet-core/src/main.rs (Fix 4)
3. SRP: Split scarllet-tui/src/main.rs + scroll helper (Fixes 3, 5)
4. ISP: Extract AgentContext (Fix 6)
5. DIP: Gemini HTTP client, config path, TUI deps (Fix 7)
6. KISS: Notify-based agent registration (Fix 9)
7. Doc comments on all 31 files (Fixes 8, 10)

### Verification

```powershell
npx nx run-many -t build
npx nx run-many -t test
npx nx run-many -t lint
```
