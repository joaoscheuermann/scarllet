use std::sync::Arc;
use std::time::Instant;

use scarllet_proto::proto::agent_message;
use scarllet_proto::proto::core_event;
use scarllet_proto::proto::orchestrator_server::Orchestrator;
use scarllet_proto::proto::tui_message;
use scarllet_proto::proto::*;
use scarllet_sdk::config::ScarlletConfig;
use scarllet_sdk::manifest::ModuleKind;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

use crate::agents::AgentRegistry;
use crate::events::build_provider_info_event;
use crate::registry::ModuleRegistry;
use crate::sessions::TuiSessionRegistry;
use crate::tasks::{self, TaskManager};
use crate::tools;

/// Central gRPC service implementing the `Orchestrator` trait.
///
/// Holds shared state (registries, config, task manager) behind `Arc<RwLock<_>>`
/// so concurrent request handlers can safely read and mutate state.
pub(crate) struct OrchestratorService {
    pub(crate) started_at: Instant,
    pub(crate) registry: Arc<RwLock<ModuleRegistry>>,
    pub(crate) config: Arc<RwLock<ScarlletConfig>>,
    pub(crate) task_manager: Arc<RwLock<TaskManager>>,
    pub(crate) session_registry: Arc<RwLock<TuiSessionRegistry>>,
    pub(crate) agent_registry: Arc<RwLock<AgentRegistry>>,
    pub(crate) bound_addr: String,
}

#[tonic::async_trait]
impl Orchestrator for OrchestratorService {
    /// Returns the server uptime so callers can verify liveness.
    async fn ping(&self, _req: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            uptime_secs: self.started_at.elapsed().as_secs(),
        }))
    }

    /// Lists all registered command modules for TUI autocompletion.
    async fn list_commands(
        &self,
        _req: Request<ListCommandsRequest>,
    ) -> Result<Response<ListCommandsResponse>, Status> {
        let reg = self.registry.read().await;
        let commands = reg
            .by_kind(ModuleKind::Command)
            .into_iter()
            .map(|(_, m)| CommandInfo {
                name: m.name.clone(),
                aliases: m.aliases.clone(),
                description: m.description.clone(),
            })
            .collect();
        Ok(Response::new(ListCommandsResponse { commands }))
    }

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
        match cfg.active_provider() {
            Some(provider) => {
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
            None => Ok(Response::new(ActiveProviderResponse {
                configured: false,
                ..Default::default()
            })),
        }
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

        let event = CoreEvent {
            payload: Some(core_event::Payload::DebugLog(DebugLogEvent {
                source: r.source.clone(),
                level: r.level.clone(),
                message: r.message.clone(),
                timestamp_ms,
            })),
        };
        self.session_registry.read().await.broadcast(event);
        Ok(Response::new(Ack {}))
    }

    /// Accepts a task submission, records it, and spawns the requested agent process.
    async fn submit_task(
        &self,
        req: Request<TaskSubmission>,
    ) -> Result<Response<TaskReceipt>, Status> {
        let r = req.get_ref();
        let snapshot_id = self.registry.read().await.version();
        let task_id = self.task_manager.write().await.submit(
            r.agent_name.clone(),
            r.working_directory.clone(),
            snapshot_id,
        );

        let registry = Arc::clone(&self.registry);
        let task_manager = Arc::clone(&self.task_manager);
        let tid = task_id.clone();
        let addr = self.bound_addr.clone();
        tokio::spawn(async move {
            tasks::spawn_agent(&registry, &task_manager, &tid, &addr).await;
        });

        Ok(Response::new(TaskReceipt {
            task_id,
            snapshot_id,
        }))
    }

    /// Terminates a running agent process and marks its task as cancelled.
    async fn cancel_task(
        &self,
        req: Request<CancelRequest>,
    ) -> Result<Response<CancelResponse>, Status> {
        let success = tasks::cancel_task(&self.task_manager, &req.get_ref().task_id).await;
        Ok(Response::new(CancelResponse { success }))
    }

    /// Routes a progress report from an agent to connected TUI sessions.
    ///
    /// The `status` field uses a string-based protocol contract defined by the
    /// agent binary interface: `"response"` for final answers, `"error"` for
    /// failures, and any other value (typically `"thinking"`) for in-progress
    /// updates. A proto-level enum would be preferable but requires a schema
    /// migration.
    async fn report_progress(&self, req: Request<ProgressReport>) -> Result<Response<Ack>, Status> {
        let r = req.get_ref();
        let mut tm = self.task_manager.write().await;
        tm.add_progress(&r.task_id, format!("[{}] {}", r.status, r.message));
        let agent_name = tm
            .get(&r.task_id)
            .map(|t| t.agent_name.clone())
            .unwrap_or_default();
        drop(tm);

        let text_block = vec![AgentBlock {
            block_type: "text".into(),
            content: r.message.clone(),
        }];
        let event = match r.status.as_str() {
            "response" => CoreEvent {
                payload: Some(core_event::Payload::AgentResponse(AgentResponseEvent {
                    task_id: r.task_id.clone(),
                    agent_name,
                    blocks: text_block,
                })),
            },
            "error" => CoreEvent {
                payload: Some(core_event::Payload::AgentError(AgentErrorEvent {
                    task_id: r.task_id.clone(),
                    agent_name,
                    error: r.message.clone(),
                })),
            },
            _ => CoreEvent {
                payload: Some(core_event::Payload::AgentThinking(AgentThinkingEvent {
                    task_id: r.task_id.clone(),
                    agent_name,
                    blocks: text_block,
                })),
            },
        };
        self.session_registry.read().await.broadcast(event);

        Ok(Response::new(Ack {}))
    }

    /// Looks up a task by ID and returns its current status and progress log.
    async fn get_agent_status(
        &self,
        req: Request<AgentStatusQuery>,
    ) -> Result<Response<AgentStatusResponse>, Status> {
        let tm = self.task_manager.read().await;
        match tm.get(&req.get_ref().task_id) {
            Some(task) => Ok(Response::new(AgentStatusResponse {
                task_id: task.task_id.clone(),
                agent_name: task.agent_name.clone(),
                status: task.status.to_string(),
                progress_log: task.progress_log.clone(),
                working_directory: task.working_directory.clone(),
            })),
            None => Err(Status::not_found("Task not found")),
        }
    }

    type AttachTuiStream = ReceiverStream<Result<CoreEvent, Status>>;

    /// Opens a bidirectional stream between a TUI client and the core.
    ///
    /// Registers the session, sends initial state (connected + provider info),
    /// then spawns a task to forward incoming prompts and cancellations. The
    /// session is deregistered when the stream closes.
    async fn attach_tui(
        &self,
        request: Request<tonic::Streaming<TuiMessage>>,
    ) -> Result<Response<Self::AttachTuiStream>, Status> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(256);

        self.session_registry
            .write()
            .await
            .register(session_id.clone(), tx.clone());
        info!("TUI session {session_id} attached");

        let connected = CoreEvent {
            payload: Some(core_event::Payload::Connected(ConnectedEvent {
                uptime_secs: self.started_at.elapsed().as_secs(),
            })),
        };
        let _ = tx.try_send(Ok(connected));

        let provider_info = build_provider_info_event(&*self.config.read().await);
        let _ = tx.try_send(Ok(provider_info));

        let mut incoming = request.into_inner();
        let session_registry = Arc::clone(&self.session_registry);
        let agent_registry = Arc::clone(&self.agent_registry);
        let registry = Arc::clone(&self.registry);
        let config = Arc::clone(&self.config);
        let task_manager = Arc::clone(&self.task_manager);
        let core_addr = self.bound_addr.clone();

        tokio::spawn(async move {
            while let Ok(Some(msg)) = incoming.message().await {
                match msg.payload {
                    Some(tui_message::Payload::Prompt(prompt)) => {
                        crate::routing::route_prompt(
                            &prompt.text,
                            &prompt.working_directory,
                            &registry,
                            &config,
                            &task_manager,
                            &session_registry,
                            &agent_registry,
                            &core_addr,
                        )
                        .await;
                    }
                    Some(tui_message::Payload::Cancel(cancel)) => {
                        let agent_name = {
                            let tm = task_manager.read().await;
                            tm.get(&cancel.task_id)
                                .map(|t| t.agent_name.clone())
                                .unwrap_or_default()
                        };
                        let success =
                            tasks::cancel_task(&task_manager, &cancel.task_id).await;
                        if success {
                            let event = CoreEvent {
                                payload: Some(core_event::Payload::AgentError(
                                    AgentErrorEvent {
                                        task_id: cancel.task_id,
                                        agent_name,
                                        error: "Cancelled by user".into(),
                                    },
                                )),
                            };
                            session_registry.read().await.broadcast(event);
                        }
                    }
                    _ => {}
                }
            }
            session_registry.write().await.deregister(&session_id);
            info!("TUI session {session_id} disconnected");
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type AgentStreamStream = ReceiverStream<Result<AgentTask, Status>>;

    /// Opens a bidirectional stream for a long-lived agent process.
    ///
    /// The agent registers itself, then receives tasks and sends back progress,
    /// results, failures, tool calls, and token usage. Orphaned tasks are marked
    /// failed when the stream disconnects.
    async fn agent_stream(
        &self,
        request: Request<tonic::Streaming<AgentMessage>>,
    ) -> Result<Response<Self::AgentStreamStream>, Status> {
        let (task_tx, task_rx) = tokio::sync::mpsc::channel::<Result<AgentTask, Status>>(64);

        let mut incoming = request.into_inner();
        let agent_registry = Arc::clone(&self.agent_registry);
        let session_registry = Arc::clone(&self.session_registry);
        let task_manager = Arc::clone(&self.task_manager);

        tokio::spawn(async move {
            let mut agent_name: Option<String> = None;

            while let Ok(Some(msg)) = incoming.message().await {
                let Some(payload) = msg.payload else {
                    continue;
                };
                match payload {
                    agent_message::Payload::Register(reg) => {
                        let name = reg.agent_name.clone();
                        agent_registry
                            .write()
                            .await
                            .register(name.clone(), task_tx.clone());
                        agent_name = Some(name.clone());
                        info!("Agent '{name}' registered via AgentStream");
                    }
                    agent_message::Payload::Progress(p) => {
                        let tm = task_manager.read().await;
                        let a_name = tm
                            .get(&p.task_id)
                            .map(|t| t.agent_name.clone())
                            .unwrap_or_default();
                        drop(tm);
                        let event = CoreEvent {
                            payload: Some(core_event::Payload::AgentThinking(
                                AgentThinkingEvent {
                                    task_id: p.task_id,
                                    agent_name: a_name,
                                    blocks: p.blocks,
                                },
                            )),
                        };
                        session_registry.read().await.broadcast(event);
                    }
                    agent_message::Payload::Result(r) => {
                        let tm = task_manager.read().await;
                        let a_name = tm
                            .get(&r.task_id)
                            .map(|t| t.agent_name.clone())
                            .unwrap_or_default();
                        drop(tm);

                        let mut tm = task_manager.write().await;
                        tm.set_status(&r.task_id, tasks::TaskStatus::Completed);
                        drop(tm);

                        let event = CoreEvent {
                            payload: Some(core_event::Payload::AgentResponse(
                                AgentResponseEvent {
                                    task_id: r.task_id,
                                    agent_name: a_name,
                                    blocks: r.blocks,
                                },
                            )),
                        };
                        session_registry.read().await.broadcast(event);
                    }
                    agent_message::Payload::Failure(f) => {
                        let tm = task_manager.read().await;
                        let a_name = tm
                            .get(&f.task_id)
                            .map(|t| t.agent_name.clone())
                            .unwrap_or_default();
                        drop(tm);

                        let mut tm = task_manager.write().await;
                        tm.set_status(&f.task_id, tasks::TaskStatus::Failed);
                        drop(tm);

                        let event = CoreEvent {
                            payload: Some(core_event::Payload::AgentError(AgentErrorEvent {
                                task_id: f.task_id,
                                agent_name: a_name,
                                error: f.error,
                            })),
                        };
                        session_registry.read().await.broadcast(event);
                    }
                    agent_message::Payload::ToolCall(tc) => {
                        let tm = task_manager.read().await;
                        let a_name = tm
                            .get(&tc.task_id)
                            .map(|t| t.agent_name.clone())
                            .unwrap_or_default();
                        drop(tm);

                        let event = CoreEvent {
                            payload: Some(core_event::Payload::AgentToolCall(
                                AgentToolCallEvent {
                                    task_id: tc.task_id,
                                    agent_name: a_name,
                                    call_id: tc.call_id,
                                    tool_name: tc.tool_name,
                                    arguments_preview: tc.arguments_preview,
                                    status: tc.status,
                                    duration_ms: tc.duration_ms,
                                    result: tc.result,
                                },
                            )),
                        };
                        session_registry.read().await.broadcast(event);
                    }
                    agent_message::Payload::TokenUsage(tu) => {
                        let event = CoreEvent {
                            payload: Some(core_event::Payload::TokenUsage(
                                TokenUsageEvent {
                                    total_tokens: tu.total_tokens,
                                    context_window: tu.context_window,
                                },
                            )),
                        };
                        session_registry.read().await.broadcast(event);
                    }
                }
            }

            if let Some(name) = agent_name {
                agent_registry.write().await.deregister(&name);
                info!("Agent '{name}' disconnected from AgentStream");

                let mut tm = task_manager.write().await;
                let orphaned = tm.active_tasks_for_agent(&name);
                for tid in &orphaned {
                    tm.set_status(tid, tasks::TaskStatus::Failed);
                    tm.add_progress(tid, "Agent disconnected unexpectedly".into());
                }
                drop(tm);

                for tid in orphaned {
                    let event = CoreEvent {
                        payload: Some(core_event::Payload::AgentError(AgentErrorEvent {
                            task_id: tid,
                            agent_name: name.clone(),
                            error: "Agent disconnected unexpectedly".into(),
                        })),
                    };
                    session_registry.read().await.broadcast(event);
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(task_rx)))
    }
}
