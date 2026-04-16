use std::sync::Arc;

use scarllet_proto::proto::core_event;
use scarllet_proto::proto::*;
use scarllet_sdk::config::{self, ScarlletConfig};
use scarllet_sdk::manifest::ModuleKind;
use tokio::sync::RwLock;

use crate::agents::AgentRegistry;
use crate::registry::ModuleRegistry;
use crate::sessions::TuiSessionRegistry;
use crate::tasks::{self, TaskManager};

/// Decides how to handle a TUI prompt.
///
/// Checks for a configured provider, recognises slash-commands, then
/// dispatches to an available agent — either over an existing `AgentStream`
/// connection or by spawning a new agent process and waiting for it to
/// register within a 10-second window.
pub(crate) async fn route_prompt(
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

    let notify = agent_registry.read().await.notifier();

    let spawn_registry = Arc::clone(&registry);
    let spawn_task_manager = Arc::clone(&task_manager);
    let spawn_tid = tid.clone();
    let spawn_addr = addr.clone();
    tokio::spawn(async move {
        tasks::spawn_agent(&spawn_registry, &spawn_task_manager, &spawn_tid, &spawn_addr).await;
    });

    // Alternative B (future): buffer tasks in a queue and let agents drain on connect,
    // removing the need for Notify altogether.
    tokio::spawn(async move {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            async {
                loop {
                    notify.notified().await;
                    let ar = agent_registry.read().await;
                    if let Some(sender) = ar.get(&a_name) {
                        let task = AgentTask {
                            task_id: tid.clone(),
                            prompt: prompt_text.clone(),
                            working_directory: wd.clone(),
                        };
                        let _ = sender.try_send(Ok(task));
                        return;
                    }
                }
            },
        )
        .await;

        if result.is_err() {
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
        }
    });
}
