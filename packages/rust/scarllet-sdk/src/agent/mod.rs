//! Thin client SDK for agent processes.
//!
//! Wraps the generated `OrchestratorClient` so individual agents do not
//! reach into `scarllet_proto` directly. Effort 02 expands the surface
//! to support real LLM streaming:
//!
//! - [`AgentSession::connect`] reads the `SCARLLET_*` env vars, opens the
//!   bidi stream, and sends the initial `Register` message.
//! - [`AgentSession::next_task`] blocks until the core dispatches an
//!   [`AgentTask`].
//! - [`AgentSession::get_provider`] / [`AgentSession::get_history`]
//!   fetch the per-turn provider snapshot and the chronological chat
//!   history derived by core from the session node graph.
//! - [`AgentSession::create_thought`] / [`AgentSession::append_thought`]
//!   grow a single `Thought` node character-by-character via the
//!   architecture's append-mode `UpdateNode` path.
//! - [`AgentSession::create_result`] / [`AgentSession::emit_result`]
//!   land the final `Result` node; `emit_result` also sends the
//!   `TurnFinished` envelope so the per-turn process can exit cleanly.
//! - [`AgentSession::emit_failure`] signals an unrecoverable
//!   pre-`Result` failure to core.
//!
//! Tool-call helpers, sub-agent spawn, debug / token emission, and other
//! niceties land in efforts 03–07.

use std::env;

use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::{
    agent_inbound, agent_outbound, node, ActiveProviderResponse, AgentFailure, AgentInbound,
    AgentOutbound, AgentRegister, AgentTask, CreateNode, DebugPayload, ErrorPayload,
    GetActiveProviderRequest, GetConversationHistoryRequest, GetToolRegistryRequest, HistoryEntry,
    InvokeToolRequest, InvokeToolResponse, Node, NodeKind, NodePatch, ResultPayload,
    ThoughtPayload, TokenUsagePayload, ToolInfo, ToolPayload, TurnFinished, UpdateNode,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::Streaming;
use uuid::Uuid;

/// Wire-level lifecycle string carried in `tool_status` patches.
///
/// One-updated model from AC-5.6: a single `Tool` node moves through these
/// states via [`AgentSession::update_tool_status`] without ever creating a
/// second node. The `Display` impl returns the canonical lowercase wire
/// strings (`pending` / `running` / `done` / `failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Tool node was just created; the call has not been dispatched yet.
    Pending,
    /// Core has accepted the invocation; the tool process is running.
    Running,
    /// Tool process completed successfully and `result_json` carries the
    /// JSON output.
    Done,
    /// Tool invocation failed; `result_json` carries the error message
    /// (so UIs and history can render it without a second field).
    Failed,
}

impl ToolStatus {
    /// Canonical wire string used by the `tool_status` field in `NodePatch`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for ToolStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_wire())
    }
}

/// Errors surfaced to the agent binary by [`AgentSession`].
#[derive(Debug)]
pub enum AgentSdkError {
    /// A required `SCARLLET_*` env var was missing or empty.
    MissingEnv(&'static str),
    /// gRPC transport / dial failure.
    Transport(String),
    /// The bidi outbound channel was closed (core gone).
    ChannelClosed,
    /// `tonic::Status` returned by an RPC.
    Rpc(tonic::Status),
    /// A `spawn_sub_agent` tool call returned `success == false` (or the
    /// response payload could not be parsed). The message carries the
    /// failure returned by core.
    SubAgent(String),
}

impl std::fmt::Display for AgentSdkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEnv(name) => write!(f, "missing env var '{name}'"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
            Self::ChannelClosed => f.write_str("agent stream channel closed"),
            Self::Rpc(s) => write!(f, "rpc error: {s}"),
            Self::SubAgent(msg) => write!(f, "sub-agent failed: {msg}"),
        }
    }
}

impl std::error::Error for AgentSdkError {}

impl From<tonic::Status> for AgentSdkError {
    fn from(s: tonic::Status) -> Self {
        Self::Rpc(s)
    }
}

/// Live agent session bound to a single core stream.
///
/// Holds the outbound sender and inbound receiver halves of the
/// `AgentStream` bidi RPC plus identity metadata read from env at connect
/// time.
pub struct AgentSession {
    /// Session this agent belongs to (`SCARLLET_SESSION_ID`).
    pub session_id: String,
    /// Core-assigned id for this agent process (`SCARLLET_AGENT_ID`).
    pub agent_id: String,
    /// Parent id — `session_id` for main agents, calling agent id for subs
    /// (`SCARLLET_PARENT_ID`).
    pub parent_id: String,
    /// Manifest name of this module (`SCARLLET_AGENT_MODULE`).
    pub agent_module: String,
    /// gRPC client used for unary per-turn RPCs (`get_provider`,
    /// `get_history`, …) issued alongside the bidi stream.
    pub client: OrchestratorClient<Channel>,
    /// Outbound sender — every `AgentOutbound` we want to send goes here.
    pub out_tx: mpsc::Sender<AgentOutbound>,
    /// Inbound stream of `AgentInbound` messages from core.
    pub in_rx: Streaming<AgentInbound>,
    /// Id of the `Agent` node core created for this turn. Sent back to
    /// `CreateNode` so children parent correctly. Populated from
    /// `SCARLLET_AGENT_ID` (which equals the agent node id by core
    /// convention).
    pub agent_node_id: String,
}

impl AgentSession {
    /// Reads the `SCARLLET_*` env vars, opens the `AgentStream` bidi RPC,
    /// and sends the initial `Register` message. Returns once the stream
    /// is established.
    pub async fn connect() -> Result<Self, AgentSdkError> {
        let core_addr = read_env("SCARLLET_CORE_ADDR")?;
        let session_id = read_env("SCARLLET_SESSION_ID")?;
        let agent_id = read_env("SCARLLET_AGENT_ID")?;
        let parent_id = read_env("SCARLLET_PARENT_ID")?;
        let agent_module = read_env("SCARLLET_AGENT_MODULE")?;

        let endpoint = format!("http://{core_addr}");
        let mut client = OrchestratorClient::connect(endpoint)
            .await
            .map_err(|e| AgentSdkError::Transport(e.to_string()))?
            .max_decoding_message_size(64 * 1024 * 1024)
            .max_encoding_message_size(64 * 1024 * 1024);

        let (out_tx, out_rx) = mpsc::channel::<AgentOutbound>(64);

        // Push `Register` onto the outbound channel **before** dialing the
        // bidi RPC. tonic's HTTP/2 client waits for the server's response
        // headers before it can send the first request frame — but the
        // server handler must not await on `incoming.message()` before
        // returning `Response` (see `scarllet-core/src/service/agent_rpc.rs`).
        // Even though core's handler is now correctly spawn-and-return-
        // immediately, this preemptive send is defense in depth: any
        // future regression that reintroduces the synchronous first-read
        // anti-pattern would still clear because the client already has a
        // message queued when tonic starts polling the outbound stream.
        //
        // The channel has buffer capacity 64; this send is non-blocking.
        let register = AgentOutbound {
            payload: Some(agent_outbound::Payload::Register(AgentRegister {
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                agent_module: agent_module.clone(),
                parent_id: parent_id.clone(),
            })),
        };
        out_tx
            .send(register)
            .await
            .map_err(|_| AgentSdkError::ChannelClosed)?;

        let outgoing = ReceiverStream::new(out_rx);
        let in_rx = client
            .agent_stream(outgoing)
            .await
            .map_err(AgentSdkError::Rpc)?
            .into_inner();

        Ok(Self {
            session_id,
            agent_id: agent_id.clone(),
            parent_id,
            agent_module,
            client,
            out_tx,
            in_rx,
            agent_node_id: agent_id,
        })
    }

    /// Blocks on the inbound stream until an [`AgentTask`] arrives. Returns
    /// `None` when the stream closes, when core sends `CancelNow`, or when
    /// the inbound payload cannot be decoded.
    pub async fn next_task(&mut self) -> Option<AgentTask> {
        let msg = self.in_rx.message().await.ok().flatten()?;
        match msg.payload? {
            agent_inbound::Payload::Task(task) => Some(task),
            agent_inbound::Payload::Cancel(_) => None,
        }
    }

    /// Fetches the per-session active provider snapshot core captured at
    /// session-create time. Called once at the start of every turn before
    /// instantiating the LLM client.
    pub async fn get_provider(&mut self) -> Result<ActiveProviderResponse, AgentSdkError> {
        let resp = self
            .client
            .get_active_provider(GetActiveProviderRequest {
                session_id: self.session_id.clone(),
            })
            .await?
            .into_inner();
        Ok(resp)
    }

    /// Fetches the chronological chat history derived by core from the
    /// session node graph. Used by main agents to reconstruct multi-turn
    /// LLM context.
    pub async fn get_history(&mut self) -> Result<Vec<HistoryEntry>, AgentSdkError> {
        let resp = self
            .client
            .get_conversation_history(GetConversationHistoryRequest {
                session_id: self.session_id.clone(),
            })
            .await?
            .into_inner();
        Ok(resp.messages)
    }

    /// Fetches the per-session tool registry. Includes every external
    /// `Tool`-kind module plus the synthetic `spawn_sub_agent` entry (the
    /// runtime branch for `spawn_sub_agent` is wired in effort 5).
    pub async fn get_tools(&mut self) -> Result<Vec<ToolInfo>, AgentSdkError> {
        let resp = self
            .client
            .get_tool_registry(GetToolRegistryRequest {
                session_id: self.session_id.clone(),
            })
            .await?
            .into_inner();
        Ok(resp.tools)
    }

    /// Invokes a registered tool through the session-scoped `InvokeTool`
    /// RPC. The call carries the agent's `session_id` + `agent_id` so core
    /// can validate the originator and apply per-session policy in future
    /// phases.
    pub async fn invoke_tool(
        &mut self,
        tool_name: &str,
        input_json: &str,
    ) -> Result<InvokeToolResponse, AgentSdkError> {
        let resp = self
            .client
            .invoke_tool(InvokeToolRequest {
                session_id: self.session_id.clone(),
                agent_id: self.agent_id.clone(),
                tool_name: tool_name.to_string(),
                input_json: input_json.to_string(),
            })
            .await?
            .into_inner();
        Ok(resp)
    }

    /// Creates a fresh `Tool` node parented on the supplied `Agent` node id
    /// in the `pending` state and returns the new node's id. Subsequent
    /// [`Self::update_tool_status`] calls drive it through `running →
    /// done | failed` per the one-updated model (AC-5.6).
    pub async fn create_tool(
        &self,
        parent_agent_node_id: &str,
        tool_name: &str,
        arguments_preview: &str,
        arguments_json: &str,
    ) -> Result<String, AgentSdkError> {
        let id = Uuid::new_v4().to_string();
        let now = now_secs();
        let tool_node = Node {
            id: id.clone(),
            parent_id: Some(parent_agent_node_id.to_string()),
            kind: NodeKind::Tool as i32,
            created_at: now,
            updated_at: now,
            payload: Some(node::Payload::Tool(ToolPayload {
                tool_name: tool_name.to_string(),
                arguments_preview: arguments_preview.to_string(),
                arguments_json: arguments_json.to_string(),
                status: ToolStatus::Pending.to_string(),
                duration_ms: 0,
                result_json: String::new(),
            })),
        };
        self.send_outbound(agent_outbound::Payload::CreateNode(CreateNode {
            node: Some(tool_node),
        }))
        .await?;
        Ok(id)
    }

    /// Patches a `Tool` node with new `status` / `duration_ms` /
    /// `result_json`. Every field is replace-merged on the server (per the
    /// architecture's locked merge rules) so callers should always send
    /// the full final values for any field that changed.
    pub async fn update_tool_status(
        &self,
        node_id: &str,
        status: ToolStatus,
        duration_ms: u64,
        result_json: &str,
    ) -> Result<(), AgentSdkError> {
        let patch = NodePatch {
            tool_status: Some(status.to_string()),
            tool_duration_ms: Some(duration_ms),
            tool_result_json: Some(result_json.to_string()),
            ..Default::default()
        };
        self.send_outbound(agent_outbound::Payload::UpdateNode(UpdateNode {
            node_id: node_id.to_string(),
            patch: Some(patch),
        }))
        .await
    }

    /// Creates a fresh empty `Thought` node parented on the supplied agent
    /// node id and returns the new node's id. Subsequent
    /// [`Self::append_thought`] calls patch tokens into it character-by-
    /// character via the architecture's append-mode `UpdateNode` path.
    pub async fn create_thought(
        &self,
        parent_agent_node_id: &str,
    ) -> Result<String, AgentSdkError> {
        let id = Uuid::new_v4().to_string();
        let now = now_secs();
        let thought = Node {
            id: id.clone(),
            parent_id: Some(parent_agent_node_id.to_string()),
            kind: NodeKind::Thought as i32,
            created_at: now,
            updated_at: now,
            payload: Some(node::Payload::Thought(ThoughtPayload {
                content: String::new(),
            })),
        };
        self.send_outbound(agent_outbound::Payload::CreateNode(CreateNode {
            node: Some(thought),
        }))
        .await?;
        Ok(id)
    }

    /// Appends a streaming chunk to an existing `Thought` node. The
    /// server uses APPEND merge semantics on `thought_content` so each
    /// call grows the rendered text in place.
    pub async fn append_thought(
        &self,
        node_id: &str,
        chunk: &str,
    ) -> Result<(), AgentSdkError> {
        if chunk.is_empty() {
            return Ok(());
        }
        let patch = NodePatch {
            thought_content: Some(chunk.to_string()),
            ..Default::default()
        };
        self.send_outbound(agent_outbound::Payload::UpdateNode(UpdateNode {
            node_id: node_id.to_string(),
            patch: Some(patch),
        }))
        .await
    }

    /// Creates a `Result` node parented on this agent's Agent node and
    /// returns its id. Caller is responsible for following with a
    /// `TurnFinished` (use [`Self::emit_result`] for the common case).
    pub async fn create_result(&self, content: &str) -> Result<String, AgentSdkError> {
        let id = Uuid::new_v4().to_string();
        let now = now_secs();
        let result_node = Node {
            id: id.clone(),
            parent_id: Some(self.agent_node_id.clone()),
            kind: NodeKind::Result as i32,
            created_at: now,
            updated_at: now,
            payload: Some(node::Payload::Result(ResultPayload {
                content: content.into(),
                finish_reason: String::new(),
            })),
        };
        self.send_outbound(agent_outbound::Payload::CreateNode(CreateNode {
            node: Some(result_node),
        }))
        .await?;
        Ok(id)
    }

    /// Appends a streaming chunk to an existing `Result` node via the
    /// append-mode `result_content` patch. Reserved for future efforts
    /// that stream the user-visible answer (effort 02 only emits the
    /// final aggregated content via [`Self::emit_result`]).
    pub async fn append_result_content(
        &self,
        node_id: &str,
        chunk: &str,
    ) -> Result<(), AgentSdkError> {
        if chunk.is_empty() {
            return Ok(());
        }
        let patch = NodePatch {
            result_content: Some(chunk.to_string()),
            ..Default::default()
        };
        self.send_outbound(agent_outbound::Payload::UpdateNode(UpdateNode {
            node_id: node_id.to_string(),
            patch: Some(patch),
        }))
        .await
    }

    /// Convenience: emits a final `Result` node with the supplied content
    /// then `TurnFinished` so the per-turn process can exit cleanly. The
    /// `Result` node's `finish_reason` is patched in via `UpdateNode` so
    /// it appears alongside the content in core's snapshot.
    pub async fn emit_result(
        &self,
        content: &str,
        finish_reason: &str,
    ) -> Result<(), AgentSdkError> {
        let result_id = self.create_result(content).await?;
        if !finish_reason.is_empty() {
            let patch = NodePatch {
                result_finish_reason: Some(finish_reason.to_string()),
                ..Default::default()
            };
            self.send_outbound(agent_outbound::Payload::UpdateNode(UpdateNode {
                node_id: result_id,
                patch: Some(patch),
            }))
            .await?;
        }
        self.send_outbound(agent_outbound::Payload::TurnFinished(TurnFinished {
            finish_reason: finish_reason.into(),
        }))
        .await
    }

    /// Signals an unrecoverable failure before a `Result` node was emitted.
    /// Core records an `Error` node under the agent's Agent node and
    /// deregisters the agent (effort 06 will additionally flip the session
    /// to `Paused`).
    pub async fn emit_failure(&self, message: &str) -> Result<(), AgentSdkError> {
        self.send_outbound(agent_outbound::Payload::Failure(AgentFailure {
            message: message.into(),
        }))
        .await
    }

    /// Creates a `Debug` node parented on this agent's Agent node. Debug
    /// nodes are always broadcast to attached TUIs; TUIs apply their own
    /// `SCARLLET_DEBUG` filter at render time (AC-6.1 / AC-6.2).
    ///
    /// `level` is a free-form severity tag (`"info"` / `"warn"` / `"trace"`
    /// / …) surfaced in the `DebugPayload`. `source` is set to the agent
    /// module name so downstream UIs can distinguish agent-authored debug
    /// from core-authored debug when multiple sources co-exist.
    pub async fn emit_debug(&self, level: &str, message: &str) -> Result<(), AgentSdkError> {
        let id = Uuid::new_v4().to_string();
        let now = now_secs();
        let debug_node = Node {
            id,
            parent_id: Some(self.agent_node_id.clone()),
            kind: NodeKind::Debug as i32,
            created_at: now,
            updated_at: now,
            payload: Some(node::Payload::Debug(DebugPayload {
                source: self.agent_module.clone(),
                level: level.into(),
                message: message.into(),
            })),
        };
        self.send_outbound(agent_outbound::Payload::CreateNode(CreateNode {
            node: Some(debug_node),
        }))
        .await
    }

    /// Creates a `TokenUsage` node parented on this agent's Agent node so
    /// TUIs can surface per-turn token counters (AC-5.7). Callers typically
    /// emit one per turn once the LLM returns final `Usage` stats; if
    /// called multiple times, each call creates a fresh node — TUIs
    /// display the latest.
    pub async fn emit_token_usage(&self, total: u32, window: u32) -> Result<(), AgentSdkError> {
        let id = Uuid::new_v4().to_string();
        let now = now_secs();
        let usage_node = Node {
            id,
            parent_id: Some(self.agent_node_id.clone()),
            kind: NodeKind::TokenUsage as i32,
            created_at: now,
            updated_at: now,
            payload: Some(node::Payload::TokenUsage(TokenUsagePayload {
                total_tokens: total,
                context_window: window,
            })),
        };
        self.send_outbound(agent_outbound::Payload::CreateNode(CreateNode {
            node: Some(usage_node),
        }))
        .await
    }

    /// Creates an `Error` node parented on this agent's Agent node for
    /// per-turn recoverable errors (AC-3.4 belt-and-suspenders). Unlike
    /// [`Self::emit_failure`], this writes the Error node directly so it
    /// survives even if the agent process dies before core observes the
    /// stream close. The typical flow is to `emit_error` first and then
    /// return `Err` so `emit_failure` still fires on the normal path.
    pub async fn emit_error(&self, message: &str) -> Result<(), AgentSdkError> {
        let id = Uuid::new_v4().to_string();
        let now = now_secs();
        let error_node = Node {
            id,
            parent_id: Some(self.agent_node_id.clone()),
            kind: NodeKind::Error as i32,
            created_at: now,
            updated_at: now,
            payload: Some(node::Payload::Error(ErrorPayload {
                source: self.agent_module.clone(),
                message: message.into(),
            })),
        };
        self.send_outbound(agent_outbound::Payload::CreateNode(CreateNode {
            node: Some(error_node),
        }))
        .await
    }

    /// Convenience wrapper over [`Self::invoke_tool`] that targets the
    /// core-internal `spawn_sub_agent` tool. Constructs the input JSON,
    /// blocks until the sub-agent emits its `Result + TurnFinished`, and
    /// parses the returned `output_json` into a [`ResultPayload`].
    ///
    /// On failure (tool rejected, sub-agent crashed, cascade cancelled)
    /// returns [`AgentSdkError::SubAgent`] with the message from core. The
    /// default agent does **not** need to call this wrapper — the normal
    /// tool-calling loop routes spawn_sub_agent through `invoke_tool` — but
    /// human-authored agents can call it directly.
    pub async fn spawn_sub_agent(
        &mut self,
        agent_module: &str,
        prompt: &str,
    ) -> Result<ResultPayload, AgentSdkError> {
        let input = serde_json::json!({
            "agent_module": agent_module,
            "prompt": prompt,
        })
        .to_string();

        let response = self.invoke_tool("spawn_sub_agent", &input).await?;
        if !response.success {
            return Err(AgentSdkError::SubAgent(response.error_message));
        }

        let parsed: serde_json::Value = serde_json::from_str(&response.output_json)
            .map_err(|e| AgentSdkError::SubAgent(format!("malformed sub-agent output: {e}")))?;
        let content = parsed
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let finish_reason = parsed
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(ResultPayload {
            content,
            finish_reason,
        })
    }

    /// Internal helper: wraps a `Payload` in an `AgentOutbound` envelope
    /// and posts it to the bidi outbound channel. Centralised so every
    /// emit path uses the same channel-closed → [`AgentSdkError`] map.
    async fn send_outbound(
        &self,
        payload: agent_outbound::Payload,
    ) -> Result<(), AgentSdkError> {
        self.out_tx
            .send(AgentOutbound {
                payload: Some(payload),
            })
            .await
            .map_err(|_| AgentSdkError::ChannelClosed)
    }
}

/// Reads an env var, returning [`AgentSdkError::MissingEnv`] when missing
/// or empty.
fn read_env(name: &'static str) -> Result<String, AgentSdkError> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(AgentSdkError::MissingEnv(name)),
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
