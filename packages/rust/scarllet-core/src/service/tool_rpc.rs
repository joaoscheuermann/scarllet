//! Tool-registry + invocation RPC handlers.
//!
//! `GetToolRegistry` enumerates external tools plus the synthetic
//! `spawn_sub_agent` affordance; `InvokeTool` routes calls to either
//! [`crate::tools::invoke_external`] or the sub-agent bridge.

use scarllet_proto::proto::*;
use scarllet_sdk::manifest::ModuleKind;
use tonic::{Request, Response, Status};

use crate::tools;

use super::session_rpc::lookup_session;
use super::OrchestratorService;

/// `GetToolRegistry` — returns the tools visible to the session.
///
/// Effort 03: every external `ModuleKind::Tool` manifest plus the synthetic
/// `spawn_sub_agent` entry so agents discover the sub-agent affordance from
/// the same surface as every other tool. Per-session policy hooks
/// (allow-lists / quotas) land in later phases. The runtime branch for
/// `spawn_sub_agent` is wired in [`invoke_tool`] but stubs out until effort
/// 5 — advertising it here keeps agent prompts stable across efforts.
pub async fn get_tool_registry(
    svc: &OrchestratorService,
    req: Request<GetToolRegistryRequest>,
) -> Result<Response<GetToolRegistryResponse>, Status> {
    let session_id = req.into_inner().session_id;
    let _ = lookup_session(svc, &session_id).await?;

    let reg = svc.registry.read().await;
    let mut tools: Vec<ToolInfo> = reg
        .by_kind(ModuleKind::Tool)
        .into_iter()
        .map(|(_, m)| ToolInfo {
            name: m.name.clone(),
            description: m.description.clone(),
            input_schema_json: m
                .input_schema
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            timeout_ms: m.timeout_ms.unwrap_or(30_000),
        })
        .collect();
    tools.push(spawn_sub_agent_tool_info());
    Ok(Response::new(GetToolRegistryResponse { tools }))
}

/// `InvokeTool` — validates the calling session + agent and routes to
/// [`tools::invoke`].
///
/// Effort 03 enforces the AC-10.2 contract: the call must carry a
/// `session_id` for an active session and an `agent_id` currently
/// registered in that session. Calls from a dead agent (post
/// `TurnFinished`, post failure, post Stop) are rejected with
/// `failed_precondition` so the SDK surfaces a clear error to the caller.
pub async fn invoke_tool(
    svc: &OrchestratorService,
    req: Request<InvokeToolRequest>,
) -> Result<Response<InvokeToolResponse>, Status> {
    let InvokeToolRequest {
        session_id,
        agent_id,
        tool_name,
        input_json,
    } = req.into_inner();

    let handle = lookup_session(svc, &session_id).await?;
    {
        let session = handle.read().await;
        if session.agents.get(&agent_id).is_none() {
            return Err(Status::failed_precondition(format!(
                "Agent '{agent_id}' is not registered in session '{session_id}'"
            )));
        }
    }

    let result = tools::invoke(
        &svc.sessions,
        &svc.registry,
        &svc.bound_addr,
        &session_id,
        &agent_id,
        &tool_name,
        &input_json,
    )
    .await;
    Ok(Response::new(InvokeToolResponse {
        success: result.success,
        output_json: result.output_json,
        error_message: result.error_message,
        duration_ms: result.duration_ms,
    }))
}

/// Builds the synthetic `spawn_sub_agent` [`ToolInfo`] returned alongside
/// the external manifests. Kept private to the RPC layer: the constants
/// live in `tools.rs` so the runtime branch and the registry stay in lock
/// step.
fn spawn_sub_agent_tool_info() -> ToolInfo {
    ToolInfo {
        name: tools::SPAWN_SUB_AGENT_TOOL.to_string(),
        description: tools::SPAWN_SUB_AGENT_DESCRIPTION.to_string(),
        input_schema_json: tools::SPAWN_SUB_AGENT_INPUT_SCHEMA.to_string(),
        timeout_ms: 0,
    }
}

#[cfg(test)]
mod tests;
