---
status: done
created: 2026-04-16 14:30
slug: session-persistence
---

## Prompt

As an user, I want to persist my last session when I close the TUI.
As an user, when I open the TUI again, I want to be able to see and continue using my previous session, with the same state and chat history.
As an user I want to be able to create a new session pressing CTRL + N in the keyboard.

## Research

SPEC already defined and approved:
- Storage: %APPDATA%\scarllet\session.json
- Format: JSON, UTF-8, no encryption, no size limit
- Single session (latest only, overwrites)
- Auto-save triggers: user prompt, agent response complete, TUI close, CTRL+N
- New session shortcut: CTRL+N (saves current first)
- Edge cases: mid-response close (discard streaming), corrupted file (start fresh), disk failure (log and continue), folder missing (create it)

Repo structure:
- packages/rust/scarllet-tui/     — TUI application (main, app, events, render, connection, input, widgets)
- packages/rust/scarllet-sdk/    — config persistence (already uses %APPDATA% via dirs::config_dir)
- packages/rust/scarllet-proto/  — protobuf schemas (CoreEvent, TuiMessage, etc.)
- packages/rust/scarllet-core/   — orchestrator service

## Architecture

### Overview

Add session persistence to `scarllet-tui`. No new crates. No proto schema changes. Single JSON file on disk.

### Category

**offline-data** — local JSON file persistence for conversation state.

### Design Principles Applied

- **SRP** — `session.rs` owns persistence; `App` owns state; `events.rs` owns input handling
- **DIP** — `SessionRepository` trait abstracts storage; injected into `App`
- **DRY** — reuse `dirs::config_dir()` pattern from `scarllet-sdk`
- **KISS** — single JSON file, no database, no encryption, no size limits

### New Files

| File | Purpose |
|------|---------|
| `packages/rust/scarllet-tui/src/session.rs` | `Session` model, `SessionRepository` trait, `FileSessionRepository`, unit tests |

### Modified Files

| File | Changes |
|------|---------|
| `packages/rust/scarllet-tui/Cargo.toml` | Add `serde`, `serde_json`, `tracing`, `uuid` deps |
| `packages/rust/scarllet-tui/src/lib.rs` | Add `mod session` |
| `packages/rust/scarllet-tui/src/app.rs` | Add `session_repo`, `session_id`, `save_session()`, `new_session()`, `load_from_session()` |
| `packages/rust/scarllet-tui/src/events.rs` | Call `save_session()` on user prompt, agent response, CTRL+N |
| `packages/rust/scarllet-tui/src/main.rs` | Load session on startup, save on exit, handle CTRL+N at loop level |

### Data Flow

```
[User Enter] → events::handle_input() → app.push_message() → app.save_session()
[Agent Done] → events::handle_core_event() → app.push_message() → app.save_session()
[CTRL+N]     → main loop → app.save_session() → app.new_session()
[Exit]       → main loop → app.save_session()
[Startup]    → main → FileSessionRepository::load() → app.load_from_session()
```

### Storage

- **Path:** `%APPDATA%\scarllet\session.json`
- **Atomic write:** temp-file-then-rename (`session.tmp` → `session.json`)
- **Corruption handling:** `serde_json` parse failure → `tracing::warn!` → start fresh
- **Folder missing:** `std::fs::create_dir_all()` called before write

### Session ↔ App Mapping

| Session field | App field |
|---------------|-----------|
| `session.id` | `app.session_id` |
| `session.messages` | `app.messages` (filtered to User + completed Agent only) |
| `session.created_at` | Not stored in App |
| `session.updated_at` | Set by `session_repo.save()` |

### Save Triggers

| Trigger | Location |
|---------|----------|
| User submits prompt | `events::handle_input()` |
| Agent response done | `events::handle_core_event()` — `AgentResponse` arm |
| CTRL+N | `main.rs` event loop (not inside `handle_input`) |
| TUI exit | `main.rs` event loop, before `should_exit` break |

### Effort Files

- `efforts/01-scaffold-session-rs.md` — Phase 1: scaffold `session.rs`
- `efforts/02-integrate-into-app.md` — Phase 2: integrate into `App`
- `efforts/03-hook-save-triggers.md` — Phase 3: hook save triggers in `events.rs`
- `efforts/04-wire-in-main.md` — Phase 4: wire in `main.rs`

### Verification

```powershell
cd packages/rust/scarllet-tui
cargo test --lib
cargo check
```
