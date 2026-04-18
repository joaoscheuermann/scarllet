//! Fan-out helper for per-session subscriber channels.
//!
//! Used by [`crate::session::Session`] to broadcast [`scarllet_proto::proto::SessionDiff`]s
//! to every attached TUI and auto-prune senders whose receiver has been
//! dropped.

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tonic::Status;

/// Fan-out helper that mirrors a stream of values to every connected
/// subscriber and silently drops senders whose receiver has been dropped.
///
/// Used inside `Session` to broadcast `SessionDiff` messages to every
/// attached TUI without coupling each producer to the gRPC stream type.
pub struct SubscriberSet<T> {
    senders: Vec<mpsc::Sender<Result<T, Status>>>,
}

impl<T: Clone> Default for SubscriberSet<T> {
    /// Empty subscriber set; identical to [`SubscriberSet::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> SubscriberSet<T> {
    /// Initialises an empty subscriber set.
    pub fn new() -> Self {
        Self {
            senders: Vec::new(),
        }
    }

    /// Adds a new subscriber sender. The matching receiver should be returned
    /// to the caller so it can be wrapped in a `ReceiverStream`.
    pub fn push(&mut self, sender: mpsc::Sender<Result<T, Status>>) {
        self.senders.push(sender);
    }

    /// Number of currently connected subscribers. Test-only helper; the
    /// broadcast path auto-prunes closed senders on each call so runtime
    /// code does not need to query size.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.senders.len()
    }

    /// Returns `true` when no subscribers are connected. Test-only helper;
    /// the `AttachSession` path relies on retain-pruning inside
    /// [`Self::broadcast`] rather than explicit checks.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.senders.is_empty()
    }

    /// Sends `value` to every subscriber via `try_send`. Senders whose
    /// receiver has been dropped are pruned from the set; senders whose
    /// channel is full are kept — the message is dropped to avoid blocking
    /// the broadcast loop.
    pub fn broadcast(&mut self, value: T) {
        self.senders.retain(|tx| match tx.try_send(Ok(value.clone())) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Closed(_)) => false,
        });
    }
}

#[cfg(test)]
mod tests;
