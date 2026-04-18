//! Per-session queue routing + main-agent dispatch.
//!
//! `try_dispatch_main` pops the queue head, creates the turn's `Agent`
//! node, and spawns the configured default agent process. Idempotent —
//! safe to call after every queue mutation.

use std::path::PathBuf;
use std::sync::Arc;

use scarllet_proto::proto::{node, AgentPayload, ErrorPayload, Node, NodeKind, QueuedPrompt};
use scarllet_sdk::manifest::ModuleKind;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

use crate::registry::ModuleRegistry;
use crate::session::{diff, Session, SessionStatus};

use super::spawn;

/// Arguments handed to the spawn callback used by [`try_dispatch_main_with`].
///
/// Bundling the spawn parameters into a single struct lets unit tests
/// observe every dispatch attempt without having to mirror the full
/// `Command::spawn` argument list.
#[derive(Debug, Clone)]
pub struct SpawnArgs<'a> {
    /// On-disk path to the agent binary.
    pub module_path: &'a std::path::Path,
    /// Loopback gRPC address the spawned process should dial.
    pub core_addr: &'a str,
    /// Owning session id (becomes `SCARLLET_SESSION_ID`).
    pub session_id: &'a str,
    /// Core-assigned id for this specific agent process (`SCARLLET_AGENT_ID`).
    pub agent_id: &'a str,
    /// Manifest name of the module being spawned (`SCARLLET_AGENT_MODULE`).
    pub agent_module: &'a str,
    /// Working directory the prompt was issued from.
    pub working_directory: &'a str,
}

/// Attempts to dispatch the next queued prompt to a freshly spawned main
/// agent. Idempotent — the routing function early-returns when the session
/// is paused, already has a running main agent, or the queue is empty.
///
/// Mutates `session` (queue + nodes + agents + last_activity) and broadcasts
/// every diff produced along the way. Spawns the real agent process via
/// [`spawn::spawn_main_agent`].
pub async fn try_dispatch_main(
    session: &mut Session,
    registry: &Arc<RwLock<ModuleRegistry>>,
    core_addr: &str,
) {
    try_dispatch_main_with(session, registry, core_addr, |args| {
        spawn::spawn_main_agent(
            args.module_path,
            args.core_addr,
            args.session_id,
            args.agent_id,
            args.agent_module,
            args.working_directory,
        )
    })
    .await;
}

/// Test seam for [`try_dispatch_main`]: same control flow but the spawn
/// step is delegated to `spawn_fn`. Production callers always go through
/// the public [`try_dispatch_main`] which forwards to the real
/// [`spawn::spawn_main_agent`].
///
/// The closure receives a [`SpawnArgs`] borrow valid for the call only;
/// returning `None` is treated as a "best-effort spawn" identical to the
/// real spawn helper failing.
pub async fn try_dispatch_main_with<F>(
    session: &mut Session,
    registry: &Arc<RwLock<ModuleRegistry>>,
    core_addr: &str,
    spawn_fn: F,
) where
    F: FnOnce(SpawnArgs<'_>) -> Option<u32>,
{
    if session.status != SessionStatus::Running {
        return;
    }
    if session.agents.has_main() {
        return;
    }
    if session.queue.is_empty() {
        return;
    }

    let module_name = session.config.default_agent.clone();
    if module_name.is_empty() {
        warn!("Default agent module is not configured; popping prompt and emitting Error node");
        let _ = session.queue.pop_front();
        diff::broadcast_queue_changed(session);
        emit_dispatch_error(session, "default_agent not configured");
        return;
    }

    let module_path = resolve_agent_module(registry, &module_name).await;
    let Some(module_path) = module_path else {
        warn!("Default agent module '{module_name}' is not registered");
        let _ = session.queue.pop_front();
        diff::broadcast_queue_changed(session);
        emit_dispatch_error(
            session,
            &format!("agent module '{module_name}' is not registered"),
        );
        return;
    };

    let prompt = session
        .queue
        .pop_front()
        .expect("queue non-empty checked above");
    diff::broadcast_queue_changed(session);

    let agent_id = Uuid::new_v4().to_string();
    let agent_node = build_agent_node(&agent_id, &module_name);
    let stored = session
        .nodes
        .create(agent_node)
        .expect("agent node invariants always hold")
        .clone();
    session.broadcast(diff::node_created(stored));

    let pid = spawn_fn(SpawnArgs {
        module_path: &module_path,
        core_addr,
        session_id: &session.id,
        agent_id: &agent_id,
        agent_module: &module_name,
        working_directory: &prompt.working_directory,
    });

    enqueue_pending_dispatch(session, agent_id, prompt, pid);
}

/// Looks up the on-disk path of the agent module named `module_name`, if
/// it is currently registered as a [`ModuleKind::Agent`].
async fn resolve_agent_module(
    registry: &Arc<RwLock<ModuleRegistry>>,
    module_name: &str,
) -> Option<PathBuf> {
    let reg = registry.read().await;
    reg.by_kind(ModuleKind::Agent)
        .into_iter()
        .find(|(_, m)| m.name == module_name)
        .map(|(p, _)| p.clone())
}

/// Records the pending dispatch on the session so the bidi `AgentStream`
/// handler can match the agent's `Register` message back to the right
/// `Agent` node + queued prompt.
fn enqueue_pending_dispatch(
    session: &mut Session,
    agent_id: String,
    prompt: QueuedPrompt,
    pid: Option<u32>,
) {
    session
        .pending_dispatch
        .insert(agent_id, PendingDispatch { prompt, pid });
}

/// Carrier for the prompt that is in flight between `try_dispatch_main`
/// and the agent's `Register` handler. Stored on the `Session` because the
/// dispatch is per-session.
pub struct PendingDispatch {
    pub prompt: QueuedPrompt,
    /// OS PID of the spawned agent process, when the spawn call returned
    /// one. Propagated onto the [`crate::agents::AgentRecord`] during
    /// register so the AC-8.4 cascade can force-kill the process.
    pub pid: Option<u32>,
}

/// Emits a top-level `Error` node when dispatch fails because the
/// configured default agent module is not registered (AC-3.3) or
/// `default_agent` is empty.
fn emit_dispatch_error(session: &mut Session, message: &str) {
    let id = Uuid::new_v4().to_string();
    let node = Node {
        id: id.clone(),
        parent_id: None,
        kind: NodeKind::Error as i32,
        created_at: now_secs(),
        updated_at: now_secs(),
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "core".into(),
            message: message.into(),
        })),
    };
    let stored = session
        .nodes
        .create(node)
        .expect("top-level error invariants hold")
        .clone();
    session.broadcast(diff::node_created(stored));
}

/// Constructs the `Agent` node that wraps a freshly dispatched main turn.
fn build_agent_node(agent_id: &str, module_name: &str) -> Node {
    let now = now_secs();
    Node {
        id: agent_id.to_string(),
        parent_id: None,
        kind: NodeKind::Agent as i32,
        created_at: now,
        updated_at: now,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: module_name.to_string(),
            agent_id: agent_id.to_string(),
            status: "running".to_string(),
        })),
    }
}

/// Returns the current Unix-epoch second count, saturating on errors.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
