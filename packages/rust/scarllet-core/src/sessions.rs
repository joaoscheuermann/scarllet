use std::collections::HashMap;

use scarllet_proto::proto::CoreEvent;
use tokio::sync::mpsc;
use tonic::Status;

/// Manages attached TUI sessions for event broadcasting.
pub struct TuiSessionRegistry {
    sessions: HashMap<String, mpsc::Sender<Result<CoreEvent, Status>>>,
}

impl TuiSessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn register(&mut self, id: String, sender: mpsc::Sender<Result<CoreEvent, Status>>) {
        self.sessions.insert(id, sender);
    }

    pub fn deregister(&mut self, id: &str) {
        self.sessions.remove(id);
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
}
