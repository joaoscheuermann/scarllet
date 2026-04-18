//! Per-session state: node graph, queue, config snapshot, subscribers.
//!
//! Pure data with mutation helpers — no IO, no gRPC. Owned by the
//! [`SessionRegistry`] behind an `Arc<RwLock<_>>` so every gRPC handler
//! can borrow a single session without blocking unrelated ones.

/// Per-session diff envelope builders.
pub mod diff;
/// Append-only typed-node graph for a single session.
pub mod nodes;
/// FIFO queue of pending user prompts for one session.
pub mod queue;
/// Snapshot builder for the `Attached` first diff and `GetSessionState`.
pub mod state;
/// Subscriber set used by `Session` to fan out diffs to attached TUIs.
pub mod subscribers;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use scarllet_proto::proto::SessionDiff;
use scarllet_sdk::config::{Provider, ScarlletConfig};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::agents::routing::PendingDispatch;
use crate::agents::AgentRegistry;
use nodes::NodeStore;
use queue::SessionQueue;
use subscribers::SubscriberSet;

/// High-level lifecycle state of a single session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Default state — main agent dispatch is allowed.
    Running,
    /// Mid-turn agent failure has paused dispatch (AC-3.4). Cleared by
    /// `StopSession`.
    Paused,
}

/// Per-session snapshot of the global LLM config taken at create-time.
///
/// AC-9.1 / AC-9.2: a session's effective provider is fixed when it is
/// created; later global config reloads do not retroactively change it.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Active provider snapshot (None when no provider is configured).
    pub provider: Option<Provider>,
    /// Name of the agent module main turns will spawn.
    pub default_agent: String,
}

impl SessionConfig {
    /// Captures the relevant fields of `cfg` at the moment of session creation.
    pub fn from_global(cfg: &ScarlletConfig) -> Self {
        Self {
            provider: cfg.active_provider().cloned(),
            default_agent: cfg.default_agent.clone(),
        }
    }
}

/// One isolated chat session: queue, node graph, agents, subscribers.
///
/// Held inside `Arc<RwLock<Session>>` and shared by every gRPC handler that
/// needs to observe or mutate the session.
pub struct Session {
    /// Stable identifier for the session (UUID-v4).
    pub id: String,
    /// Wall-clock time the session was created.
    pub created_at: SystemTime,
    /// Wall-clock time of the last mutation (prompt, agent event, …).
    pub last_activity: SystemTime,
    /// Lifecycle state — see [`SessionStatus`].
    pub status: SessionStatus,
    /// Snapshot of the global config at session-create time.
    pub config: SessionConfig,
    /// FIFO of pending user prompts.
    pub queue: SessionQueue,
    /// Append-only typed-node graph backing `Messages`.
    pub nodes: NodeStore,
    /// Per-session agent registry (main + sub-agents).
    pub agents: AgentRegistry,
    /// Prompts that have been popped from the queue and routed to a freshly
    /// spawned agent process; the matching agent's `Register` handler
    /// removes the entry and dispatches the prompt as an `AgentTask`.
    pub pending_dispatch: HashMap<String, PendingDispatch>,
    /// Connected TUI subscribers receiving `SessionDiff`s.
    pub subscribers: SubscriberSet<SessionDiff>,
}

impl Session {
    /// Builds a fresh session with empty queue / nodes / agents / subscribers.
    pub fn new(id: String, config: SessionConfig) -> Self {
        let now = SystemTime::now();
        Self {
            id,
            created_at: now,
            last_activity: now,
            status: SessionStatus::Running,
            config,
            queue: SessionQueue::new(),
            nodes: NodeStore::new(),
            agents: AgentRegistry::new(),
            pending_dispatch: HashMap::new(),
            subscribers: SubscriberSet::new(),
        }
    }

    /// Marks the session as having just been mutated, updating `last_activity`.
    pub fn touch(&mut self) {
        self.last_activity = SystemTime::now();
    }

    /// Sends `diff` to every attached subscriber, pruning closed senders.
    ///
    /// Convenience wrapper that touches `last_activity` and then delegates
    /// to [`SubscriberSet::broadcast`].
    pub fn broadcast(&mut self, diff: SessionDiff) {
        self.touch();
        self.subscribers.broadcast(diff);
    }

    /// Transitions the session lifecycle to `new_status`, returning `true`
    /// when a transition actually happened. Idempotent — repeated calls
    /// with the same value are a no-op and return `false`, so callers can
    /// safely invoke this helper without worrying about spamming
    /// `StatusChanged` broadcasts.
    ///
    /// Callers are responsible for broadcasting the diff when `true` is
    /// returned (typically via [`diff::broadcast_status_changed`]).
    pub fn set_status(&mut self, new_status: SessionStatus) -> bool {
        if self.status == new_status {
            return false;
        }
        self.status = new_status;
        self.touch();
        true
    }
}

/// Process-wide collection of every active session.
///
/// Wrapped in `Arc<RwLock<…>>` once at the top level so the gRPC handlers
/// can hand out per-session `Arc<RwLock<Session>>` clones without holding
/// the registry lock for the duration of a session-scoped operation.
pub struct SessionRegistry {
    sessions: HashMap<String, Arc<RwLock<Session>>>,
}

impl Default for SessionRegistry {
    /// Empty registry; identical to [`SessionRegistry::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    /// Initialises an empty registry.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Allocates a fresh session id, snapshots the global config, and stores
    /// the new session. Returns the generated id.
    pub fn create_session(&mut self, cfg: &ScarlletConfig) -> String {
        let id = Uuid::new_v4().to_string();
        let config = SessionConfig::from_global(cfg);
        let session = Session::new(id.clone(), config);
        self.sessions
            .insert(id.clone(), Arc::new(RwLock::new(session)));
        id
    }

    /// Removes a session from the registry, returning the dropped handle so
    /// callers can broadcast the terminal `Destroyed` diff before the lock
    /// is released.
    pub fn destroy_session(&mut self, id: &str) -> Option<Arc<RwLock<Session>>> {
        self.sessions.remove(id)
    }

    /// Returns the session handle if it exists.
    pub fn get(&self, id: &str) -> Option<Arc<RwLock<Session>>> {
        self.sessions.get(id).map(Arc::clone)
    }

    /// Returns every session handle (used by `ListSessions`).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<RwLock<Session>>)> {
        self.sessions.iter()
    }

    /// Number of currently-tracked sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// `true` when no sessions are tracked.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests;
