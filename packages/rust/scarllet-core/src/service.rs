use std::sync::Arc;
use std::time::Instant;

use scarllet_proto::proto::orchestrator_server::Orchestrator;
use scarllet_proto::proto::*;
use scarllet_sdk::config::ScarlletConfig;
use scarllet_sdk::manifest::ModuleKind;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

use crate::agents::{self, AgentRegistry, AgentStreamDeps};
use crate::events;
use crate::registry::ModuleRegistry;
use crate::sessions::{self, TuiSessionRegistry, TuiStreamDeps};
use crate::tasks::TaskManager;
use crate::tools;

/// Central gRPC service implementing the `Orchestrator` trait.
///
/// Holds shared state (registries, config, task manager) behind `Arc<RwLock<_>>`
/// so concurrent request handlers can safely read and mutate state. All the
/// heavy per-variant logic lives in [`sessions`] and [`agents`]; this impl is
/// deliberately a thin wiring layer.
pub(crate) struct OrchestratorService {
    pub(crate) started_at: Instant,
    pub(crate) registry: Arc<RwLock<ModuleRegistry>>,
    pub(crate) config: Arc<RwLock<ScarlletConfig>>,
    pub(crate) task_manager: Arc<RwLock<TaskManager>>,
    pub(crate) session_registry: Arc<RwLock<TuiSessionRegistry>>,
    pub(crate) agent_registry: Arc<RwLock<AgentRegistry>>,
    pub(crate) conversation_history: Arc<RwLock<Vec<HistoryEntry>>>,
    pub(crate) bound_addr: String,
}

#[tonic::async_trait]
impl Orchestrator for OrchestratorService {
    /// Returns the full tool catalog so agents can discover available tools.
    async fn get_tool_registry(
        &self,
        _req: Request<ToolRegistryQuery>,
    ) -> Result<Response<ToolRegistryResponse>, Status> {
        let reg = self.registry.read().await;
        let tools = reg
            .by_kind(ModuleKind::Tool)
            .into_iter()
            .map(|(_, m)| ToolInfo {
                name: m.name.clone(),
                description: m.description.clone(),
                input_schema_json: m
                    .input_schema
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                timeout_ms: m.timeout_ms.unwrap_or(30000),
            })
            .collect();
        Ok(Response::new(ToolRegistryResponse { tools }))
    }

    /// Returns the currently active LLM provider configuration.
    async fn get_active_provider(
        &self,
        _req: Request<ActiveProviderQuery>,
    ) -> Result<Response<ActiveProviderResponse>, Status> {
        let cfg = self.config.read().await;
        let Some(provider) = cfg.active_provider() else {
            return Ok(Response::new(ActiveProviderResponse {
                configured: false,
                ..Default::default()
            }));
        };
        let type_str = match provider.provider_type {
            scarllet_sdk::config::ProviderType::Openai => "openai",
            scarllet_sdk::config::ProviderType::Gemini => "gemini",
        };
        Ok(Response::new(ActiveProviderResponse {
            configured: true,
            provider_name: provider.name.clone(),
            provider_type: type_str.into(),
            api_url: provider.api_url.clone().unwrap_or_default(),
            api_key: provider.api_key.clone(),
            model: provider.model.clone(),
            reasoning_effort: provider
                .reasoning_effort()
                .unwrap_or_default()
                .to_string(),
        }))
    }

    /// Executes a registered tool by name, forwarding JSON input and returning the result.
    async fn invoke_tool(
        &self,
        req: Request<ToolInvocation>,
    ) -> Result<Response<ToolResult>, Status> {
        let r = req.get_ref();
        let result =
            tools::invoke(&self.registry, &r.tool_name, &r.input_json, r.snapshot_id).await;
        Ok(Response::new(ToolResult {
            success: result.success,
            output_json: result.output_json,
            error_message: result.error_message,
            duration_ms: result.duration_ms,
        }))
    }

    /// Broadcasts a timestamped debug log entry to all connected TUI sessions.
    async fn emit_debug_log(
        &self,
        req: Request<DebugLogRequest>,
    ) -> Result<Response<Ack>, Status> {
        let r = req.get_ref();
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.session_registry
            .read()
            .await
            .broadcast(events::debug_log(
                r.source.clone(),
                r.level.clone(),
                r.message.clone(),
                timestamp_ms,
            ));
        Ok(Response::new(Ack {}))
    }

    type AttachTuiStream = ReceiverStream<Result<CoreEvent, Status>>;

    /// Opens a bidirectional stream between a TUI client and the core.
    ///
    /// Registers the session, sends initial state, and delegates the message
    /// loop to [`sessions::run_tui_stream`]. The session is deregistered when
    /// the stream closes.
    async fn attach_tui(
        &self,
        request: Request<tonic::Streaming<TuiMessage>>,
    ) -> Result<Response<Self::AttachTuiStream>, Status> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel(256);

        self.session_registry
            .write()
            .await
            .register(session_id.clone(), tx.clone());
        info!("TUI session {session_id} attached");

        sessions::send_initial_state(
            &tx,
            self.started_at.elapsed().as_secs(),
            &*self.config.read().await,
        );

        let deps = TuiStreamDeps {
            registry: Arc::clone(&self.registry),
            config: Arc::clone(&self.config),
            task_manager: Arc::clone(&self.task_manager),
            session_registry: Arc::clone(&self.session_registry),
            agent_registry: Arc::clone(&self.agent_registry),
            conversation_history: Arc::clone(&self.conversation_history),
            core_addr: self.bound_addr.clone(),
        };

        tokio::spawn(sessions::run_tui_stream(
            session_id,
            request.into_inner(),
            deps,
        ));

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type AgentStreamStream = ReceiverStream<Result<AgentInstruction, Status>>;

    /// Opens a bidirectional stream for a long-lived agent process.
    ///
    /// Hands the message loop off to [`agents::run_agent_stream`], which
    /// handles registration, progress forwarding, tool-call updates, and
    /// disconnect cleanup. Orphaned tasks are marked failed when the stream
    /// drops.
    async fn agent_stream(
        &self,
        request: Request<tonic::Streaming<AgentMessage>>,
    ) -> Result<Response<Self::AgentStreamStream>, Status> {
        let (task_tx, task_rx) = mpsc::channel::<Result<AgentInstruction, Status>>(64);

        let deps = AgentStreamDeps {
            agent_registry: Arc::clone(&self.agent_registry),
            session_registry: Arc::clone(&self.session_registry),
            task_manager: Arc::clone(&self.task_manager),
            conversation_history: Arc::clone(&self.conversation_history),
        };

        tokio::spawn(agents::run_agent_stream(request.into_inner(), task_tx, deps));

        Ok(Response::new(ReceiverStream::new(task_rx)))
    }
}
