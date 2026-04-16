use std::sync::Arc;
use std::time::Duration;

use scarllet_proto::proto::AgentBlock;
use scarllet_sdk::config::{self, ScarlletConfig};
use scarllet_sdk::manifest::ModuleKind;
use tokio::sync::RwLock;

use crate::agents::AgentRegistry;
use crate::events;
use crate::registry::ModuleRegistry;
use crate::sessions::TuiSessionRegistry;
use crate::tasks::{self, TaskManager, TaskStatus};

/// How long the router waits for a freshly spawned agent to register
/// on `AgentStream` before giving up and reporting task status back to TUIs.
const AGENT_REGISTER_TIMEOUT: Duration = Duration::from_secs(10);

/// Decides how to handle a TUI prompt.
///
/// Ensures a provider is configured, picks the first available agent, records a
/// task, and dispatches it — either over an existing `AgentStream` connection
/// or by spawning a new agent process and waiting for it to register within
/// [`AGENT_REGISTER_TIMEOUT`].
#[allow(clippy::too_many_arguments)]
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
    if !ensure_provider_configured(config, session_registry).await {
        return;
    }

    let Some((agent_name, snapshot_id)) = pick_agent(registry, session_registry).await else {
        return;
    };

    let task_id = task_manager
        .write()
        .await
        .submit(agent_name.clone(), working_dir.to_string(), snapshot_id);

    session_registry
        .read()
        .await
        .broadcast(events::agent_started(task_id.clone(), agent_name.clone()));

    if dispatch_to_connected(agent_registry, &agent_name, &task_id, text, working_dir).await {
        return;
    }

    spawn_and_dispatch(
        registry,
        task_manager,
        session_registry,
        agent_registry,
        core_addr,
        agent_name,
        task_id,
        text.to_string(),
        working_dir.to_string(),
    );
}

/// Returns `true` if a provider is configured. Otherwise broadcasts an
/// explanatory `SystemEvent` and returns `false`.
async fn ensure_provider_configured(
    config: &Arc<RwLock<ScarlletConfig>>,
    session_registry: &Arc<RwLock<TuiSessionRegistry>>,
) -> bool {
    if config.read().await.active_provider().is_some() {
        return true;
    }
    let path = config::config_path();
    let message = format!(
        "No provider configured. Edit config.json at {} to set up a provider.",
        path.display()
    );
    session_registry.read().await.broadcast(events::system(message));
    false
}

/// Picks the first registered agent and returns its name alongside the current
/// module-registry snapshot ID. Broadcasts an explanation and returns `None` if
/// no agent is registered.
async fn pick_agent(
    registry: &Arc<RwLock<ModuleRegistry>>,
    session_registry: &Arc<RwLock<TuiSessionRegistry>>,
) -> Option<(String, u64)> {
    let reg = registry.read().await;
    let agents = reg.by_kind(ModuleKind::Agent);
    if let Some((_, manifest)) = agents.first() {
        return Some((manifest.name.clone(), reg.version()));
    }
    drop(reg);
    session_registry
        .read()
        .await
        .broadcast(events::system(
            "No agent available to handle this prompt.".into(),
        ));
    None
}

/// Attempts to deliver the task to an already-connected agent. Returns `true`
/// on success.
async fn dispatch_to_connected(
    agent_registry: &Arc<RwLock<AgentRegistry>>,
    agent_name: &str,
    task_id: &str,
    prompt: &str,
    working_dir: &str,
) -> bool {
    let ar = agent_registry.read().await;
    let Some(sender) = ar.get(agent_name) else {
        return false;
    };
    let instruction =
        events::task_instruction(task_id.to_string(), prompt.to_string(), working_dir.to_string());
    let _ = sender.try_send(Ok(instruction));
    true
}

/// Spawns the agent binary and a waiter that delivers the task once the agent
/// registers (or reports final status if the spawn completed before the waiter
/// noticed).
#[allow(clippy::too_many_arguments)]
fn spawn_and_dispatch(
    registry: &Arc<RwLock<ModuleRegistry>>,
    task_manager: &Arc<RwLock<TaskManager>>,
    session_registry: &Arc<RwLock<TuiSessionRegistry>>,
    agent_registry: &Arc<RwLock<AgentRegistry>>,
    core_addr: &str,
    agent_name: String,
    task_id: String,
    prompt: String,
    working_dir: String,
) {
    let spawn_registry = Arc::clone(registry);
    let spawn_task_manager = Arc::clone(task_manager);
    let spawn_tid = task_id.clone();
    let spawn_addr = core_addr.to_string();
    tokio::spawn(async move {
        tasks::spawn_agent(&spawn_registry, &spawn_task_manager, &spawn_tid, &spawn_addr).await;
    });

    let wait_task_manager = Arc::clone(task_manager);
    let wait_session_registry = Arc::clone(session_registry);
    let wait_agent_registry = Arc::clone(agent_registry);
    tokio::spawn(async move {
        wait_for_registration_and_dispatch(
            wait_task_manager,
            wait_session_registry,
            wait_agent_registry,
            agent_name,
            task_id,
            prompt,
            working_dir,
        )
        .await;
    });
}

/// Blocks on `AgentRegistry::notifier()` until the target agent registers
/// (or a timeout elapses) and then either delivers the task or reports
/// whatever terminal status the spawned child reached.
async fn wait_for_registration_and_dispatch(
    task_manager: Arc<RwLock<TaskManager>>,
    session_registry: Arc<RwLock<TuiSessionRegistry>>,
    agent_registry: Arc<RwLock<AgentRegistry>>,
    agent_name: String,
    task_id: String,
    prompt: String,
    working_dir: String,
) {
    let notify = agent_registry.read().await.notifier();

    // Alternative B (future): buffer tasks in a queue and let agents drain on connect,
    // removing the need for Notify altogether.
    let delivered = tokio::time::timeout(AGENT_REGISTER_TIMEOUT, async {
        loop {
            notify.notified().await;
            let ar = agent_registry.read().await;
            let Some(sender) = ar.get(&agent_name) else {
                continue;
            };
            let instruction = events::task_instruction(
                task_id.clone(),
                prompt.clone(),
                working_dir.clone(),
            );
            let _ = sender.try_send(Ok(instruction));
            return;
        }
    })
    .await;

    if delivered.is_ok() {
        return;
    }

    broadcast_terminal_status(&task_manager, &session_registry, &task_id).await;
}

/// When registration timed out, inspect the spawned child's final task status
/// and forward a matching `AgentResponse`/`AgentError` to the TUI so the chat
/// view does not stay stuck on the spinner.
async fn broadcast_terminal_status(
    task_manager: &Arc<RwLock<TaskManager>>,
    session_registry: &Arc<RwLock<TuiSessionRegistry>>,
    task_id: &str,
) {
    let tm = task_manager.read().await;
    let Some(task) = tm.get(task_id) else {
        return;
    };
    let event = match task.status {
        TaskStatus::Completed => events::agent_response(
            task_id.to_string(),
            task.agent_name.clone(),
            vec![AgentBlock {
                block_type: "text".into(),
                content: "Task completed.".into(),
            }],
        ),
        TaskStatus::Failed => events::agent_error(
            task_id.to_string(),
            task.agent_name.clone(),
            task.progress_log
                .last()
                .cloned()
                .unwrap_or_else(|| "Unknown error".into()),
        ),
        _ => return,
    };
    drop(tm);
    session_registry.read().await.broadcast(event);
}
