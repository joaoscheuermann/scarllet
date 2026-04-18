//! Session lifecycle + TUI-facing RPC handlers.
//!
//! Implements `CreateSession`, `ListSessions`, `DestroySession`,
//! `GetSessionState`, `AttachSession` (first-diff hydration),
//! `SendPrompt`, and `StopSession`. The diff broadcast is owned here;
//! business rules (queue / dispatch) are delegated to `agents::routing`.

use scarllet_proto::proto::*;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;
use uuid::Uuid;

use crate::agents::routing;
use crate::agents::stream::{cascade_cancel, CANCEL_REASON};
use crate::session::{diff, state, Session, SessionStatus};

use super::OrchestratorService;

/// Channel buffer for `AttachSession` streams. Large enough to absorb the
/// initial `Attached` payload plus a burst of node creates without blocking
/// the per-session broadcast loop.
const SUBSCRIBER_BUFFER: usize = 256;

/// `CreateSession` — allocates a fresh session id and returns it.
pub async fn create_session(
    svc: &OrchestratorService,
    _req: Request<CreateSessionRequest>,
) -> Result<Response<CreateSessionResponse>, Status> {
    let cfg = svc.config.read().await.clone();
    let mut sessions = svc.sessions.write().await;
    let session_id = sessions.create_session(&cfg);
    info!("Created session {session_id}");
    Ok(Response::new(CreateSessionResponse { session_id }))
}

/// `ListSessions` — returns one [`SessionSummary`] per active session.
pub async fn list_sessions(
    svc: &OrchestratorService,
    _req: Request<ListSessionsRequest>,
) -> Result<Response<ListSessionsResponse>, Status> {
    let sessions = svc.sessions.read().await;
    let mut summaries = Vec::with_capacity(sessions.len());
    for (id, handle) in sessions.iter() {
        let session = handle.read().await;
        summaries.push(SessionSummary {
            session_id: id.clone(),
            created_at: to_unix_secs(session.created_at),
            last_activity: to_unix_secs(session.last_activity),
            main_agent_module: session
                .agents
                .iter_records()
                .find(|r| r.parent_id == *id)
                .map(|r| r.agent_module.clone())
                .unwrap_or_default(),
        });
    }
    Ok(Response::new(ListSessionsResponse {
        sessions: summaries,
    }))
}

/// `DestroySession` — drops the session and broadcasts a terminal diff.
pub async fn destroy_session(
    svc: &OrchestratorService,
    req: Request<DestroySessionRequest>,
) -> Result<Response<DestroySessionResponse>, Status> {
    let session_id = req.into_inner().session_id;
    destroy_session_inner(svc, &session_id).await;
    Ok(Response::new(DestroySessionResponse {}))
}

/// Internal helper used by both `DestroySession` RPC and the auto-cleanup
/// path when the last subscriber disconnects.
///
/// Cascades agent kills (sub-agents first, main agent last) so every
/// running process gets a `CancelNow` + best-effort PID kill after the
/// grace period before the session itself is dropped from the registry.
/// After the cascade we broadcast a terminal `SessionDestroyed` diff so
/// attached TUIs know to clean up.
pub(crate) async fn destroy_session_inner(svc: &OrchestratorService, session_id: &str) {
    let removed = {
        let mut sessions = svc.sessions.write().await;
        sessions.destroy_session(session_id)
    };
    let Some(handle) = removed else {
        return;
    };
    let mut session = handle.write().await;
    cascade_cancel(&mut session, CANCEL_REASON);
    session.queue.clear();
    session.pending_dispatch.clear();
    session.broadcast(diff::destroyed(session_id.to_string()));
    info!("Destroyed session {session_id}");
}

/// `GetSessionState` — returns the full snapshot for a session.
pub async fn get_session_state(
    svc: &OrchestratorService,
    req: Request<GetSessionStateRequest>,
) -> Result<Response<SessionState>, Status> {
    let session_id = req.into_inner().session_id;
    let handle = lookup_session(svc, &session_id).await?;
    let session = handle.read().await;
    Ok(Response::new(state::snapshot(&session)))
}

/// `AttachSession` — registers a new subscriber and immediately sends the
/// `Attached` first diff.
///
/// If `session_id` is empty a fresh session is created (AC-1.2).
pub async fn attach_session(
    svc: &OrchestratorService,
    req: Request<AttachSessionRequest>,
) -> Result<
    Response<<crate::service::OrchestratorService as scarllet_proto::proto::orchestrator_server::Orchestrator>::AttachSessionStream>,
    Status,
> {
    let session_id = match req.into_inner().session_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            let cfg = svc.config.read().await.clone();
            let mut sessions = svc.sessions.write().await;
            let id = sessions.create_session(&cfg);
            info!("Auto-created session {id} on attach");
            id
        }
    };

    let handle = lookup_session(svc, &session_id).await?;
    let (tx, rx) = mpsc::channel(SUBSCRIBER_BUFFER);

    {
        let mut session = handle.write().await;
        session.subscribers.push(tx.clone());
        let snapshot = state::snapshot(&session);
        if tx.try_send(Ok(diff::attached(snapshot))).is_err() {
            return Err(Status::internal(
                "subscriber channel closed before Attached diff could be sent",
            ));
        }
    }

    Ok(Response::new(ReceiverStream::new(rx)))
}

/// `SendPrompt` — appends a `User` node, queues the prompt, and triggers
/// `try_dispatch_main`. Returns the new user node id.
pub async fn send_prompt(
    svc: &OrchestratorService,
    req: Request<SendPromptRequest>,
) -> Result<Response<SendPromptResponse>, Status> {
    let SendPromptRequest {
        session_id,
        text,
        working_directory,
    } = req.into_inner();

    if text.is_empty() {
        return Err(Status::invalid_argument("text must not be empty"));
    }

    let handle = lookup_session(svc, &session_id).await?;
    let mut session = handle.write().await;

    let user_node_id = Uuid::new_v4().to_string();
    let now = now_secs();
    let user_node = Node {
        id: user_node_id.clone(),
        parent_id: None,
        kind: NodeKind::User as i32,
        created_at: now,
        updated_at: now,
        payload: Some(node::Payload::User(UserPayload {
            text: text.clone(),
            working_directory: working_directory.clone(),
        })),
    };
    let stored = session
        .nodes
        .create(user_node)
        .map_err(|err| Status::internal(format!("invalid User node: {err:?}")))?
        .clone();
    session.broadcast(diff::node_created(stored));

    let prompt_id = Uuid::new_v4().to_string();
    let queued = QueuedPrompt {
        prompt_id,
        text,
        working_directory,
        user_node_id: user_node_id.clone(),
    };
    session.queue.push_back(queued);
    diff::broadcast_queue_changed(&mut session);

    routing::try_dispatch_main(&mut session, &svc.registry, &svc.bound_addr).await;

    Ok(Response::new(SendPromptResponse { user_node_id }))
}

/// `StopSession` — cascading cancellation of every running agent, queue
/// clear, and `Paused → Running` recovery.
///
/// Flow (effort 06):
/// 1. Cascade through every registered agent in reverse topological order
///    via `cascade_cancel` — sub-agents die first, then their parent(s),
///    then the main agent. Each gets a `CancelNow` + grace-period kill,
///    has its Agent node patched to `status="failed"`, has an `Error`
///    child node `"cancelled by user"` appended, and is deregistered.
/// 2. Any `sub_agent_waiters` registered for the cascaded agents fire
///    with `Err("cancelled by user")` so parent `InvokeTool` calls
///    unblock cleanly.
/// 3. Drop every queued prompt + pending dispatch; broadcast one empty
///    `QueueChanged` so attached TUIs clear their indicators.
/// 4. If the session was `Paused`, transition back to `Running`
///    (AC-3.4 recovery) and broadcast `StatusChanged`.
pub async fn stop_session(
    svc: &OrchestratorService,
    req: Request<StopSessionRequest>,
) -> Result<Response<StopSessionResponse>, Status> {
    let session_id = req.into_inner().session_id;
    let handle = lookup_session(svc, &session_id).await?;
    let mut session = handle.write().await;

    cascade_cancel(&mut session, CANCEL_REASON);

    session.queue.clear();
    session.pending_dispatch.clear();
    diff::broadcast_queue_changed(&mut session);

    if session.set_status(SessionStatus::Running) {
        diff::broadcast_status_changed(&mut session);
    }

    Ok(Response::new(StopSessionResponse {}))
}

/// Looks up a session by id, returning `Status::not_found` if missing.
pub(crate) async fn lookup_session(
    svc: &OrchestratorService,
    session_id: &str,
) -> Result<std::sync::Arc<tokio::sync::RwLock<Session>>, Status> {
    let sessions = svc.sessions.read().await;
    sessions
        .get(session_id)
        .ok_or_else(|| Status::not_found(format!("Session '{session_id}' not found")))
}

/// Converts a [`std::time::SystemTime`] to seconds since the Unix epoch.
fn to_unix_secs(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Returns the current Unix-epoch second count, saturating on errors.
fn now_secs() -> u64 {
    to_unix_secs(std::time::SystemTime::now())
}
