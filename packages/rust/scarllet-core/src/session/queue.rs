//! FIFO queue of pending user prompts for one session.
//!
//! Wraps a [`VecDeque`] of [`QueuedPrompt`] protos so the same
//! representation round-trips through `QueueChanged` broadcasts and
//! `SessionState` snapshots without intermediate conversion.

use std::collections::VecDeque;

use scarllet_proto::proto::QueuedPrompt;

/// Per-session FIFO buffer of queued user prompts.
///
/// Wraps a [`VecDeque`] of [`QueuedPrompt`] proto messages so the same
/// representation can be broadcast in `QueueChanged` diffs and snapshotted
/// into `SessionState` without conversion.
pub struct SessionQueue {
    items: VecDeque<QueuedPrompt>,
}

impl Default for SessionQueue {
    /// Empty queue; identical to [`SessionQueue::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl SessionQueue {
    /// Initialises an empty queue.
    pub fn new() -> Self {
        Self {
            items: VecDeque::new(),
        }
    }

    /// Appends a prompt to the back of the queue.
    pub fn push_back(&mut self, prompt: QueuedPrompt) {
        self.items.push_back(prompt);
    }

    /// Removes and returns the next prompt, or `None` if empty.
    pub fn pop_front(&mut self) -> Option<QueuedPrompt> {
        self.items.pop_front()
    }

    /// Returns the current queue length. Test-only helper; production
    /// callers rely on the queue-changed broadcast for observability.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` when no prompts are queued. Used by
    /// [`crate::agents::routing::try_dispatch_main_with`] to short-circuit
    /// dispatch when the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns a `Vec` snapshot of the queue, used when broadcasting
    /// `QueueChanged` diffs and building `SessionState`.
    pub fn snapshot(&self) -> Vec<QueuedPrompt> {
        self.items.iter().cloned().collect()
    }

    /// Empties the queue.
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests;
