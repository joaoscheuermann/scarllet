//! Tool invocation — external modules + the synthetic `spawn_sub_agent`
//! built-in.
//!
//! Both surfaces return a common [`ToolResult`] shape so agents see a
//! uniform tool-call outcome regardless of whether the call crossed a
//! subprocess boundary or was handled entirely inside core.

use crate::agents;
use crate::registry::ModuleRegistry;
use crate::session::SessionRegistry;
use scarllet_sdk::manifest::ModuleKind;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Outcome of a tool invocation, carrying either JSON output or an error.
pub struct ToolResult {
    pub success: bool,
    pub output_json: String,
    pub error_message: String,
    pub duration_ms: u64,
}

/// Reserved tool name routed to the core-internal sub-agent spawn path.
pub const SPAWN_SUB_AGENT_TOOL: &str = "spawn_sub_agent";

/// Synthetic tool description advertised in `GetToolRegistry` so agents can
/// discover the sub-agent spawn affordance from the same surface as every
/// other tool. The runtime branch in [`invoke`] delegates to
/// [`agents::spawn::handle_spawn_sub_agent`].
pub const SPAWN_SUB_AGENT_DESCRIPTION: &str =
    "Spawn a sub-agent to handle a focused sub-task. Pass `{ \"agent_module\": \"<name>\", \"prompt\": \"<task>\" }`. The call blocks until the sub-agent emits its final Result; the returned output_json carries `{ content, finish_reason }`.";

/// JSON Schema string for the synthetic `spawn_sub_agent` manifest entry.
/// Kept here so both the registry RPC and the runtime branch agree on shape.
pub const SPAWN_SUB_AGENT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "agent_module": { "type": "string", "description": "name of an installed agent module" },
        "prompt": { "type": "string", "description": "instruction for the sub-agent" }
    },
    "required": ["agent_module", "prompt"]
}"#;

/// Routes a tool invocation. Branches on the synthetic `spawn_sub_agent`
/// name so the core-internal spawn path is exercised without leaking
/// through the external-tool manifest resolver; everything else falls
/// through to [`invoke_external`].
pub async fn invoke(
    sessions: &Arc<RwLock<SessionRegistry>>,
    registry: &Arc<RwLock<ModuleRegistry>>,
    core_addr: &str,
    session_id: &str,
    agent_id: &str,
    tool_name: &str,
    input_json: &str,
) -> ToolResult {
    if tool_name == SPAWN_SUB_AGENT_TOOL {
        return agents::spawn::handle_spawn_sub_agent(
            sessions,
            registry,
            core_addr,
            session_id,
            agent_id,
            input_json,
        )
        .await;
    }
    invoke_external(registry, session_id, agent_id, tool_name, input_json).await
}

/// Runs a registered tool binary by piping `input_json` to its stdin.
///
/// Looks up the manifest by name in the per-process [`ModuleRegistry`],
/// enforces the manifest-declared timeout, and returns structured success
/// or error information. `session_id` and `agent_id` are forwarded to the
/// child process via `SCARLLET_SESSION_ID` / `SCARLLET_AGENT_ID` so tools
/// that want to audit-log have the originating identity.
pub async fn invoke_external(
    registry: &Arc<RwLock<ModuleRegistry>>,
    session_id: &str,
    agent_id: &str,
    tool_name: &str,
    input_json: &str,
) -> ToolResult {
    let reg = registry.read().await;

    let tool_entry = reg
        .by_kind(ModuleKind::Tool)
        .into_iter()
        .find(|(_, m)| m.name == tool_name);

    let (path, manifest) = match tool_entry {
        Some((p, m)) => (p.clone(), m.clone()),
        None => {
            return ToolResult {
                success: false,
                output_json: String::new(),
                error_message: format!("Tool '{tool_name}' not found"),
                duration_ms: 0,
            };
        }
    };

    let timeout_ms = manifest.timeout_ms.unwrap_or(30000);
    drop(reg);

    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    let result = tokio::time::timeout(timeout, async {
        let mut child = match tokio::process::Command::new(&path)
            .env("SCARLLET_SESSION_ID", session_id)
            .env("SCARLLET_AGENT_ID", agent_id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output_json: String::new(),
                    error_message: format!("Failed to spawn tool: {e}"),
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(input_json.as_bytes()).await;
            drop(stdin);
        }

        match child.wait_with_output().await {
            Ok(output) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return ToolResult {
                        success: false,
                        output_json: String::new(),
                        error_message: format!("Tool exited with {}: {stderr}", output.status),
                        duration_ms,
                    };
                }
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                if serde_json::from_str::<serde_json::Value>(&stdout).is_err() {
                    debug!("Tool output is not valid JSON");
                }
                ToolResult {
                    success: true,
                    output_json: stdout,
                    error_message: String::new(),
                    duration_ms,
                }
            }
            Err(e) => ToolResult {
                success: false,
                output_json: String::new(),
                error_message: format!("Failed to read tool output: {e}"),
                duration_ms: start.elapsed().as_millis() as u64,
            },
        }
    })
    .await;

    match result {
        Ok(r) => r,
        Err(_) => {
            warn!("Tool '{tool_name}' exceeded timeout of {timeout_ms}ms — killing");
            ToolResult {
                success: false,
                output_json: String::new(),
                error_message: format!("Tool exceeded timeout of {timeout_ms}ms"),
                duration_ms: timeout_ms,
            }
        }
    }
}
