mod agents;
mod registry;
mod sessions;
mod tasks;
mod tools;
mod watcher;

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Instant;

use clap::Parser;
use agents::AgentRegistry;
use registry::ModuleRegistry;
use scarllet_proto::proto::agent_message;
use scarllet_proto::proto::core_event;
use scarllet_proto::proto::orchestrator_server::{Orchestrator, OrchestratorServer};
use scarllet_proto::proto::tui_message;
use scarllet_proto::proto::*;
use scarllet_sdk::config::{self, ScarlletConfig};
use scarllet_sdk::lockfile;
use scarllet_sdk::manifest::ModuleKind;
use sessions::TuiSessionRegistry;
use std::sync::Arc;
use tasks::TaskManager;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Parser)]
#[command(name = "scarllet-core", about = "Scarllet Core Orchestrator")]
struct Cli {}

struct OrchestratorService {
    started_at: Instant,
    registry: Arc<RwLock<ModuleRegistry>>,
    config: Arc<RwLock<ScarlletConfig>>,
    task_manager: Arc<RwLock<TaskManager>>,
    session_registry: Arc<RwLock<TuiSessionRegistry>>,
    agent_registry: Arc<RwLock<AgentRegistry>>,
    bound_addr: String,
}

#[tonic::async_trait]
impl Orchestrator for OrchestratorService {
    async fn ping(&self, _req: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            uptime_secs: self.started_at.elapsed().as_secs(),
        }))
    }

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

    async fn cancel_task(
        &self,
        req: Request<CancelRequest>,
    ) -> Result<Response<CancelResponse>, Status> {
        let success = tasks::cancel_task(&self.task_manager, &req.get_ref().task_id).await;
        Ok(Response::new(CancelResponse { success }))
    }

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
                if let Some(tui_message::Payload::Prompt(prompt)) = msg.payload {
                    route_prompt(
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
            }
            session_registry.write().await.deregister(&session_id);
            info!("TUI session {session_id} disconnected");
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type AgentStreamStream = ReceiverStream<Result<AgentTask, Status>>;

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

fn build_provider_info_event(cfg: &ScarlletConfig) -> CoreEvent {
    match cfg.active_provider() {
        Some(provider) => CoreEvent {
            payload: Some(core_event::Payload::ProviderInfo(ProviderInfoEvent {
                provider_name: provider.name.clone(),
                model: provider.model.clone(),
                reasoning_effort: provider
                    .reasoning_effort()
                    .unwrap_or_default()
                    .to_string(),
            })),
        },
        None => CoreEvent {
            payload: Some(core_event::Payload::ProviderInfo(ProviderInfoEvent {
                provider_name: String::new(),
                model: String::new(),
                reasoning_effort: String::new(),
            })),
        },
    }
}

async fn route_prompt(
    text: &str,
    working_dir: &str,
    registry: &Arc<RwLock<ModuleRegistry>>,
    config: &Arc<RwLock<ScarlletConfig>>,
    task_manager: &Arc<RwLock<TaskManager>>,
    session_registry: &Arc<RwLock<TuiSessionRegistry>>,
    agent_registry: &Arc<RwLock<AgentRegistry>>,
    core_addr: &str,
) {
    {
        let cfg = config.read().await;
        if cfg.active_provider().is_none() {
            let path = config::config_path();
            let sys = CoreEvent {
                payload: Some(core_event::Payload::System(SystemEvent {
                    message: format!(
                        "No provider configured. Edit config.json at {} to set up a provider.",
                        path.display()
                    ),
                })),
            };
            drop(cfg);
            session_registry.read().await.broadcast(sys);
            return;
        }
    }

    let reg = registry.read().await;

    if text.starts_with('/') {
        let cmd_name = text
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        let has_command = reg.by_kind(ModuleKind::Command).into_iter().any(|(_, m)| {
            m.name == cmd_name
                || m.aliases
                    .iter()
                    .any(|a| a.trim_start_matches('/') == cmd_name)
        });
        if has_command {
            let sys = CoreEvent {
                payload: Some(core_event::Payload::System(SystemEvent {
                    message: format!("Command '/{cmd_name}' is not yet implemented in chat mode."),
                })),
            };
            drop(reg);
            session_registry.read().await.broadcast(sys);
            return;
        }
    }

    let agents = reg.by_kind(ModuleKind::Agent);
    if agents.is_empty() {
        let sys = CoreEvent {
            payload: Some(core_event::Payload::System(SystemEvent {
                message: "No agent available to handle this prompt.".into(),
            })),
        };
        drop(reg);
        session_registry.read().await.broadcast(sys);
        return;
    }

    let agent_name = agents[0].1.name.clone();
    let snapshot_id = reg.version();
    drop(reg);

    let task_id = task_manager.write().await.submit(
        agent_name.clone(),
        working_dir.to_string(),
        snapshot_id,
    );

    let started = CoreEvent {
        payload: Some(core_event::Payload::AgentStarted(AgentStartedEvent {
            task_id: task_id.clone(),
            agent_name: agent_name.clone(),
        })),
    };
    session_registry.read().await.broadcast(started);

    let ar = agent_registry.read().await;
    if let Some(sender) = ar.get(&agent_name) {
        let task = AgentTask {
            task_id: task_id.clone(),
            prompt: text.to_string(),
            working_directory: working_dir.to_string(),
        };
        let _ = sender.try_send(Ok(task));
        return;
    }
    drop(ar);

    let registry = Arc::clone(registry);
    let task_manager = Arc::clone(task_manager);
    let session_registry = Arc::clone(session_registry);
    let agent_registry = Arc::clone(agent_registry);
    let tid = task_id.clone();
    let a_name = agent_name.clone();
    let addr = core_addr.to_string();
    let prompt_text = text.to_string();
    let wd = working_dir.to_string();

    let spawn_registry = Arc::clone(&registry);
    let spawn_task_manager = Arc::clone(&task_manager);
    let spawn_tid = tid.clone();
    let spawn_addr = addr.clone();
    tokio::spawn(async move {
        tasks::spawn_agent(&spawn_registry, &spawn_task_manager, &spawn_tid, &spawn_addr).await;
    });

    tokio::spawn(async move {
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let ar = agent_registry.read().await;
            if let Some(sender) = ar.get(&a_name) {
                let task = AgentTask {
                    task_id: tid.clone(),
                    prompt: prompt_text,
                    working_directory: wd,
                };
                let _ = sender.try_send(Ok(task));
                return;
            }
        }

        let tm = task_manager.read().await;
        let Some(task) = tm.get(&tid) else {
            return;
        };
        let event = match task.status {
            tasks::TaskStatus::Completed => CoreEvent {
                payload: Some(core_event::Payload::AgentResponse(AgentResponseEvent {
                    task_id: tid.clone(),
                    agent_name: task.agent_name.clone(),
                    blocks: vec![AgentBlock {
                        block_type: "text".into(),
                        content: "Task completed.".into(),
                    }],
                })),
            },
            tasks::TaskStatus::Failed => CoreEvent {
                payload: Some(core_event::Payload::AgentError(AgentErrorEvent {
                    task_id: tid.clone(),
                    agent_name: task.agent_name.clone(),
                    error: task
                        .progress_log
                        .last()
                        .cloned()
                        .unwrap_or_else(|| "Unknown error".into()),
                })),
            },
            _ => return,
        };
        drop(tm);
        session_registry.read().await.broadcast(event);
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _cli = Cli::parse();
    tracing_subscriber::fmt::init();

    let started_at = Instant::now();
    let registry = Arc::new(RwLock::new(ModuleRegistry::new()));
    let task_manager = Arc::new(RwLock::new(TaskManager::new()));
    let session_registry = Arc::new(RwLock::new(TuiSessionRegistry::new()));
    let agent_registry = Arc::new(RwLock::new(AgentRegistry::new()));

    let cfg = config::load().unwrap_or_default();
    info!("Loaded {} provider(s) from config", cfg.providers.len());
    let config = Arc::new(RwLock::new(cfg));

    let dirs = watcher::watched_dirs();
    watcher::ensure_dirs(&dirs);

    let watcher_registry = Arc::clone(&registry);
    tokio::spawn(async move {
        watcher::run(watcher_registry, dirs).await;
    });

    let watcher_config = Arc::clone(&config);
    let watcher_sessions = Arc::clone(&session_registry);
    tokio::spawn(async move {
        watcher::watch_config(watcher_config, watcher_sessions).await;
    });

    let addr: SocketAddr = "127.0.0.1:0".parse()?;
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    let bound_addr_str = bound_addr.to_string();

    info!("Listening on {}", bound_addr);

    lockfile::write(&bound_addr)?;

    let service = OrchestratorService {
        started_at,
        registry,
        config,
        task_manager,
        session_registry,
        agent_registry,
        bound_addr: bound_addr_str,
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Mutex::new(Some(shutdown_tx));
    ctrlc::set_handler(move || {
        if let Some(tx) = shutdown_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    })?;

    let incoming = TcpListenerStream::new(listener);

    tonic::transport::Server::builder()
        .add_service(OrchestratorServer::new(service))
        .serve_with_incoming_shutdown(incoming, async {
            let _ = shutdown_rx.await;
            info!("Shutdown signal received");
        })
        .await?;

    lockfile::remove();
    info!("Core stopped");
    println!("Core stopped");

    Ok(())
}
