---
title: Integrate session persistence into App
phase: 2
status: done
depends_on: 01-scaffold-session-rs
---

## Objective

Modify `packages/rust/scarllet-tui/src/app.rs` to:
1. Add `session_repo: Arc<dyn SessionRepository>` field
2. Add `save_session()` method
3. Add `new_session()` method  
4. Add `load_from_session()` method (restores messages from loaded session)
5. Update `App::new()` signature to accept `session_repo`

## Files

### Modified: `packages/rust/scarllet-tui/src/app.rs`

```rust
// Add to imports
use std::sync::Arc;
use uuid::Uuid;

// Add to App struct
pub(crate) session_repo: Arc<dyn session::SessionRepository>,
pub(crate) session_id: String,

// Add to App::new() parameters
session_repo: Arc<dyn session::SessionRepository>,

// Add methods to App impl:

/// Persists the current session to disk.
/// Silently logs and continues on failure.
pub(crate) fn save_session(&self) {
    let messages: Vec<session::SessionMessage> = self
        .messages
        .iter()
        .filter_map(|e| match e {
            ChatEntry::User { text } => Some(session::SessionMessage {
                id: Uuid::new_v4().to_string(),
                role: session::MessageRole::User,
                content: text.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            }),
            ChatEntry::Agent { blocks, done, .. } if *done => {
                // Only persist completed agent responses
                let content = blocks
                    .iter()
                    .filter_map(|b| match b {
                        DisplayBlock::Text(t) | DisplayBlock::Thought(t) => {
                            Some(t.clone())
                        }
                        DisplayBlock::ToolCallRef(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(session::SessionMessage {
                    id: Uuid::new_v4().to_string(),
                    role: session::MessageRole::Assistant,
                    content,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
            }
            _ => None,
        })
        .collect();

    let session = session::Session {
        id: self.session_id.clone(),
        created_at: String::new(), // Keep existing or load from repo
        updated_at: chrono::Utc::now().to_rfc3339(),
        messages,
    };

    if let Err(e) = self.session_repo.save(&session) {
        tracing::warn!("Failed to save session: {e}");
    }
}

/// Creates a new empty session, clearing the chat history.
pub(crate) fn new_session(&mut self) {
    self.session_id = Uuid::new_v4().to_string();
    self.messages.clear();
    self.scroll_view_state = crate::widgets::ScrollViewState::new();
    self.focused_message_idx = None;
}

/// Loads chat messages from a restored session into App state.
pub(crate) fn load_from_session(&mut self, session: session::Session) {
    self.session_id = session.id;
    self.messages = session
        .messages
        .into_iter()
        .map(|m| match m.role {
            session::MessageRole::User => ChatEntry::User {
                text: m.content,
            },
            session::MessageRole::Assistant => ChatEntry::Agent {
                name: String::new(),
                task_id: String::new(),
                blocks: vec![DisplayBlock::Text(m.content)],
                visible_chars: m.content.chars().count(),
                done: true,
            },
        })
        .collect();
}
```

## Verification

```powershell
cd packages/rust/scarllet-tui
cargo check
cargo test --lib
```
