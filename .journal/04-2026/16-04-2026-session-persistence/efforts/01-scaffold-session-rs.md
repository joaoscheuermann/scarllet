---
title: Scaffold session.rs
phase: 1
status: done
---

## Change Summary

**Files created:**
- `packages/rust/scarllet-tui/src/session.rs` — data models, `SessionRepository` trait, `FileSessionRepository`, `NullSessionRepository`, helpers, unit tests

**Files modified:**
- `packages/rust/scarllet-tui/Cargo.toml` — added `serde`, `serde_json`, `tracing`, `uuid` dependencies
- `packages/rust/scarllet-tui/src/main.rs` — added `mod session` declaration

**Key decisions:**
- Used `#[allow(dead_code)]` on `SessionRepository`, `FileSessionRepository`, and `NullSessionRepository` to suppress warnings until phases 2-4 wire them in
- `FileSessionRepository::new()` and `session_file()` will be used in phase 4, so the warning is expected

**Deviations:**
- The effort referenced `lib.rs` for the module declaration, but `scarllet-tui` is a binary crate with no `lib.rs`. Added `mod session` directly to `main.rs` instead.

## Objective

Create `packages/rust/scarllet-tui/src/session.rs` with:
- `Session` and `SessionMessage` data models
- `SessionRepository` trait (DIP)
- `FileSessionRepository` implementation with atomic write (temp-file-then-rename)
- Unit tests

## Files

### Modified: `packages/rust/scarllet-tui/Cargo.toml`

Add dependencies:
```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
uuid = { version = "1", features = ["v4"] }
```

### New: `packages/rust/scarllet-tui/src/session.rs`

```rust
use std::io;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────
// Data models
// ─────────────────────────────────────────────────────────────

/// Discriminator for who sent a message in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// A single persisted message in the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

/// The top-level session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<SessionMessage>,
}

// ─────────────────────────────────────────────────────────────
// SessionRepository trait (DIP)
// ─────────────────────────────────────────────────────────────

/// Abstracts session persistence so callers do not couple to file I/O.
pub trait SessionRepository: Send + Sync {
    /// Persists the session to storage. Returns Ok(()) on success.
    fn save(&self, session: &Session) -> io::Result<()>;

    /// Loads the session from storage.
    /// Returns Ok(Some(session)) if a valid session exists,
    /// Ok(None) if the file is absent or corrupted.
    fn load(&self) -> io::Result<Option<Session>>;

    /// Returns the path where sessions are stored.
    fn session_path(&self) -> PathBuf;
}

// ─────────────────────────────────────────────────────────────
// FileSessionRepository
// ─────────────────────────────────────────────────────────────

/// Implements `SessionRepository` using a single JSON file on disk.
/// Writes are atomic (temp-file-then-rename) to avoid partial writes.
pub struct FileSessionRepository {
    base: PathBuf,
}

impl FileSessionRepository {
    /// Creates a new repository rooted at the platform APPDATA/scarllet directory.
    pub fn new() -> io::Result<Self> {
        let base = dirs::config_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine OS config directory"))?
            .join("scarllet");
        Ok(Self { base })
    }

    /// Returns the path to the session JSON file.
    fn session_file(&self) -> PathBuf {
        self.base.join("session.json")
    }
}

impl SessionRepository for FileSessionRepository {
    fn session_path(&self) -> PathBuf {
        self.session_file()
    }

    fn save(&self, session: &Session) -> io::Result<()> {
        std::fs::create_dir_all(&self.base)?;

        let tmp = self.base.join("session.tmp");
        let json = serde_json::to_string_pretty(session)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, self.session_file())?;

        Ok(())
    }

    fn load(&self) -> io::Result<Option<Session>> {
        let path = self.session_file();
        if !path.exists() {
            return Ok(None);
        }

        let contents = std::fs::read_to_string(&path)?;
        match serde_json::from_str(&contents) {
            Ok(session) => Ok(Some(session)),
            Err(e) => {
                tracing::warn!("Corrupted session.json, starting fresh: {e}");
                Ok(None)
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────

/// Creates a fresh session with the given ID.
pub fn new_session(id: String) -> Session {
    let now = chrono::Utc::now().to_rfc3339();
    Session {
        id,
        created_at: now.clone(),
        updated_at: now,
        messages: Vec::new(),
    }
}

/// Updates the session's updated_at and appends a message.
pub fn append_message(session: &mut Session, role: MessageRole, content: String) {
    session.updated_at = chrono::Utc::now().to_rfc3339();
    session.messages.push(SessionMessage {
        id: Uuid::new_v4().to_string(),
        role,
        content,
        timestamp: chrono::Utc::now().to_rfc3339(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_has_no_messages() {
        let s = new_session("test-id".into());
        assert!(s.messages.is_empty());
        assert_eq!(s.id, "test-id");
    }

    #[test]
    fn append_message_updates_timestamp() {
        let mut s = new_session("id".into());
        append_message(&mut s, MessageRole::User, "Hello".into());
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "Hello");
        assert_eq!(s.messages[0].role, MessageRole::User);
        assert!(s.updated_at >= s.created_at);
    }

    #[test]
    fn session_serialize_roundtrip() {
        let mut s = new_session("roundtrip".into());
        append_message(&mut s, MessageRole::User, "hi".into());
        append_message(&mut s, MessageRole::Assistant, "hello".into());

        let json = serde_json::to_string_pretty(&s).unwrap();
        let loaded: Session = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.id, s.id);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role, MessageRole::User);
        assert_eq!(loaded.messages[1].role, MessageRole::Assistant);
    }
}
```

## Verification

```powershell
cd packages/rust/scarllet-tui
cargo test --lib
cargo check
```
