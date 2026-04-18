//! Append-only typed-node graph for a single session.
//!
//! Enforces AC-5.4 parent-kind rules and merges partial [`NodePatch`]es
//! onto stored nodes so callers can broadcast `NodeCreated` /
//! `NodeUpdated` diffs with minimal allocation.

use std::collections::HashMap;

use scarllet_proto::proto::{node, Node, NodeKind, NodePatch};

/// Reasons a `NodeStore::create` / `update` call can be rejected.
///
/// Returned by [`NodeStore`] mutators when the requested change would
/// violate the per-session node-graph invariants from the architecture
/// (parent rules per AC-5.4 and id uniqueness).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantError {
    /// The supplied node id already exists in the store.
    DuplicateId(String),
    /// `update` was called with a node id that is not in the store.
    UnknownNode(String),
    /// The node has an unrecognised / unspecified kind.
    UnsupportedKind,
    /// The supplied `parent_id` does not match any existing node.
    UnknownParent(String),
    /// The kind requires a parent of a specific kind, but the supplied
    /// parent was a different kind.
    InvalidParentKind {
        /// Kind of the node being created / updated.
        child: NodeKind,
        /// Kind required by the parent rule.
        expected_parent: NodeKind,
        /// Actual kind of the supplied parent.
        actual_parent: NodeKind,
    },
    /// The kind requires a parent but the request asked for a top-level node.
    ParentRequired(NodeKind),
    /// The supplied node had no parent, but the kind never allows top-level.
    TopLevelNotAllowed(NodeKind),
}

/// Per-session append-only node graph.
///
/// Stores nodes in insertion order so the `SessionState` snapshot, the
/// `NodeCreated` diffs, and TUI rendering can all walk the same canonical
/// sequence. Parent / child links are mirrored in `children_of` for fast
/// subtree walks.
pub struct NodeStore {
    /// Node ids in creation order.
    pub order: Vec<String>,
    /// Lookup table from node id to the full `Node` proto.
    pub by_id: HashMap<String, Node>,
    /// Reverse-lookup: for each parent id, the ids of its direct children.
    pub children_of: HashMap<String, Vec<String>>,
}

impl Default for NodeStore {
    /// Empty store; identical to [`NodeStore::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl NodeStore {
    /// Initialises an empty node store.
    pub fn new() -> Self {
        Self {
            order: Vec::new(),
            by_id: HashMap::new(),
            children_of: HashMap::new(),
        }
    }

    /// Inserts `node` after validating id uniqueness and parent-kind rules
    /// (AC-5.4). Returns the stored node on success.
    pub fn create(&mut self, node: Node) -> Result<&Node, InvariantError> {
        let id = node.id.clone();
        if id.is_empty() {
            return Err(InvariantError::UnsupportedKind);
        }
        if self.by_id.contains_key(&id) {
            return Err(InvariantError::DuplicateId(id));
        }
        let kind = NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified);
        if matches!(kind, NodeKind::Unspecified) {
            return Err(InvariantError::UnsupportedKind);
        }
        validate_parent(self, kind, node.parent_id.as_deref())?;

        if let Some(parent_id) = node.parent_id.as_ref() {
            self.children_of
                .entry(parent_id.clone())
                .or_default()
                .push(id.clone());
        }
        self.order.push(id.clone());
        self.by_id.insert(id.clone(), node);
        Ok(self.by_id.get(&id).expect("just inserted"))
    }

    /// Merges a [`NodePatch`] into the node identified by `id`.
    ///
    /// Per the architecture's locked merge rules (`thought_content` and
    /// `result_content` APPEND to the existing payload string; everything
    /// else REPLACES the field). `updated_at` is set on the stored node so
    /// callers can broadcast the matching `NodeUpdated` diff with the same
    /// timestamp.
    ///
    /// Returns `InvariantError::UnknownNode` when `id` is not present.
    pub fn update(
        &mut self,
        id: &str,
        patch: NodePatch,
        updated_at: u64,
    ) -> Result<&Node, InvariantError> {
        let Some(node) = self.by_id.get_mut(id) else {
            return Err(InvariantError::UnknownNode(id.to_string()));
        };
        apply_patch(node, patch);
        node.updated_at = updated_at;
        Ok(&*node)
    }

    /// Returns the node with the given id, if any.
    pub fn get(&self, id: &str) -> Option<&Node> {
        self.by_id.get(id)
    }

    /// Iterates every node in creation order.
    pub fn all(&self) -> impl Iterator<Item = &Node> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// Returns a flat snapshot of every node in creation order, used by the
    /// `SessionState` builder.
    pub fn snapshot(&self) -> Vec<Node> {
        self.all().cloned().collect()
    }

    /// Number of nodes currently stored. Test-only helper; production paths
    /// iterate via [`NodeStore::all`] or [`NodeStore::snapshot`] instead.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// `true` when the store is empty. Test-only helper; production paths
    /// do not branch on emptiness — they walk the node graph directly.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Enforces AC-5.4: User / Agent / Error MAY be top-level; Thought / Tool /
/// Result / Debug / TokenUsage MUST parent onto an Agent node. Sub-agent
/// `Agent` nodes are allowed to parent onto a `Tool` node (effort 5);
/// session-level `Error` nodes may also be Agent-parented (per-turn errors).
fn validate_parent(
    store: &NodeStore,
    kind: NodeKind,
    parent_id: Option<&str>,
) -> Result<(), InvariantError> {
    match (kind, parent_id) {
        (NodeKind::Unspecified, _) => Err(InvariantError::UnsupportedKind),

        (NodeKind::User, None) => Ok(()),
        (NodeKind::User, Some(_)) => Err(InvariantError::TopLevelNotAllowed(NodeKind::User)),

        (NodeKind::Agent, None) => Ok(()),
        (NodeKind::Agent, Some(parent)) => require_parent_kind(store, parent, NodeKind::Agent, NodeKind::Tool),

        (NodeKind::Error, None) => Ok(()),
        (NodeKind::Error, Some(parent)) => require_parent_kind(store, parent, NodeKind::Error, NodeKind::Agent),

        (
            kind @ (NodeKind::Thought
            | NodeKind::Tool
            | NodeKind::Result
            | NodeKind::Debug
            | NodeKind::TokenUsage),
            None,
        ) => Err(InvariantError::ParentRequired(kind)),
        (
            kind @ (NodeKind::Thought
            | NodeKind::Tool
            | NodeKind::Result
            | NodeKind::Debug
            | NodeKind::TokenUsage),
            Some(parent),
        ) => require_parent_kind(store, parent, kind, NodeKind::Agent),
    }
}

/// Applies the per-field merge rules from the architecture: `thought_content`
/// and `result_content` are appended to the existing payload string;
/// everything else replaces the matching field. Patch fields whose target
/// payload variant doesn't match the node's actual kind are silently ignored
/// — the agent stream layer is responsible for refusing nonsensical patches
/// before this is reached.
fn apply_patch(node: &mut Node, patch: NodePatch) {
    let Some(payload) = node.payload.as_mut() else {
        return;
    };
    match payload {
        node::Payload::Agent(agent) => {
            if let Some(status) = patch.agent_status {
                agent.status = status;
            }
        }
        node::Payload::Thought(thought) => {
            if let Some(chunk) = patch.thought_content {
                thought.content.push_str(&chunk);
            }
        }
        node::Payload::Tool(tool) => {
            if let Some(status) = patch.tool_status {
                tool.status = status;
            }
            if let Some(duration) = patch.tool_duration_ms {
                tool.duration_ms = duration;
            }
            if let Some(result_json) = patch.tool_result_json {
                tool.result_json = result_json;
            }
        }
        node::Payload::Result(result) => {
            if let Some(chunk) = patch.result_content {
                result.content.push_str(&chunk);
            }
            if let Some(reason) = patch.result_finish_reason {
                result.finish_reason = reason;
            }
        }
        node::Payload::Error(error) => {
            if let Some(message) = patch.error_message {
                error.message = message;
            }
        }
        node::Payload::TokenUsage(usage) => {
            if let Some(total) = patch.token_total {
                usage.total_tokens = total;
            }
            if let Some(window) = patch.token_window {
                usage.context_window = window;
            }
        }
        node::Payload::User(_) | node::Payload::Debug(_) => {
            // No patchable fields in effort 02.
        }
    }
}

/// Looks up `parent_id` in `store` and verifies its kind matches `expected`.
fn require_parent_kind(
    store: &NodeStore,
    parent_id: &str,
    child: NodeKind,
    expected: NodeKind,
) -> Result<(), InvariantError> {
    let Some(parent) = store.by_id.get(parent_id) else {
        return Err(InvariantError::UnknownParent(parent_id.to_string()));
    };
    let actual = NodeKind::try_from(parent.kind).unwrap_or(NodeKind::Unspecified);
    if actual != expected {
        return Err(InvariantError::InvalidParentKind {
            child,
            expected_parent: expected,
            actual_parent: actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
