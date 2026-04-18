//! Per-session agent orchestration.
//!
//! Owns the lifecycles of main and sub-agent processes (spawn, bidi
//! stream, routing, cascade kills). Agents here are purely
//! message-emitters — they cannot mutate session state beyond the
//! nodes they create / patch through the stream handler.

/// Routing and dispatch logic that drains the per-session queue.
pub mod routing;
/// Spawning of agent processes (main + sub).
pub mod spawn;
/// Bidi `AgentStream` handler.
pub mod stream;

use std::collections::HashMap;

use scarllet_proto::proto::{AgentInbound, NodeKind, ResultPayload};
use tokio::sync::{mpsc, oneshot};
use tonic::Status;

use crate::session::nodes::NodeStore;

/// Per-process record stored in [`AgentRegistry`] for a connected agent.
///
/// `tx` is the channel core uses to push `AgentInbound` messages (e.g.
/// `AgentTask`, `CancelNow`) back to the agent. `agent_node_id` points to
/// the `Agent` node that core created for this turn so updates can target
/// it directly.
pub struct AgentRecord {
    /// Stable id assigned by core when the turn was dispatched.
    pub agent_id: String,
    /// Manifest name of the agent module (e.g. `"default"`).
    pub agent_module: String,
    /// `session_id` for main agents; calling agent's id for sub-agents.
    pub parent_id: String,
    /// Operating-system process id (when the spawn call returned one).
    pub pid: Option<u32>,
    /// Sender used by core to push `AgentInbound` messages to the agent.
    pub tx: mpsc::Sender<Result<AgentInbound, Status>>,
    /// Id of the `Agent` node core created for this turn.
    pub agent_node_id: String,
}

/// Per-session collection of connected agents.
///
/// Tracks main agents (`parent_id == session_id`) alongside sub-agents
/// (`parent_id == <parent agent id>`). At most one main agent may be
/// registered at a time; sub-agents are registered one per active
/// `spawn_sub_agent` call and removed when the sub-agent's `TurnFinished`
/// (or failure / disconnect) is observed.
pub struct AgentRegistry {
    by_id: HashMap<String, AgentRecord>,
    main_agent_id: Option<String>,
    /// One-shot waiters keyed by the sub-agent's `agent_id`. The
    /// `spawn_sub_agent` tool parks the parent's `InvokeTool` call on the
    /// receiver half; the stream handler fires the sender when the
    /// sub-agent's `TurnFinished` / failure / disconnect is observed.
    sub_agent_waiters: HashMap<String, oneshot::Sender<Result<ResultPayload, String>>>,
}

impl Default for AgentRegistry {
    /// Empty registry; identical to [`AgentRegistry::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    /// Initialises an empty per-session agent registry.
    pub fn new() -> Self {
        Self {
            by_id: HashMap::new(),
            main_agent_id: None,
            sub_agent_waiters: HashMap::new(),
        }
    }

    /// Inserts `record`. If `record.parent_id` matches `session_id` the
    /// record is also flagged as the session's main agent; the caller must
    /// have already verified `has_main` returned `false` (the bidi handler
    /// does this before calling `register`).
    pub fn register(&mut self, session_id: &str, record: AgentRecord) {
        if record.parent_id == session_id {
            self.main_agent_id = Some(record.agent_id.clone());
        }
        self.by_id.insert(record.agent_id.clone(), record);
    }

    /// Removes the agent with the given id (if any). Clears the
    /// `main_agent_id` slot when removing the active main agent.
    pub fn deregister(&mut self, agent_id: &str) -> Option<AgentRecord> {
        let removed = self.by_id.remove(agent_id);
        if self.main_agent_id.as_deref() == Some(agent_id) {
            self.main_agent_id = None;
        }
        removed
    }

    /// Returns a reference to the record if it exists.
    pub fn get(&self, agent_id: &str) -> Option<&AgentRecord> {
        self.by_id.get(agent_id)
    }

    /// Patches the PID of an already-registered record. Used by
    /// `handle_spawn_sub_agent` to propagate the OS PID onto the record
    /// when the sub-agent registered before the spawn call returned
    /// (rare, but possible under fast startup).
    pub fn set_pid(&mut self, agent_id: &str, pid: Option<u32>) {
        if let Some(rec) = self.by_id.get_mut(agent_id) {
            rec.pid = pid;
        }
    }

    /// `true` when a main agent is currently dispatched.
    pub fn has_main(&self) -> bool {
        self.main_agent_id.is_some()
    }

    /// Iterates every record (main + sub).
    pub fn iter_records(&self) -> impl Iterator<Item = &AgentRecord> {
        self.by_id.values()
    }

    /// Registers a oneshot waiter for a freshly-spawned sub-agent. The
    /// receiver half is held by the in-flight `spawn_sub_agent` invocation;
    /// the stream handler fires this sender when the matching sub-agent's
    /// `TurnFinished` / failure / disconnect is observed.
    pub fn register_sub_agent_waiter(
        &mut self,
        child_agent_id: String,
        tx: oneshot::Sender<Result<ResultPayload, String>>,
    ) {
        self.sub_agent_waiters.insert(child_agent_id, tx);
    }

    /// Removes and returns the waiter for `child_agent_id` if one was
    /// registered. Returns `None` when the sub-agent was not spawned via
    /// `spawn_sub_agent` (e.g. a main agent).
    pub fn take_sub_agent_waiter(
        &mut self,
        child_agent_id: &str,
    ) -> Option<oneshot::Sender<Result<ResultPayload, String>>> {
        self.sub_agent_waiters.remove(child_agent_id)
    }

    /// `true` when the given agent id corresponds to a registered
    /// sub-agent waiter (i.e. the agent was spawned via `spawn_sub_agent`
    /// and has not yet emitted `TurnFinished` / failed / disconnected).
    pub fn has_sub_agent_waiter(&self, child_agent_id: &str) -> bool {
        self.sub_agent_waiters.contains_key(child_agent_id)
    }

    /// Walks the node subtree rooted at `parent_agent_id`'s Agent node and
    /// returns `true` if any Agent-kind descendant corresponds to a
    /// currently-registered agent.
    ///
    /// Used by the `TurnFinished` handler to enforce AC-8.4 — a parent may
    /// not finish while any of its sub-agents are still running.
    pub fn any_descendant_running(&self, parent_agent_id: &str, nodes: &NodeStore) -> bool {
        let Some(parent_record) = self.by_id.get(parent_agent_id) else {
            return false;
        };
        let mut stack = vec![parent_record.agent_node_id.clone()];
        while let Some(node_id) = stack.pop() {
            let Some(children) = nodes.children_of.get(&node_id) else {
                continue;
            };
            for child_id in children {
                let Some(child) = nodes.by_id.get(child_id) else {
                    continue;
                };
                let kind = NodeKind::try_from(child.kind).unwrap_or(NodeKind::Unspecified);
                if matches!(kind, NodeKind::Agent) && self.by_id.contains_key(child_id) {
                    return true;
                }
                stack.push(child_id.clone());
            }
        }
        false
    }

    /// Collects the agent ids of every Agent-kind descendant of
    /// `parent_agent_id`'s Agent node that is currently registered. Used
    /// by the cascade-kill path when the AC-8.4 invariant is violated.
    pub fn descendant_agent_ids(&self, parent_agent_id: &str, nodes: &NodeStore) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let Some(parent_record) = self.by_id.get(parent_agent_id) else {
            return found;
        };
        let mut stack = vec![parent_record.agent_node_id.clone()];
        while let Some(node_id) = stack.pop() {
            let Some(children) = nodes.children_of.get(&node_id) else {
                continue;
            };
            for child_id in children {
                let Some(child) = nodes.by_id.get(child_id) else {
                    continue;
                };
                let kind = NodeKind::try_from(child.kind).unwrap_or(NodeKind::Unspecified);
                if matches!(kind, NodeKind::Agent) && self.by_id.contains_key(child_id) {
                    found.push(child_id.clone());
                }
                stack.push(child_id.clone());
            }
        }
        found
    }
}

#[cfg(test)]
mod tests;
