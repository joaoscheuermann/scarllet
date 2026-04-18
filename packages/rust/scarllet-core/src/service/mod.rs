//! Thin gRPC wiring for the `Orchestrator` service.
//!
//! [`OrchestratorService`] is the single `tonic` implementer; each RPC
//! delegates straight through to the sibling modules which own the
//! per-surface business logic (session lifecycle, tool invocation,
//! agent stream + per-turn unary queries).

/// `Orchestrator` impl: agent-facing RPCs (per-turn unary + bidi stream).
pub mod agent_rpc;
/// `Orchestrator` impl: session lifecycle + TUI-facing RPCs.
pub mod session_rpc;
/// `Orchestrator` impl: tool-registry + invocation RPCs.
pub mod tool_rpc;

use std::sync::Arc;

use scarllet_proto::proto::orchestrator_server::Orchestrator;
use scarllet_proto::proto::*;
use scarllet_sdk::config::ScarlletConfig;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::registry::ModuleRegistry;
use crate::session::SessionRegistry;

/// Central gRPC service implementing the `Orchestrator` trait.
///
/// Holds shared state (registries, config, sessions) behind `Arc<RwLock<_>>`
/// so concurrent request handlers can safely read and mutate state. All the
/// heavy per-RPC logic lives in the sibling modules; this struct stays a
/// thin wiring layer.
pub struct OrchestratorService {
    /// Module manifests discovered by the watcher.
    pub registry: Arc<RwLock<ModuleRegistry>>,
    /// Global config (provider list + default agent).
    pub config: Arc<RwLock<ScarlletConfig>>,
    /// All active sessions.
    pub sessions: Arc<RwLock<SessionRegistry>>,
    /// Address the gRPC server is bound to (passed to spawned agents).
    pub bound_addr: String,
}

#[tonic::async_trait]
impl Orchestrator for OrchestratorService {
    async fn create_session(
        &self,
        req: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        session_rpc::create_session(self, req).await
    }

    async fn list_sessions(
        &self,
        req: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        session_rpc::list_sessions(self, req).await
    }

    async fn destroy_session(
        &self,
        req: Request<DestroySessionRequest>,
    ) -> Result<Response<DestroySessionResponse>, Status> {
        session_rpc::destroy_session(self, req).await
    }

    async fn get_session_state(
        &self,
        req: Request<GetSessionStateRequest>,
    ) -> Result<Response<SessionState>, Status> {
        session_rpc::get_session_state(self, req).await
    }

    type AttachSessionStream = ReceiverStream<Result<SessionDiff, Status>>;

    async fn attach_session(
        &self,
        req: Request<AttachSessionRequest>,
    ) -> Result<Response<Self::AttachSessionStream>, Status> {
        session_rpc::attach_session(self, req).await
    }

    async fn send_prompt(
        &self,
        req: Request<SendPromptRequest>,
    ) -> Result<Response<SendPromptResponse>, Status> {
        session_rpc::send_prompt(self, req).await
    }

    async fn stop_session(
        &self,
        req: Request<StopSessionRequest>,
    ) -> Result<Response<StopSessionResponse>, Status> {
        session_rpc::stop_session(self, req).await
    }

    async fn get_active_provider(
        &self,
        req: Request<GetActiveProviderRequest>,
    ) -> Result<Response<ActiveProviderResponse>, Status> {
        agent_rpc::get_active_provider(self, req).await
    }

    async fn get_tool_registry(
        &self,
        req: Request<GetToolRegistryRequest>,
    ) -> Result<Response<GetToolRegistryResponse>, Status> {
        tool_rpc::get_tool_registry(self, req).await
    }

    async fn get_conversation_history(
        &self,
        req: Request<GetConversationHistoryRequest>,
    ) -> Result<Response<ConversationHistoryResponse>, Status> {
        agent_rpc::get_conversation_history(self, req).await
    }

    async fn invoke_tool(
        &self,
        req: Request<InvokeToolRequest>,
    ) -> Result<Response<InvokeToolResponse>, Status> {
        tool_rpc::invoke_tool(self, req).await
    }

    type AgentStreamStream = ReceiverStream<Result<AgentInbound, Status>>;

    async fn agent_stream(
        &self,
        req: Request<tonic::Streaming<AgentOutbound>>,
    ) -> Result<Response<Self::AgentStreamStream>, Status> {
        let (out_tx, out_rx) = mpsc::channel::<Result<AgentInbound, Status>>(64);
        agent_rpc::agent_stream(self, req, out_tx).await?;
        Ok(Response::new(ReceiverStream::new(out_rx)))
    }
}
