use std::collections::HashMap;

use scarllet_proto::proto::AgentTask;
use tokio::sync::mpsc;
use tonic::Status;

/// Tracks long-lived agent processes that have an active AgentStream.
pub struct AgentRegistry {
    agents: HashMap<String, mpsc::Sender<Result<AgentTask, Status>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: String, sender: mpsc::Sender<Result<AgentTask, Status>>) {
        self.agents.insert(name, sender);
    }

    pub fn deregister(&mut self, name: &str) {
        self.agents.remove(name);
    }

    pub fn get(&self, name: &str) -> Option<&mpsc::Sender<Result<AgentTask, Status>>> {
        self.agents.get(name)
    }

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
