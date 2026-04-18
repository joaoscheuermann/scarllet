//! Agent-process spawning (main + sub-agents) and the sub-agent
//! `InvokeTool` bridge.
//!
//! [`spawn_main_agent`] and [`spawn_sub_agent_process`] launch the agent
//! binaries detected by the module watcher; [`handle_spawn_sub_agent`]
//! implements the core-internal `spawn_sub_agent` tool branch, parking
//! the parent's `InvokeTool` call on a `oneshot::Sender` until the
//! sub-agent finishes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use scarllet_proto::proto::{node, AgentPayload, Node, NodeKind, QueuedPrompt};
use scarllet_sdk::manifest::ModuleKind;
use tokio::sync::{oneshot, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

use crate::agents::routing::PendingDispatch;
use crate::registry::ModuleRegistry;
use crate::session::{diff, SessionRegistry};
use crate::tools::ToolResult;

/// Spawns the agent binary at `module_path` as a child process and returns
/// the operating-system PID on success.
///
/// Sets the canonical agent env vars so the in-process SDK helper
/// (`scarllet_sdk::agent::AgentSession::connect`) can dial back to core,
/// register, and receive its `AgentTask`. The child is detached: stdout /
/// stderr are inherited so panics surface in the core's terminal during
/// development.
pub fn spawn_main_agent(
    module_path: &Path,
    core_addr: &str,
    session_id: &str,
    agent_id: &str,
    agent_module: &str,
    working_directory: &str,
) -> Option<u32> {
    spawn_agent_process(
        module_path,
        core_addr,
        session_id,
        agent_id,
        session_id,
        agent_module,
        working_directory,
    )
}

/// Spawns a sub-agent binary with `SCARLLET_PARENT_ID` set to the calling
/// agent's id, so the sub-agent's `Register` message carries the right
/// parent pointer for the core-side validation and node-tree wiring.
pub fn spawn_sub_agent_process(
    module_path: &Path,
    core_addr: &str,
    session_id: &str,
    agent_id: &str,
    parent_agent_id: &str,
    agent_module: &str,
    working_directory: &str,
) -> Option<u32> {
    spawn_agent_process(
        module_path,
        core_addr,
        session_id,
        agent_id,
        parent_agent_id,
        agent_module,
        working_directory,
    )
}

/// Shared spawn helper used by [`spawn_main_agent`] and
/// [`spawn_sub_agent_process`]. Only `parent_id` differs between the two
/// entry points (session_id for main, calling agent_id for sub).
fn spawn_agent_process(
    module_path: &Path,
    core_addr: &str,
    session_id: &str,
    agent_id: &str,
    parent_id: &str,
    agent_module: &str,
    working_directory: &str,
) -> Option<u32> {
    let child = std::process::Command::new(module_path)
        .env("SCARLLET_CORE_ADDR", core_addr)
        .env("SCARLLET_SESSION_ID", session_id)
        .env("SCARLLET_AGENT_ID", agent_id)
        .env("SCARLLET_PARENT_ID", parent_id)
        .env("SCARLLET_AGENT_MODULE", agent_module)
        .current_dir(if working_directory.is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            std::path::PathBuf::from(working_directory)
        })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn();

    match child {
        Ok(c) => {
            let pid = c.id();
            info!(
                "Spawned agent '{agent_module}' as pid {pid} (session {session_id}, parent {parent_id})"
            );
            Some(pid)
        }
        Err(e) => {
            warn!(
                "Failed to spawn agent '{agent_module}' from {}: {e}",
                module_path.display()
            );
            None
        }
    }
}

/// Arguments handed to the spawn callback used by
/// [`handle_spawn_sub_agent_with`]. Kept as a struct so unit tests can
/// observe every spawn attempt without mirroring the argument list.
#[derive(Debug, Clone)]
pub struct SubAgentSpawnArgs<'a> {
    /// On-disk path to the sub-agent's binary.
    pub module_path: &'a Path,
    /// Loopback gRPC address the spawned process should dial.
    pub core_addr: &'a str,
    /// Owning session id (becomes `SCARLLET_SESSION_ID`).
    pub session_id: &'a str,
    /// Core-assigned id for the sub-agent (`SCARLLET_AGENT_ID`).
    pub child_agent_id: &'a str,
    /// Parent agent id (`SCARLLET_PARENT_ID`).
    pub parent_agent_id: &'a str,
    /// Manifest name of the sub-agent module (`SCARLLET_AGENT_MODULE`).
    pub agent_module: &'a str,
    /// Working directory inherited from the parent (falls back to empty).
    pub working_directory: &'a str,
}

/// Top-level payload expected from the parent agent when it invokes
/// `spawn_sub_agent`. Parse failures return a structured `ToolResult`
/// error so the parent's LLM sees an actionable message. We use
/// `serde_json::Value` rather than a derived struct because
/// `scarllet-core` does not depend on `serde` directly.
struct SpawnSubAgentInput {
    agent_module: String,
    prompt: String,
}

impl SpawnSubAgentInput {
    /// Parses the raw `input_json` a parent agent sent via `InvokeTool`.
    /// Extra fields (e.g. `working_directory` injected by the default
    /// agent) are ignored.
    fn parse(input_json: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(input_json)
            .map_err(|e| format!("invalid spawn_sub_agent args: {e}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| "invalid spawn_sub_agent args: expected JSON object".to_string())?;
        let agent_module = obj
            .get("agent_module")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "invalid spawn_sub_agent args: `agent_module` missing or not a string".to_string()
            })?
            .to_string();
        let prompt = obj
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "invalid spawn_sub_agent args: `prompt` missing or not a string".to_string()
            })?
            .to_string();
        Ok(Self {
            agent_module,
            prompt,
        })
    }
}

/// Handles a `spawn_sub_agent` tool call by creating the nested Agent
/// node, spawning the sub-agent process, parking on a oneshot until the
/// sub-agent emits its `Result`, and mapping the outcome into a
/// [`ToolResult`] ready to return through `InvokeToolResponse`.
///
/// Production callers use this entry point. Unit tests drive
/// [`handle_spawn_sub_agent_with`] directly with a fake spawn callback
/// so they can exercise the happy / failure branches without a real
/// child process.
pub async fn handle_spawn_sub_agent(
    sessions: &Arc<RwLock<SessionRegistry>>,
    registry: &Arc<RwLock<ModuleRegistry>>,
    core_addr: &str,
    session_id: &str,
    parent_agent_id: &str,
    input_json: &str,
) -> ToolResult {
    handle_spawn_sub_agent_with(
        sessions,
        registry,
        core_addr,
        session_id,
        parent_agent_id,
        input_json,
        |args| {
            spawn_sub_agent_process(
                args.module_path,
                args.core_addr,
                args.session_id,
                args.child_agent_id,
                args.parent_agent_id,
                args.agent_module,
                args.working_directory,
            )
        },
    )
    .await
}

/// Test seam for [`handle_spawn_sub_agent`]: same control flow but the
/// spawn step is delegated to `spawn_fn`. Returning `None` from `spawn_fn`
/// is treated as "best-effort spawn failed"; the function still awaits
/// the oneshot so tests can drive the failure path by firing `Err(...)`.
pub async fn handle_spawn_sub_agent_with<F>(
    sessions: &Arc<RwLock<SessionRegistry>>,
    registry: &Arc<RwLock<ModuleRegistry>>,
    core_addr: &str,
    session_id: &str,
    parent_agent_id: &str,
    input_json: &str,
    spawn_fn: F,
) -> ToolResult
where
    F: FnOnce(SubAgentSpawnArgs<'_>) -> Option<u32>,
{
    let start = std::time::Instant::now();

    let input = match SpawnSubAgentInput::parse(input_json) {
        Ok(v) => v,
        Err(msg) => {
            return failure(msg, start.elapsed().as_millis() as u64);
        }
    };
    if input.agent_module.is_empty() {
        return failure(
            "invalid spawn_sub_agent args: `agent_module` is empty".into(),
            start.elapsed().as_millis() as u64,
        );
    }
    if input.prompt.is_empty() {
        return failure(
            "invalid spawn_sub_agent args: `prompt` is empty".into(),
            start.elapsed().as_millis() as u64,
        );
    }

    let module_path = resolve_agent_module(registry, &input.agent_module).await;
    let Some(module_path) = module_path else {
        return failure(
            format!("agent module '{}' not registered", input.agent_module),
            start.elapsed().as_millis() as u64,
        );
    };

    let Some(handle) = ({
        let guard = sessions.read().await;
        guard.get(session_id)
    }) else {
        return failure(
            format!("session '{session_id}' not found"),
            start.elapsed().as_millis() as u64,
        );
    };

    let (tx, rx) = oneshot::channel::<Result<scarllet_proto::proto::ResultPayload, String>>();
    let child_agent_id = Uuid::new_v4().to_string();
    let working_directory = String::new();

    {
        let mut session = handle.write().await;

        let parent_tool_node_id =
            match find_parent_tool_node_id(&session.nodes, parent_agent_id, &input) {
                Some(id) => id,
                None => {
                    return failure(
                        format!(
                            "parent agent '{parent_agent_id}' has no pending spawn_sub_agent Tool node"
                        ),
                        start.elapsed().as_millis() as u64,
                    );
                }
            };

        let agent_node = build_sub_agent_node(
            &child_agent_id,
            &parent_tool_node_id,
            &input.agent_module,
        );
        match session.nodes.create(agent_node) {
            Ok(stored) => {
                let cloned = stored.clone();
                session.broadcast(diff::node_created(cloned));
            }
            Err(err) => {
                return failure(
                    format!("failed to create sub-agent node: {err:?}"),
                    start.elapsed().as_millis() as u64,
                );
            }
        };

        session
            .agents
            .register_sub_agent_waiter(child_agent_id.clone(), tx);

        let queued = QueuedPrompt {
            prompt_id: Uuid::new_v4().to_string(),
            text: input.prompt.clone(),
            working_directory: working_directory.clone(),
            user_node_id: parent_tool_node_id.clone(),
        };
        session.pending_dispatch.insert(
            child_agent_id.clone(),
            PendingDispatch {
                prompt: queued,
                pid: None,
            },
        );
    }

    let pid = spawn_fn(SubAgentSpawnArgs {
        module_path: &module_path,
        core_addr,
        session_id,
        child_agent_id: &child_agent_id,
        parent_agent_id,
        agent_module: &input.agent_module,
        working_directory: &working_directory,
    });

    if pid.is_some() {
        // Best-effort PID propagation: if the sub-agent has not yet
        // registered, patch the pending_dispatch entry so the PID lands on
        // its AgentRecord at register time. If it already registered,
        // patch the record directly. If it already finished, do nothing.
        let mut session = handle.write().await;
        if let Some(pending) = session.pending_dispatch.get_mut(&child_agent_id) {
            pending.pid = pid;
        } else {
            session.agents.set_pid(&child_agent_id, pid);
        }
    }

    match rx.await {
        Ok(Ok(payload)) => {
            let output = serde_json::json!({
                "content": payload.content,
                "finish_reason": payload.finish_reason,
            })
            .to_string();
            ToolResult {
                success: true,
                output_json: output,
                error_message: String::new(),
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }
        Ok(Err(msg)) => failure(msg, start.elapsed().as_millis() as u64),
        Err(_) => failure(
            "sub-agent terminated unexpectedly".into(),
            start.elapsed().as_millis() as u64,
        ),
    }
}

/// Builds a failure [`ToolResult`] with empty `output_json`. Centralised
/// so every early-return in the spawn path emits the same shape.
fn failure(message: String, duration_ms: u64) -> ToolResult {
    ToolResult {
        success: false,
        output_json: String::new(),
        error_message: message,
        duration_ms,
    }
}

/// Finds the `Tool` node the parent agent created immediately before
/// calling `spawn_sub_agent`. Walks the node store in reverse creation
/// order looking for a Tool child of the parent's Agent node whose
/// `tool_name == "spawn_sub_agent"` and whose `arguments_json` matches.
///
/// Falls back to the most recent `spawn_sub_agent` Tool node if no exact
/// `arguments_json` match is found — this is defensive: the parent's
/// `arguments_json` may have been normalised differently on the way
/// through (e.g. the LLM emits `{ "prompt": "…", "agent_module": "…" }`
/// and we parse it into `SpawnSubAgentInput` without preserving the
/// original key order).
fn find_parent_tool_node_id(
    nodes: &crate::session::nodes::NodeStore,
    parent_agent_id: &str,
    input: &SpawnSubAgentInput,
) -> Option<String> {
    let mut latest: Option<String> = None;
    for node in nodes.all() {
        if node.parent_id.as_deref() != Some(parent_agent_id) {
            continue;
        }
        let kind = NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified);
        if !matches!(kind, NodeKind::Tool) {
            continue;
        }
        let Some(node::Payload::Tool(payload)) = node.payload.as_ref() else {
            continue;
        };
        if payload.tool_name != crate::tools::SPAWN_SUB_AGENT_TOOL {
            continue;
        }
        // Prefer the argument-match, but track the most recent spawn Tool as a fallback.
        if arguments_match(&payload.arguments_json, input) {
            return Some(node.id.clone());
        }
        latest = Some(node.id.clone());
    }
    latest
}

/// Returns `true` if `arguments_json` parses to the same `agent_module` +
/// `prompt` as `input`.
fn arguments_match(arguments_json: &str, input: &SpawnSubAgentInput) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments_json) else {
        return false;
    };
    let Some(obj) = parsed.as_object() else {
        return false;
    };
    let module_ok = obj
        .get("agent_module")
        .and_then(|v| v.as_str())
        .map(|s| s == input.agent_module)
        .unwrap_or(false);
    let prompt_ok = obj
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(|s| s == input.prompt)
        .unwrap_or(false);
    module_ok && prompt_ok
}

/// Builds the sub-agent's `Agent` node parented to the parent's `Tool`
/// node (per AC-8.2).
fn build_sub_agent_node(child_agent_id: &str, parent_tool_node_id: &str, module: &str) -> Node {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    Node {
        id: child_agent_id.to_string(),
        parent_id: Some(parent_tool_node_id.to_string()),
        kind: NodeKind::Agent as i32,
        created_at: now,
        updated_at: now,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: module.to_string(),
            agent_id: child_agent_id.to_string(),
            status: "running".to_string(),
        })),
    }
}

/// Looks up the on-disk path of the agent module named `module_name`, if
/// it is currently registered as a [`ModuleKind::Agent`].
async fn resolve_agent_module(
    registry: &Arc<RwLock<ModuleRegistry>>,
    module_name: &str,
) -> Option<PathBuf> {
    let reg = registry.read().await;
    reg.by_kind(ModuleKind::Agent)
        .into_iter()
        .find(|(_, m)| m.name == module_name)
        .map(|(p, _)| p.clone())
}

#[cfg(test)]
mod tests;
