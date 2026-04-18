//! Agent-facing RPC handlers: per-turn unary queries + bidi stream.
//!
//! Exposes `GetActiveProvider`, `GetConversationHistory`, and the
//! `AgentStream` bidi endpoint. The heavy stream machinery lives in
//! [`crate::agents::stream`]; this module is thin wiring only.

use std::sync::Arc;

use scarllet_proto::proto::{agent_outbound, *};
use tokio::sync::{mpsc, RwLock};
use tonic::{Request, Response, Status};

use crate::agents::stream::{self, StreamDeps};
use crate::registry::ModuleRegistry;
use crate::session::{state, SessionRegistry};

use super::session_rpc::lookup_session;
use super::OrchestratorService;

/// `GetActiveProvider(session_id)` — returns the provider snapshot taken
/// when the session was created.
pub async fn get_active_provider(
    svc: &OrchestratorService,
    req: Request<GetActiveProviderRequest>,
) -> Result<Response<ActiveProviderResponse>, Status> {
    let session_id = req.into_inner().session_id;
    let handle = lookup_session(svc, &session_id).await?;
    let session = handle.read().await;
    Ok(Response::new(state::provider_response(
        session.config.provider.as_ref(),
    )))
}

/// `GetConversationHistory(session_id)` — derives a chronological chat
/// history from the session's node graph (top-level User → user message,
/// top-level Agent with a Result child → assistant message). Tool-call
/// history lands in effort 03.
pub async fn get_conversation_history(
    svc: &OrchestratorService,
    req: Request<GetConversationHistoryRequest>,
) -> Result<Response<ConversationHistoryResponse>, Status> {
    let session_id = req.into_inner().session_id;
    let handle = lookup_session(svc, &session_id).await?;
    let session = handle.read().await;
    let messages = state::conversation_history(&session.nodes);
    Ok(Response::new(ConversationHistoryResponse { messages }))
}

/// `AgentStream` — bidi handler for connected agents.
///
/// Mirrors the spawn-and-return-immediately contract used by
/// [`super::session_rpc::attach_session`]: this function allocates no
/// additional state, spawns the per-stream driver onto the tokio
/// runtime, and returns `Ok(())` so the caller in [`super::mod.rs`]
/// can flush the response headers.
///
/// **Why the spawn matters:** tonic's HTTP/2 client will not consider
/// the RPC "established" until the server returns `Response` (i.e. the
/// response headers are flushed). The client sits waiting for those
/// headers and therefore cannot send its first `AgentOutbound` frame.
/// If the handler body awaits `incoming.message().await` before
/// returning `Response`, both sides deadlock: server waits for client
/// data, client waits for server headers. The spawn sidesteps that
/// by making the first-message read happen *after* the handler has
/// already returned.
pub async fn agent_stream(
    svc: &OrchestratorService,
    req: Request<tonic::Streaming<AgentOutbound>>,
    out_tx: mpsc::Sender<Result<AgentInbound, Status>>,
) -> Result<(), Status> {
    let incoming = req.into_inner();

    // Snapshot the shared Arcs so the spawned task's lifetime does not
    // depend on `svc`. Clones are cheap — these are reference-counted
    // pointers over the process-wide registries.
    let sessions = Arc::clone(&svc.sessions);
    let registry = Arc::clone(&svc.registry);
    let core_addr = svc.bound_addr.clone();

    tokio::spawn(run_agent_stream(
        incoming, out_tx, sessions, registry, core_addr,
    ));

    Ok(())
}

/// Drives one client's bidi stream end-to-end: reads the mandatory
/// `Register` frame, validates it, resolves the owning session, and then
/// hands off to [`stream::run_with_register`] for the main loop.
///
/// On protocol errors (stream closed before `Register`, wrong first
/// message kind, empty `agent_id`, unknown `session_id`) the task pushes
/// a single `Err(Status)` onto `out_tx` and drops the sender. tonic
/// translates that into a clean gRPC error on the client's inbound
/// stream, matching what every other RPC in this service returns when
/// its preconditions fail.
async fn run_agent_stream(
    mut incoming: tonic::Streaming<AgentOutbound>,
    out_tx: mpsc::Sender<Result<AgentInbound, Status>>,
    sessions: Arc<RwLock<SessionRegistry>>,
    registry: Arc<RwLock<ModuleRegistry>>,
    core_addr: String,
) {
    let register = match read_first_register(&mut incoming).await {
        Ok(reg) => reg,
        Err(status) => {
            let _ = out_tx.send(Err(status)).await;
            return;
        }
    };

    if register.agent_id.is_empty() {
        let _ = out_tx
            .send(Err(Status::invalid_argument(
                "AgentRegister missing agent_id",
            )))
            .await;
        return;
    }

    let handle = {
        let sessions = sessions.read().await;
        sessions.get(&register.session_id)
    };
    let Some(handle) = handle else {
        let _ = out_tx
            .send(Err(Status::not_found(format!(
                "Session '{}' not found",
                register.session_id
            ))))
            .await;
        return;
    };

    let deps = StreamDeps {
        session: handle,
        registry,
        core_addr,
    };

    stream::run_with_register(register, incoming, out_tx, deps).await;
}

/// Reads the first `AgentOutbound` message off the incoming stream and
/// unwraps it into an [`AgentRegister`]. Any deviation from the
/// documented protocol is reported as a `Status` so the spawn wrapper
/// can forward it to the client verbatim.
async fn read_first_register(
    incoming: &mut tonic::Streaming<AgentOutbound>,
) -> Result<AgentRegister, Status> {
    match incoming.message().await {
        Ok(Some(msg)) => match msg.payload {
            Some(agent_outbound::Payload::Register(reg)) => Ok(reg),
            _ => Err(Status::invalid_argument(
                "AgentStream first message must be Register",
            )),
        },
        Ok(None) => Err(Status::invalid_argument(
            "AgentStream closed before sending Register",
        )),
        Err(e) => Err(Status::internal(format!("AgentStream recv failed: {e}"))),
    }
}
