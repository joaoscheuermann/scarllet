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

/// A serializable content block preserving thought vs text distinction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBlock {
    pub block_type: String,
    pub content: String,
}

/// A single persisted message in the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<SessionBlock>>,
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
//
// Used by app.rs and main.rs in subsequent phases.
// ─────────────────────────────────────────────────────────────
#[allow(dead_code)]
pub trait SessionRepository: Send + Sync {
    /// Persists the session to storage. Returns `Ok(())` on success.
    fn save(&self, session: &Session) -> io::Result<()>;

    /// Loads the session from storage.
    /// Returns `Ok(Some(session))` if a valid session exists,
    /// `Ok(None)` if the file is absent or corrupted.
    fn load(&self) -> io::Result<Option<Session>>;

    /// Returns the path where sessions are stored.
    fn session_path(&self) -> PathBuf;
}

// ─────────────────────────────────────────────────────────────
// FileSessionRepository
//
// Constructed and used in main.rs.
// ─────────────────────────────────────────────────────────────
#[allow(dead_code)]
pub struct FileSessionRepository {
    base: PathBuf,
}

impl FileSessionRepository {
    /// Creates a new repository rooted at the platform APPDATA/scarllet directory.
    pub fn new() -> io::Result<Self> {
        let base = dirs::config_dir()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot determine OS config directory",
                )
            })?
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
        let json =
            serde_json::to_string_pretty(session).map_err(|e| io::Error::new(
                io::ErrorKind::InvalidData,
                e,
            ))?;

        std::fs::write(&tmp, &json)?;
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
// NullSessionRepository
//
// Used in main.rs when FileSessionRepository cannot be constructed.
// ─────────────────────────────────────────────────────────────
#[allow(dead_code)]
pub struct NullSessionRepository;

impl SessionRepository for NullSessionRepository {
    fn save(&self, _: &Session) -> io::Result<()> {
        Ok(())
    }

    fn load(&self) -> io::Result<Option<Session>> {
        Ok(None)
    }

    fn session_path(&self) -> PathBuf {
        PathBuf::new()
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

/// Updates the session's `updated_at` and appends a message.
pub fn append_message(session: &mut Session, role: MessageRole, content: String) {
    session.updated_at = chrono::Utc::now().to_rfc3339();
    session.messages.push(SessionMessage {
        id: Uuid::new_v4().to_string(),
        role,
        content,
        timestamp: chrono::Utc::now().to_rfc3339(),
        agent_name: None,
        task_id: None,
        blocks: None,
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
