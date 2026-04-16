---
title: Wire session persistence in main.rs
phase: 4
status: done
depends_on: 03-hook-save-triggers
---

## Objective

Modify `packages/rust/scarllet-tui/src/main.rs` to:
1. Add `mod session` declaration
2. Construct `FileSessionRepository` on startup
3. Load session from disk before entering the event loop
4. Inject `Arc<dyn SessionRepository>` into `App::new()`
5. Save session on exit (before terminal restore)
6. Handle CTRL+N shortcut at the loop level (not inside `handle_input` to avoid double-save)

## Files

### Modified: `packages/rust/scarllet-tui/src/main.rs`

**1. Add module declaration:**
```rust
mod app;
mod connection;
mod events;
mod git_info;
mod input;
mod render;
mod session;  // ← ADD THIS
mod widgets;
```

**2. Construct repository and load session before event loop:**
```rust
// After App::new() construction, before the main loop:
// Load persisted session
let session_repo = match session::FileSessionRepository::new() {
    Ok(repo) => Arc::new(repo),
    Err(e) => {
        tracing::warn!("Could not create session repository: {e}. Continuing without persistence.");
        Arc::new(session::NullSessionRepository) as Arc<dyn session::SessionRepository>
    }
};

if let Ok(Some(s)) = session_repo.load() {
    app.load_from_session(s);
}
```

**3. Add `NullSessionRepository` stub to `session.rs`:**
```rust
/// A no-op repository used when the file system is unavailable.
pub struct NullSessionRepository;

impl SessionRepository for NullSessionRepository {
    fn save(&self, _: &Session) -> io::Result<()> { Ok(()) }
    fn load(&self) -> io::Result<Option<Session>> { Ok(None) }
    fn session_path(&self) -> PathBuf { PathBuf::new() }
}
```

**4. Save on exit:**
```rust
// In the exit block, before ratatui::restore():
if should_exit {
    app.save_session();  // ← ADD THIS
    break;
}
```

**5. Handle CTRL+N at loop level:**
```rust
// In the event loop, after processing a Key event,
// check for CTRL+N before calling handle_input:
Event::Key(key) => {
    if key.kind != crossterm::event::KeyEventKind::Press {
        continue;
    }
    if key.code == KeyCode::Char('n')
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        app.save_session();
        app.new_session();
        continue;
    }
    if events::handle_input(&mut app, key) {
        should_exit = true;
        break;
    }
}
```

> **Note:** CTRL+N is handled at the **loop level**, not inside `handle_input`. This is intentional — it avoids the complexity of `handle_input` needing to signal a new-session event, and keeps the save-then-clear atomic at the call site.

## Verification

```powershell
cd packages/rust/scarllet-tui
cargo build
cargo test --lib
cargo run
```

**Manual test:**
1. Run TUI
2. Send a message → confirm `session.json` appears in `%APPDATA%\scarllet\`
3. Close TUI
4. Reopen TUI → confirm previous messages are displayed
5. Press CTRL+N → confirm chat clears and new session starts
