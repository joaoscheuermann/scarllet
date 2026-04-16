use std::collections::HashMap;
use std::sync::Arc;

use scarllet_proto::blocks_to_text;
use scarllet_proto::proto::{
    agent_message, AgentFailure, AgentInstruction, AgentMessage, AgentProgressMsg, AgentRegister,
    AgentResultMsg, AgentTokenUsageMsg, AgentToolCallMsg, HistoryEntry,
};
use tokio::sync::{mpsc, Notify, RwLock};
use tonic::Status;
use tracing::info;

use crate::events;
use crate::sessions::TuiSessionRegistry;
use crate::tasks::TaskManager;

/// Tracks long-lived agent processes connected via `AgentStream`.
///
/// Each agent registers a task sender so the core can dispatch work to it
/// without spawning a new process. A `Notify` wakes any coroutine waiting
/// for an agent to appear after a fresh spawn.
pub struct AgentRegistry {
    agents: HashMap<String, mpsc::Sender<Result<AgentInstruction, Status>>>,
    notify: Arc<Notify>,
}

impl AgentRegistry {
    /// Initialises an empty registry with a shared notifier.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Inserts an agent and wakes any coroutines waiting for it.
    pub fn register(&mut self, name: String, sender: mpsc::Sender<Result<AgentInstruction, Status>>) {
        self.agents.insert(name, sender);
        self.notify.notify_waiters();
    }

    /// Returns a clonable handle used to await agent registration.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    /// Removes an agent, typically when its stream disconnects.
    pub fn deregister(&mut self, name: &str) {
        self.agents.remove(name);
    }

    /// Removes all agents, returning their names. Dropping the senders
    /// closes each agent's task channel, causing it to exit.
    pub fn deregister_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self.agents.keys().cloned().collect();
        self.agents.clear();
        names
    }

    /// Returns the task sender for a connected agent, if present.
    pub fn get(&self, name: &str) -> Option<&mpsc::Sender<Result<AgentInstruction, Status>>> {
        self.agents.get(name)
    }

    /// Checks whether an agent with the given name is currently connected.
    #[allow(dead_code)] // This is used for testing
    pub fn is_running(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }
}

/// Shared handles the agent message loop needs.
pub(crate) struct AgentStreamDeps {
    pub agent_registry: Arc<RwLock<AgentRegistry>>,
    pub session_registry: Arc<RwLock<TuiSessionRegistry>>,
    pub task_manager: Arc<RwLock<TaskManager>>,
    pub conversation_history: Arc<RwLock<Vec<HistoryEntry>>>,
}

/// Runs the bidirectional `AgentStream` message loop until the agent
/// disconnects. Failures during streaming always propagate a final
/// `AgentError` back to the attached TUIs so tasks never hang silently.
pub(crate) async fn run_agent_stream(
    mut incoming: tonic::Streaming<AgentMessage>,
    task_tx: mpsc::Sender<Result<AgentInstruction, Status>>,
    deps: AgentStreamDeps,
) {
    let mut agent_name: Option<String> = None;

    while let Ok(Some(msg)) = incoming.message().await {
        let Some(payload) = msg.payload else {
            continue;
        };
        match payload {
            agent_message::Payload::Register(reg) => {
                agent_name = Some(handle_register(&deps, &task_tx, reg).await);
            }
            agent_message::Payload::Progress(p) => handle_progress(&deps, p).await,
            agent_message::Payload::Result(r) => handle_result(&deps, r).await,
            agent_message::Payload::Failure(f) => handle_failure(&deps, f).await,
            agent_message::Payload::ToolCall(tc) => handle_tool_call(&deps, tc).await,
            agent_message::Payload::TokenUsage(tu) => handle_token_usage(&deps, tu).await,
        }
    }

    if let Some(name) = agent_name {
        handle_disconnect(&deps, &name).await;
    }
}

/// Registers the agent in the [`AgentRegistry`] and seeds it with any existing
/// conversation history so it can reconstruct LLM context. Returns the name
/// the agent registered under, so the outer loop can track it for disconnect
/// cleanup.
async fn handle_register(
    deps: &AgentStreamDeps,
    task_tx: &mpsc::Sender<Result<AgentInstruction, Status>>,
    reg: AgentRegister,
) -> String {
    let name = reg.agent_name.clone();
    deps.agent_registry
        .write()
        .await
        .register(name.clone(), task_tx.clone());
    info!("Agent '{name}' registered via AgentStream");

    let history = deps.conversation_history.read().await.clone();
    if !history.is_empty() {
        let _ = task_tx.try_send(Ok(events::history_instruction(history)));
    }
    name
}

/// Forwards streaming reasoning/content blocks to the attached TUIs.
async fn handle_progress(deps: &AgentStreamDeps, p: AgentProgressMsg) {
    let a_name = deps.task_manager.read().await.agent_name_for(&p.task_id);
    deps.session_registry
        .read()
        .await
        .broadcast(events::agent_thinking(p.task_id, a_name, p.blocks));
}

/// Marks the task completed, appends the assistant reply to the canonical
/// conversation history, and broadcasts the final response to every TUI.
async fn handle_result(deps: &AgentStreamDeps, r: AgentResultMsg) {
    let a_name = deps.task_manager.read().await.agent_name_for(&r.task_id);
    deps.task_manager.write().await.mark_completed(&r.task_id);

    let assistant_text = blocks_to_text(&r.blocks);
    if !assistant_text.is_empty() {
        deps.conversation_history.write().await.push(HistoryEntry {
            role: "assistant".into(),
            content: assistant_text,
        });
    }

    deps.session_registry
        .read()
        .await
        .broadcast(events::agent_response(r.task_id, a_name, r.blocks));
}

/// Marks the task failed and broadcasts the agent-reported error to TUIs.
async fn handle_failure(deps: &AgentStreamDeps, f: AgentFailure) {
    let a_name = deps.task_manager.read().await.agent_name_for(&f.task_id);
    deps.task_manager.write().await.mark_failed(&f.task_id, &f.error);
    deps.session_registry
        .read()
        .await
        .broadcast(events::agent_error(f.task_id, a_name, f.error));
}

/// Forwards a tool-call lifecycle update (running/done/failed) to TUIs.
async fn handle_tool_call(deps: &AgentStreamDeps, tc: AgentToolCallMsg) {
    let a_name = deps.task_manager.read().await.agent_name_for(&tc.task_id);
    deps.session_registry
        .read()
        .await
        .broadcast(events::agent_tool_call(
            tc.task_id,
            a_name,
            tc.call_id,
            tc.tool_name,
            tc.arguments_preview,
            tc.status,
            tc.duration_ms,
            tc.result,
        ));
}

/// Forwards the agent's reported token usage for the status bar.
async fn handle_token_usage(deps: &AgentStreamDeps, tu: AgentTokenUsageMsg) {
    deps.session_registry
        .read()
        .await
        .broadcast(events::token_usage(tu.total_tokens, tu.context_window));
}

/// Deregisters the agent, fails its orphaned tasks, and broadcasts an error
/// for each orphan so the TUI does not keep spinning indefinitely.
async fn handle_disconnect(deps: &AgentStreamDeps, name: &str) {
    deps.agent_registry.write().await.deregister(name);
    info!("Agent '{name}' disconnected from AgentStream");

    let orphaned = {
        let mut tm = deps.task_manager.write().await;
        let ids = tm.active_tasks_for_agent(name);
        for tid in &ids {
            tm.mark_failed(tid, "Agent disconnected unexpectedly");
        }
        ids
    };

    let sessions = deps.session_registry.read().await;
    for tid in orphaned {
        sessions.broadcast(events::agent_error(
            tid,
            name.to_string(),
            "Agent disconnected unexpectedly".into(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_get() {
        let mut reg = AgentRegistry::new();
        let (tx, _rx) = mpsc::channel::<Result<AgentInstruction, Status>>(16);
        reg.register("chat".into(), tx);

        assert!(reg.is_running("chat"));
        assert!(reg.get("chat").is_some());
        assert!(!reg.is_running("other"));
    }

    #[tokio::test]
    async fn deregister_removes() {
        let mut reg = AgentRegistry::new();
        let (tx, _rx) = mpsc::channel::<Result<AgentInstruction, Status>>(16);
        reg.register("chat".into(), tx);
        reg.deregister("chat");

        assert!(!reg.is_running("chat"));
        assert!(reg.get("chat").is_none());
    }

    #[tokio::test]
    async fn deregister_all_clears_and_returns_names() {
        let mut reg = AgentRegistry::new();
        let (tx1, _rx1) = mpsc::channel::<Result<AgentInstruction, Status>>(16);
        let (tx2, _rx2) = mpsc::channel::<Result<AgentInstruction, Status>>(16);
        reg.register("alpha".into(), tx1);
        reg.register("beta".into(), tx2);

        let mut names = reg.deregister_all();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert!(!reg.is_running("alpha"));
        assert!(!reg.is_running("beta"));
    }

    #[tokio::test]
    async fn deregister_all_closes_channels() {
        let mut reg = AgentRegistry::new();
        let (tx, mut rx) = mpsc::channel::<Result<AgentInstruction, Status>>(16);
        reg.register("agent".into(), tx);

        reg.deregister_all();

        // Channel should be closed — recv returns None
        assert!(rx.recv().await.is_none());
    }
}
