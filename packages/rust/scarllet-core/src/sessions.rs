use std::collections::HashMap;

use scarllet_proto::proto::CoreEvent;
use tokio::sync::mpsc;
use tonic::Status;

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

#[cfg(test)]
mod tests {
    use super::*;
    use scarllet_proto::proto::core_event;
    use scarllet_proto::proto::ConnectedEvent;

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
