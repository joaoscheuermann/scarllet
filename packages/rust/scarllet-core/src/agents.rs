use std::collections::HashMap;
use std::sync::Arc;

use scarllet_proto::proto::AgentTask;
use tokio::sync::{mpsc, Notify};
use tonic::Status;

/// Tracks long-lived agent processes connected via `AgentStream`.
///
/// Each agent registers a task sender so the core can dispatch work to it
/// without spawning a new process. A `Notify` wakes any coroutine waiting
/// for an agent to appear after a fresh spawn.
pub struct AgentRegistry {
    agents: HashMap<String, mpsc::Sender<Result<AgentTask, Status>>>,
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
    pub fn register(&mut self, name: String, sender: mpsc::Sender<Result<AgentTask, Status>>) {
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

    /// Returns the task sender for a connected agent, if present.
    pub fn get(&self, name: &str) -> Option<&mpsc::Sender<Result<AgentTask, Status>>> {
        self.agents.get(name)
    }

    /// Checks whether an agent with the given name is currently connected.
    #[allow(dead_code)] // This is used for testing
    pub fn is_running(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_get() {
        let mut reg = AgentRegistry::new();
        let (tx, _rx) = mpsc::channel::<Result<AgentTask, Status>>(16);
        reg.register("chat".into(), tx);

        assert!(reg.is_running("chat"));
        assert!(reg.get("chat").is_some());
        assert!(!reg.is_running("other"));
    }

    #[tokio::test]
    async fn deregister_removes() {
        let mut reg = AgentRegistry::new();
        let (tx, _rx) = mpsc::channel::<Result<AgentTask, Status>>(16);
        reg.register("chat".into(), tx);
        reg.deregister("chat");

        assert!(!reg.is_running("chat"));
        assert!(reg.get("chat").is_none());
    }
}
