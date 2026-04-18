//! Snapshot builders for the `Attached` first diff and `GetSessionState`.
//!
//! Also derives the [`HistoryEntry`] list consumed by agents through
//! `GetConversationHistory` so history stays a function of the node
//! graph rather than a second source of truth.

use std::time::{SystemTime, UNIX_EPOCH};

use scarllet_proto::proto::{
    node, ActiveProviderResponse, AgentSummary, HistoryEntry, NodeKind, SessionState,
};
use scarllet_sdk::config::{Provider, ProviderType};

use super::nodes::NodeStore;
use super::Session;

/// Builds a full `SessionState` snapshot for `Attached` and `GetSessionState`.
///
/// The snapshot is fully-owned (clones nodes / queue / agent summaries) so
/// the caller can release the session lock immediately after building it.
pub fn snapshot(session: &Session) -> SessionState {
    let agents = session
        .agents
        .iter_records()
        .map(|rec| AgentSummary {
            agent_id: rec.agent_id.clone(),
            agent_module: rec.agent_module.clone(),
            parent_id: rec.parent_id.clone(),
            agent_node_id: rec.agent_node_id.clone(),
        })
        .collect();

    SessionState {
        session_id: session.id.clone(),
        status: super::diff::status_str(session.status).to_string(),
        provider: Some(provider_response(session.config.provider.as_ref())),
        queue: session.queue.snapshot(),
        nodes: session.nodes.snapshot(),
        agents,
        created_at: to_unix_secs(session.created_at),
        last_activity: to_unix_secs(session.last_activity),
    }
}

/// Renders a session's snapshotted [`Provider`] into the wire-level
/// [`ActiveProviderResponse`]. Used both by `GetActiveProvider` and the
/// initial `SessionState` snapshot.
pub fn provider_response(provider: Option<&Provider>) -> ActiveProviderResponse {
    let Some(p) = provider else {
        return ActiveProviderResponse {
            configured: false,
            ..Default::default()
        };
    };
    ActiveProviderResponse {
        configured: true,
        provider_name: p.name.clone(),
        provider_type: provider_type_str(&p.provider_type).to_string(),
        api_url: p.api_url.clone().unwrap_or_default(),
        api_key: p.api_key.clone(),
        model: p.model.clone(),
        reasoning_effort: p.reasoning_effort().unwrap_or_default().to_string(),
    }
}

/// Maps the SDK provider enum onto the canonical wire string.
fn provider_type_str(kind: &ProviderType) -> &'static str {
    match kind {
        ProviderType::Openai => "openai",
        ProviderType::Gemini => "gemini",
    }
}

/// Derives a chronological chat history from the per-session node graph.
///
/// Walks every node in creation order and emits one [`HistoryEntry`] per
/// turn-relevant payload:
///
/// - top-level `User` node — `{ role: "user", content: text }`
/// - top-level `Agent` node — for each terminal `Tool` child (in creation
///   order) emits two entries:
///     1. `{ role: "assistant", content: "", tool_calls_json: Some([...]) }`
///        carrying the LLM-shaped tool call (the SDK adapter parses this
///        into `ChatMessage::tool_calls`).
///     2. `{ role: "tool", content: result_json, tool_call_id: Some(...) }`
///        with the `Tool` node id used as the stable call id (no extra
///        proto field needed; the id is already unique).
///
///   Then, if the Agent subtree has a `Result`, emits the final
///   `{ role: "assistant", content: result.content }`.
///
/// Tool nodes whose status is not `done` / `failed` are skipped (the call
/// is still in flight), and Agent turns without any of the above are
/// silently omitted so the LLM never sees partial state.
///
/// **Schema choice**: the optional `tool_call_id` / `tool_calls_json`
/// fields on [`HistoryEntry`] (proto-side) are populated here. The
/// alternative — encoding the tool call as a JSON blob on a synthetic
/// `assistant-tool-call` role string — was rejected because the SDK
/// adapter would have to invent a custom role and re-parse JSON content;
/// the optional fields keep the role enum stable (`user|assistant|tool|
/// system`) and let the SDK build `ChatMessage` directly.
pub fn conversation_history(nodes: &NodeStore) -> Vec<HistoryEntry> {
    let mut entries: Vec<HistoryEntry> = Vec::new();
    for node in nodes.all() {
        if node.parent_id.is_some() {
            continue;
        }
        let kind = NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified);
        match (kind, node.payload.as_ref()) {
            (NodeKind::User, Some(node::Payload::User(user))) => {
                entries.push(HistoryEntry {
                    role: "user".into(),
                    content: user.text.clone(),
                    tool_call_id: None,
                    tool_calls_json: None,
                });
            }
            (NodeKind::Agent, _) => {
                append_agent_turn_entries(nodes, &node.id, &mut entries);
            }
            _ => {}
        }
    }
    entries
}

/// Walks the children of `agent_node_id` in creation order and appends the
/// derived [`HistoryEntry`] items (tool call / tool result pairs followed
/// by the final assistant Result, if any).
fn append_agent_turn_entries(
    nodes: &NodeStore,
    agent_node_id: &str,
    entries: &mut Vec<HistoryEntry>,
) {
    for child in nodes.all() {
        if child.parent_id.as_deref() != Some(agent_node_id) {
            continue;
        }
        let kind = NodeKind::try_from(child.kind).unwrap_or(NodeKind::Unspecified);
        match (kind, child.payload.as_ref()) {
            (NodeKind::Tool, Some(node::Payload::Tool(tool))) => {
                if !is_terminal_tool_status(&tool.status) {
                    continue;
                }
                let call_id = child.id.clone();
                if let Some(tool_calls_json) = build_tool_calls_json(&call_id, tool) {
                    entries.push(HistoryEntry {
                        role: "assistant".into(),
                        content: String::new(),
                        tool_call_id: None,
                        tool_calls_json: Some(tool_calls_json),
                    });
                }
                let content = if tool.result_json.is_empty() {
                    String::new()
                } else {
                    tool.result_json.clone()
                };
                entries.push(HistoryEntry {
                    role: "tool".into(),
                    content,
                    tool_call_id: Some(call_id),
                    tool_calls_json: None,
                });
            }
            (NodeKind::Result, Some(node::Payload::Result(r))) => {
                entries.push(HistoryEntry {
                    role: "assistant".into(),
                    content: r.content.clone(),
                    tool_call_id: None,
                    tool_calls_json: None,
                });
            }
            _ => {}
        }
    }
}

/// `true` for `Tool` statuses that mean the call has finished and its
/// payload is safe to replay (i.e. not `pending` / `running`).
fn is_terminal_tool_status(status: &str) -> bool {
    matches!(status, "done" | "failed")
}

/// Encodes a single tool invocation in the LLM-compatible JSON shape the
/// SDK adapter expects when building `ChatMessage::tool_calls`.
///
/// Returns `None` if `arguments_json` is empty (the call never carried any
/// arguments — extremely unlikely in practice but guards against emitting
/// malformed JSON).
fn build_tool_calls_json(
    call_id: &str,
    tool: &scarllet_proto::proto::ToolPayload,
) -> Option<String> {
    let arguments = if tool.arguments_json.is_empty() {
        "{}".to_string()
    } else {
        tool.arguments_json.clone()
    };
    let value = serde_json::json!([{
        "id": call_id,
        "type": "function",
        "function": {
            "name": tool.tool_name,
            "arguments": arguments,
        }
    }]);
    Some(value.to_string())
}

/// Converts a [`SystemTime`] to seconds since the Unix epoch (saturating).
fn to_unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
