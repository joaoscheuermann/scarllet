//! Per-session [`SessionDiff`] envelope builders.
//!
//! Every mutation on a [`Session`] ends by building exactly one of these
//! diffs and broadcasting it via [`Session::broadcast`]. Keeping the
//! builders out of the mutating call sites avoids scattering proto
//! construction throughout the dispatch / stream layers.

use scarllet_proto::proto::{
    session_diff, AgentRegistered, AgentUnregistered, Attached, Node, NodeCreated, NodePatch,
    NodeUpdated, QueueChanged, QueuedPrompt, SessionDestroyed, SessionDiff, SessionState,
    StatusChanged,
};

use super::{Session, SessionStatus};

/// Wraps a `SessionDiff::Payload` into the outer envelope.
fn wrap(payload: session_diff::Payload) -> SessionDiff {
    SessionDiff {
        payload: Some(payload),
    }
}

/// Builds the first-message hydration diff sent to a freshly attached TUI.
pub fn attached(state: SessionState) -> SessionDiff {
    wrap(session_diff::Payload::Attached(Attached {
        state: Some(state),
    }))
}

/// Builds a `NodeCreated` diff for a freshly inserted node.
pub fn node_created(node: Node) -> SessionDiff {
    wrap(session_diff::Payload::NodeCreated(NodeCreated {
        node: Some(node),
    }))
}

/// Builds a `NodeUpdated` diff carrying a partial-patch applied to an
/// already-stored node. `updated_at` should match the timestamp the
/// [`super::nodes::NodeStore::update`] call recorded on the node so the
/// snapshot and the diff agree.
pub fn node_updated(node_id: String, patch: NodePatch, updated_at: u64) -> SessionDiff {
    wrap(session_diff::Payload::NodeUpdated(NodeUpdated {
        node_id,
        patch: Some(patch),
        updated_at,
    }))
}

/// Builds a `NodeUpdated` diff and fans it out to every subscriber on
/// `session`. Pairs with [`super::nodes::NodeStore::update`] so callers
/// always send the same `updated_at` they wrote into the store.
pub fn broadcast_node_updated(
    session: &mut Session,
    node_id: String,
    patch: NodePatch,
    updated_at: u64,
) {
    session.broadcast(node_updated(node_id, patch, updated_at));
}

/// Builds a `QueueChanged` diff carrying the current full queue snapshot.
pub fn queue_changed(queued: Vec<QueuedPrompt>) -> SessionDiff {
    wrap(session_diff::Payload::QueueChanged(QueueChanged { queued }))
}

/// Snapshots `session.queue` and broadcasts the resulting `QueueChanged`
/// diff to every connected subscriber.
///
/// Centralises the "snapshot + broadcast" pattern so every enqueue / pop
/// path emits the same diff shape (full queue snapshot per the spec, not
/// an incremental delta).
pub fn broadcast_queue_changed(session: &mut Session) {
    let snapshot = session.queue.snapshot();
    session.broadcast(queue_changed(snapshot));
}

/// Builds an `AgentRegistered` diff for a newly accepted agent stream.
pub fn agent_registered(
    agent_id: String,
    agent_module: String,
    parent_id: String,
    agent_node_id: String,
) -> SessionDiff {
    wrap(session_diff::Payload::AgentRegistered(AgentRegistered {
        agent_id,
        agent_module,
        parent_id,
        agent_node_id,
    }))
}

/// Builds an `AgentUnregistered` diff for a deregistered agent.
pub fn agent_unregistered(agent_id: String) -> SessionDiff {
    wrap(session_diff::Payload::AgentUnregistered(AgentUnregistered {
        agent_id,
    }))
}

/// Builds a `StatusChanged` diff translating [`SessionStatus`] to its
/// canonical wire string.
pub fn status_changed(status: SessionStatus) -> SessionDiff {
    wrap(session_diff::Payload::StatusChanged(StatusChanged {
        status: status_str(status).to_string(),
    }))
}

/// Broadcasts a `StatusChanged` diff carrying `session.status` to every
/// attached subscriber. Pair with [`super::Session::set_status`] — only
/// call this when `set_status` returned `true` so attached TUIs do not
/// receive idempotent no-op transitions.
pub fn broadcast_status_changed(session: &mut Session) {
    let diff = status_changed(session.status);
    session.broadcast(diff);
}

/// Builds a `SessionDestroyed` terminal diff.
pub fn destroyed(session_id: String) -> SessionDiff {
    wrap(session_diff::Payload::Destroyed(SessionDestroyed {
        session_id,
    }))
}

/// Translates a [`SessionStatus`] to the canonical wire string.
pub fn status_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "RUNNING",
        SessionStatus::Paused => "PAUSED",
    }
}
