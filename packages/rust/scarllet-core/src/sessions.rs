use std::collections::HashMap;
use std::sync::Arc;

use scarllet_proto::proto::{
    tui_message, CancelPrompt, CoreEvent, HistoryEntry, HistorySync, PromptMessage, TuiMessage,
};
use scarllet_sdk::config::ScarlletConfig;
use tokio::sync::{mpsc, RwLock};
use tonic::Status;
use tracing::info;

use crate::agents::AgentRegistry;
use crate::events;
use crate::registry::ModuleRegistry;
use crate::routing;
use crate::tasks::{self, TaskManager};

/// Shared handles the TUI message loop needs. Grouping these into a single
/// struct keeps [`run_tui_stream`] and its helpers honest about their
/// dependencies (see dependency-injection convention).
pub(crate) struct TuiStreamDeps {
    pub registry: Arc<RwLock<ModuleRegistry>>,
    pub config: Arc<RwLock<ScarlletConfig>>,
    pub task_manager: Arc<RwLock<TaskManager>>,
    pub session_registry: Arc<RwLock<TuiSessionRegistry>>,
    pub agent_registry: Arc<RwLock<AgentRegistry>>,
    pub conversation_history: Arc<RwLock<Vec<HistoryEntry>>>,
    pub core_addr: String,
}

/// Manages attached TUI sessions for event broadcasting.
pub struct TuiSessionRegistry {
    sessions: HashMap<String, mpsc::Sender<Result<CoreEvent, Status>>>,
}

impl TuiSessionRegistry {
    /// Initialises an empty session registry.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Adds a TUI session identified by a unique ID.
    pub fn register(&mut self, id: String, sender: mpsc::Sender<Result<CoreEvent, Status>>) {
        self.sessions.insert(id, sender);
    }

    /// Removes a disconnected TUI session so it no longer receives events.
    pub fn deregister(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    /// Returns `true` when no TUI sessions are connected.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Sends an event to all connected TUI sessions. Silently drops on full/closed channels.
    pub fn broadcast(&self, event: CoreEvent) {
        for sender in self.sessions.values() {
            let _ = sender.try_send(Ok(event.clone()));
        }
    }
}

/// Pushes the initial handshake events (`Connected` + current `ProviderInfo`)
/// to a freshly attached TUI before the bidirectional loop starts.
pub(crate) fn send_initial_state(
    tx: &mpsc::Sender<Result<CoreEvent, Status>>,
    uptime_secs: u64,
    cfg: &ScarlletConfig,
) {
    let _ = tx.try_send(Ok(events::connected(uptime_secs)));
    let _ = tx.try_send(Ok(events::provider_info(cfg)));
}

/// Runs the bidirectional TUI message loop until the client disconnects,
/// dispatching each payload variant to its dedicated handler and cleaning up
/// agent state once the last TUI leaves.
pub(crate) async fn run_tui_stream(
    session_id: String,
    mut incoming: tonic::Streaming<TuiMessage>,
    deps: TuiStreamDeps,
) {
    while let Ok(Some(msg)) = incoming.message().await {
        let Some(payload) = msg.payload else {
            continue;
        };
        match payload {
            tui_message::Payload::Prompt(prompt) => handle_prompt(&deps, prompt).await,
            tui_message::Payload::HistorySync(sync) => {
                handle_history_sync(&deps, &session_id, sync).await
            }
            tui_message::Payload::Cancel(cancel) => handle_cancel(&deps, cancel).await,
        }
    }

    deps.session_registry
        .write()
        .await
        .deregister(&session_id);
    info!("TUI session {session_id} disconnected");

    if deps.session_registry.read().await.is_empty() {
        cleanup_when_no_tuis(&deps).await;
    }
}

/// Records the user prompt in the canonical conversation history, then
/// delegates routing to [`routing::route_prompt`].
async fn handle_prompt(deps: &TuiStreamDeps, prompt: PromptMessage) {
    deps.conversation_history.write().await.push(HistoryEntry {
        role: "user".into(),
        content: prompt.text.clone(),
    });

    routing::route_prompt(
        &prompt.text,
        &prompt.working_directory,
        &deps.registry,
        &deps.config,
        &deps.task_manager,
        &deps.session_registry,
        &deps.agent_registry,
        &deps.core_addr,
    )
    .await;
}

/// Overwrites the core's conversation transcript with the snapshot the TUI
/// sent on (re)connect so subsequently registered agents can be re-seeded.
async fn handle_history_sync(deps: &TuiStreamDeps, session_id: &str, sync: HistorySync) {
    let mut history = deps.conversation_history.write().await;
    *history = sync.messages;
    info!(
        "TUI session {session_id} sent history sync ({} entries)",
        history.len()
    );
}

/// Cancels the running task, marks it cancelled, and broadcasts a visible
/// "Cancelled by user" error back to every TUI.
async fn handle_cancel(deps: &TuiStreamDeps, cancel: CancelPrompt) {
    let agent_name = deps.task_manager.read().await.agent_name_for(&cancel.task_id);
    let success = tasks::cancel_task(&deps.task_manager, &cancel.task_id).await;
    if !success {
        return;
    }
    deps.session_registry
        .read()
        .await
        .broadcast(events::agent_error(
            cancel.task_id,
            agent_name,
            "Cancelled by user".into(),
        ));
}

/// Best-effort cleanup when no TUI remains attached: cancel every running task
/// and drop the agent registry so agent processes exit with their channels.
async fn cleanup_when_no_tuis(deps: &TuiStreamDeps) {
    info!("No TUI sessions remain, cleaning up agents");
    let active = deps.task_manager.read().await.all_active_task_ids();
    for tid in &active {
        tasks::cancel_task(&deps.task_manager, tid).await;
    }
    let agents = deps.agent_registry.write().await.deregister_all();
    for name in &agents {
        info!("Agent '{name}' disconnected (no TUI sessions)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarllet_proto::proto::{core_event, ConnectedEvent};

    fn make_event() -> CoreEvent {
        CoreEvent {
            payload: Some(core_event::Payload::Connected(ConnectedEvent {
                uptime_secs: 42,
            })),
        }
    }

    #[tokio::test]
    async fn register_and_broadcast() {
        let mut registry = TuiSessionRegistry::new();
        let (tx, mut rx) = mpsc::channel(16);
        registry.register("s1".into(), tx);

        registry.broadcast(make_event());

        let received = rx.try_recv().unwrap().unwrap();
        assert!(matches!(
            received.payload,
            Some(core_event::Payload::Connected(_))
        ));
    }

    #[tokio::test]
    async fn deregister_stops_broadcast() {
        let mut registry = TuiSessionRegistry::new();
        let (tx, mut rx) = mpsc::channel(16);
        registry.register("s1".into(), tx);
        registry.deregister("s1");

        registry.broadcast(make_event());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn is_empty_reflects_session_count() {
        let mut registry = TuiSessionRegistry::new();
        assert!(registry.is_empty());

        let (tx1, _rx1) = mpsc::channel(16);
        registry.register("s1".into(), tx1);
        assert!(!registry.is_empty());

        let (tx2, _rx2) = mpsc::channel(16);
        registry.register("s2".into(), tx2);
        assert!(!registry.is_empty());

        registry.deregister("s1");
        assert!(!registry.is_empty());

        registry.deregister("s2");
        assert!(registry.is_empty());
    }
}
