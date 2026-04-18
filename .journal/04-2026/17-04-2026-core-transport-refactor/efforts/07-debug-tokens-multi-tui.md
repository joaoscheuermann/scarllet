---
status: done
order: 7
created: 2026-04-17 13:18
title: "Debug, TokenUsage, Error polish + multi-TUI attach"
---

## Description

Land the last three node kinds (`Debug`, `TokenUsage`, `Error`) as first-class citizens on the agent side and in the TUI; replace every remaining tracing / out-of-band log path used by the default agent with `Debug` nodes. Add the SDK helpers. Confirm multi-TUI attach works: two TUIs on the same session id see the same diff stream, each applies the debug-node filter independently based on its own `SCARLLET_DEBUG` env flag. Polish `Error` node rendering (both top-level session errors from AC-3.3 and agent-parented errors from AC-3.4).

## Objective

Three observable scenarios:

### A — Debug flag toggle on two TUIs

1. Start the core implicitly via TUI-A without `SCARLLET_DEBUG`.
2. Note the session id from TUI-A's status bar.
3. Start a second TUI-B in a separate terminal, setting `SCARLLET_DEBUG=true` and passing the session id (via a new CLI flag `--session <id>`).
4. Send a prompt from TUI-A. In both TUIs the User/Agent/Thought/Result nodes stream in sync.
5. TUI-B additionally shows Debug nodes under the Agent (e.g. `[debug] Using provider: openrouter / gpt-4o`). TUI-A does not.
6. Close TUI-A. The session persists (TUI-B is still attached). Send another prompt from TUI-B. Debug nodes continue to render there.
7. Close TUI-B. Session is destroyed (last subscriber gone; `SessionDestroyed` diff fires — nobody receives it, session is GC'd).

### B — Token usage visible

1. After any LLM-backed turn, TUI status bar shows `tokens: <total>/<window>` (e.g. `tokens: 420/128000`) in the footer. Values update on every `TokenUsage` node update.

### C — Error node rendering

1. With missing `default_agent` config (triggered in effort 4), the top-level red `Error` node renders distinctly at the top of the chat (no Agent indent).
2. Force an LLM error mid-turn (bad API key for one send). The per-turn `Error` node appears indented under the owning Agent node, styled red.

## Implementation Details

### 1. Core — node validation

- `session/nodes.rs::create` rules refresher: `Debug` / `TokenUsage` must have an Agent parent; `Error` may be top-level OR have an Agent parent (top-level for session errors, agent-parented for turn errors). Add the test matrix.
- `node_patch` merges: `error_message`, `token_total`, `token_window` are **replace** (already encoded in effort 2).

### 2. SDK — `scarllet-sdk/src/agent/mod.rs`

Add:

- `pub async fn emit_debug(&self, level: &str, message: &str) -> Result<(), _>` — creates a `Debug` node parented to the agent's Agent node.
- `pub async fn emit_token_usage(&self, total: u32, window: u32) -> Result<(), _>` — creates a `TokenUsage` node parented to the Agent node (one per turn is typical; if called twice, two nodes are created — TUI displays the latest).
- `pub async fn emit_error(&self, message: &str) -> Result<(), _>` — creates an `Error` node parented to the Agent node (for the per-turn error path). For top-level session errors, core is responsible; agents never need to emit those.

### 3. Default agent — migrate debug_log calls

Replace every legacy `debug_log(...)` call (the old `EmitDebugLog` RPC has already been deleted from the proto in effort 1; the compiler is forcing you to address these now) with `session.emit_debug(level, message).await?`. Examples:

- `"Using provider type: gemini"` → debug
- `"Tools available: [tree, grep, …]"` → debug
- `"Stream ended: finish_reason=stop, tool_calls=2"` → debug

At the end of each turn (or whenever `Usage` is available), call `session.emit_token_usage(total, window).await?`.

On LLM error inside the tool loop, before returning Err: call `session.emit_error(&e.to_string()).await?` so a visible `Error` node is produced even if the process dies before core notices the disconnect (belt-and-suspenders with effort 6's Paused path).

### 4. TUI — render path

- `app.rs`: add `debug_enabled: bool` (from `SCARLLET_DEBUG` env at startup).
- Render loop: walk nodes. Skip nodes where `kind == Debug && !app.debug_enabled`. Everything else renders.
- `Error` nodes: if `parent_id.is_none()` → render as a top-level red banner with left-margin 0 (same lane as User and Agent nodes). If parented → render indented under the Agent card, red foreground.
- `TokenUsage` nodes: don't render inline in the chat body; surface the latest one as `tokens: <total>/<window>` in the status bar. Keep the node in `app.nodes` so future clients can surface it differently.

### 5. TUI — session id CLI flag

- Extend `packages/rust/scarllet-tui/src/main.rs` to accept `--session <id>` via `clap` (TUI is already a binary — add clap if not already a dep).
- If `--session <id>` is passed and the id exists on core, `AttachSession { session_id: Some(id) }`; if core rejects (session doesn't exist), fall back to `AttachSession { session_id: None }` and show a status message "session <id> not found; started a new one".
- Without the flag, behaviour is unchanged (auto-create).

### 6. Multi-TUI subscriber handling

- Confirm `SubscriberSet::broadcast` clones the diff and sends to **all** subscribers; already true, but add a unit test that two subscribers receive the same stream of diffs.
- The `destroy-on-last-detach` rule (AC-2.5) — ensure it only fires when `subscribers.len() == 0`, not when a single subscriber drops. Add a unit test.

### 7. Unit tests

- `NodeStore::create` accepts `Error { parent_id: None }`, accepts `Error { parent_id: Some(agent_node) }`, rejects `Debug { parent_id: None }`, rejects `TokenUsage { parent_id: None }`, rejects `Thought { parent_id: Some(non-agent-node) }`.
- `SubscriberSet` multi-subscriber broadcast.
- `scarllet-tui` render filter test (pure function over a node list, input: `debug_enabled: bool`, expected: Debug nodes present/absent).

## Verification Criteria

1. All crates build, clippy clean.
2. `npx nx run scarllet-core:test` + `scarllet-tui:test` — new tests green.
3. **Run & observe (required, scenario A)**: execute §Objective A end-to-end. Key observables:
   - Two TUIs render the same User/Agent/Thought/Result sequence.
   - Only the `SCARLLET_DEBUG=true` TUI shows Debug nodes.
   - Closing the last TUI destroys the session (verify via core's log or an attempt to `GetSessionState` after which returns NOT_FOUND).
4. **Run & observe (required, scenario B)**: after any normal prompt, TUI status bar updates token counters (non-zero total, correct window).
5. **Run & observe (required, scenario C)**: trigger both Error variants (top-level via missing `default_agent`; per-turn via bad API key). Both render distinctly.

## Done

- Debug / TokenUsage / Error node kinds work end-to-end with validation.
- Default agent emits all former tracing/out-of-band signals through `Debug` nodes.
- Token counts are visible in the TUI footer.
- Multi-TUI attach works with independent debug filtering and correct last-detach destruction semantics.

## Change Summary

### Files modified (this session)

- `packages/rust/scarllet-tui/src/connection.rs` — clippy `large_enum_variant` auto-fix: `AttachedState` is now boxed inside the `Attached` diff variant (`Attached(Box<AttachedState>)`), and the construction site wraps the struct in `Box::new(AttachedState { … })`. No semantic change.

### Files verified in place (landed by efforts 02–06 going slightly beyond their own scope; confirmed to match effort 07's spec point-by-point)

- `packages/rust/scarllet-core/src/session/nodes.rs` — AC-5.4 parent validation + the 5 required unit-test cases (`Error{parent_id: None}` accepted, `Error{parent_id: Some(agent)}` accepted, `Debug{parent_id: None}` rejected, `TokenUsage{parent_id: None}` rejected, `Thought{parent_id: Some(non-agent)}` rejected).
- `packages/rust/scarllet-core/src/session/subscribers.rs` — multi-subscriber broadcast + last-detach unit tests (`multiple_subscribers_receive_identical_ordered_stream`, `single_subscriber_drop_does_not_empty_the_set`, `broadcast_prunes_closed_senders`).
- `packages/rust/scarllet-sdk/src/agent/mod.rs` — `emit_debug` (line 474), `emit_token_usage` (line 500), `emit_error` (line 526).
- `packages/rust/agents/default/src/main.rs` — 15 `emit_debug` call sites replacing every prior `tracing::*!` that would have been a debug log; `emit_token_usage(total, window)` at turn end when `Usage` is available (line 181); `emit_error` belt-and-suspenders on LLM / tool-loop failure paths (lines 76, 234, 254).
- `packages/rust/scarllet-tui/Cargo.toml` — `clap = { version = "4", features = ["derive"] }` dep.
- `packages/rust/scarllet-tui/src/main.rs` — `#[derive(clap::Parser)] struct Args { session: Option<String> }`; `debug_enabled` read from `SCARLLET_DEBUG` env.
- `packages/rust/scarllet-tui/src/connection.rs` — fallback-attach path (`--session <id>` unknown → `AttachSession{None}` + client-side status `Error` diff `"session <id> not found; started a new one"`).
- `packages/rust/scarllet-tui/src/app.rs` — `debug_enabled: bool` field + `latest_token_usage()` helper.
- `packages/rust/scarllet-tui/src/render.rs` — `draw_status_bar` surfaces `tokens: <total>/<window>` when a `TokenUsage` node exists.
- `packages/rust/scarllet-tui/src/widgets/chat_message.rs` — top-level red `⚠` banner for unparented `Error`, indented red `✗` line for Agent-parented `Error`, `is_inline_visible`, debug-gated render path; 4 unit tests covering the render filter.

### Files created / deleted

None.

### Key decisions / trade-offs

- **No new code authored in this session.** The effort's Implementation Details were already satisfied by in-flight work in efforts 02–06 (SDK helpers accumulated as adjacent utilities; the default agent was migrated onto them as the tool loop was rebuilt in 03/04; the TUI's `--session` flag, `debug_enabled`, token footer, and `Error` styling all landed alongside effort 06's `PAUSED` lifecycle segment). Re-authoring would have been a DRY violation per `coding-conventions/references/dry.md`.
- The single `large_enum_variant` clippy auto-fix on `connection.rs` was kept rather than reverted — it is the canonical fix clippy itself suggests, and reverting would re-trigger the warning on every subsequent run.

### Deviations from Implementation Details

None. The effort's bullet points map 1:1 to what exists on disk; the unit-test matrix is present at `session/nodes.rs:704-755`, `session/subscribers.rs:99-146`, and `widgets/chat_message.rs:611-738`.

### Verification

| # | Criterion | Result |
|---|---|---|
| 1 | All crates build; `cargo clippy --workspace --all-targets` clean of new warnings | PASS — `cargo build --workspace` exit 0; clippy exit 0 (remaining warnings live in `input.rs`, `git_info.rs`, `openai.rs`, `tools/*` — all pre-existing, `git diff HEAD` empty on those files) |
| 2 | `scarllet-core:test` + `scarllet-tui:test` green incl. the 5 `NodeStore::create` cases + multi-subscriber + last-detach + TUI render-filter tests | PASS — 80 / 0 in `scarllet-core`, 7 / 0 in `scarllet-tui`, 17 / 0 in `scarllet-sdk` |
| 3 | Scenario A — two TUIs + `SCARLLET_DEBUG` toggle, last-detach destruction | **DEFERRED — human required** |
| 4 | Scenario B — `tokens: <total>/<window>` footer updates after each turn | **DEFERRED — human required** |
| 5 | Scenario C — top-level `Error` banner on missing `default_agent`, indented `Error` on bad API key | **DEFERRED — human required** |

Independent tester invoked and confirmed PASS for 1–2, DEFERRED for 3–5.

### Pending human verification

With a working LLM provider configured, from `c:\Users\jvito\Documents\git\scarllet`:

1. Scenario A — two TUIs + debug filter.
   - Terminal 1: `Remove-Item Env:SCARLLET_DEBUG -ErrorAction SilentlyContinue; npx nx run scarllet-tui:run`. Note the session id.
   - Terminal 2: `$env:SCARLLET_DEBUG = "true"; cargo run -p scarllet-tui -- --session <id>`.
   - Send a prompt from Terminal 1 → both TUIs show identical User/Agent/Thought/Result; only Terminal 2 shows `[debug info] …` lines under the Agent.
   - Close Terminal 1; send another prompt from Terminal 2; debug nodes still render.
   - Close Terminal 2; session is destroyed. Re-attach to the same id: expect a red banner `session <id> not found; started a new one`.
2. Scenario B — `npx nx run scarllet-tui:run`, send any prompt → after the Result, status bar bottom-right shows `tokens: <total>/<window>`.
3. Scenario C.1 — blank out `default_agent` in `~/.scarllet/config.toml`, relaunch TUI, send a prompt → expect a red banner at column 0 `⚠ Error (core): default_agent not configured`. Restore config.
4. Scenario C.2 — set `OPENAI_API_KEY` to a bad key, relaunch TUI, send a prompt → expect a red indented `✗ Error: …` line under the failing Agent card + lifecycle flips to `PAUSED`. Press `Esc` to resume.
