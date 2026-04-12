---
status: done
order: 6
created: 2026-04-10 19:48
title: "Agent lifecycle, delegation, and cancellation"
---

## Description

Implement agent process management in Core. When a task is submitted, Core selects a registered agent, spawns its binary, passes the Core gRPC address via `SCARLLET_CORE_ADDR` environment variable, and tracks its lifecycle. The agent connects back to Core as a gRPC client, can invoke tools via `InvokeTool`, report progress via `ReportProgress`, and query state. The user can cancel a task, which causes Core to kill the agent and all its child processes. Add `SubmitTask`, `CancelTask`, `ReportProgress`, and `GetAgentStatus` RPCs.

## Objective

Submitting a task via `SubmitTask` RPC causes Core to spawn an agent binary. The agent connects back, invokes a tool, reports progress, and completes. The task lifecycle is visible via `GetAgentStatus`. Cancelling a task kills the agent immediately.

## Implementation Details

1. **`scarllet-proto` additions:**
   - `rpc SubmitTask(TaskSubmission) returns (TaskReceipt)` — `TaskSubmission { agent_name, task_description, working_directory }`, `TaskReceipt { task_id, snapshot_id }`.
   - `rpc CancelTask(CancelRequest) returns (CancelResponse)` — `CancelRequest { task_id }`.
   - `rpc ReportProgress(ProgressReport) returns (Ack)` — `ProgressReport { task_id, status, message, tool_invocation (optional) }`.
   - `rpc GetAgentStatus(AgentStatusQuery) returns (AgentStatusResponse)` — returns task state, agent name, progress history.
   - Task status enum: `Pending`, `Running`, `Completed`, `Failed`, `Cancelled`.
2. **`scarllet-core` task manager:**
   - `TaskManager` struct: in-memory map of `task_id → TaskState`.
   - `TaskState`: agent name, PID, status, progress log, snapshot_id, working_directory.
   - `SubmitTask` handler:
     - Resolve agent from registry.
     - Create a point-in-time tool snapshot (version ID from Effort 5).
     - Spawn agent binary with env `SCARLLET_CORE_ADDR=127.0.0.1:<port>`, `SCARLLET_TASK_ID=<task_id>`, `SCARLLET_SNAPSHOT_ID=<snapshot_id>`.
     - Set working directory to `working_directory` from request.
     - Track PID in TaskState.
   - `CancelTask` handler:
     - Send SIGTERM to agent process.
     - 2-second grace period, then SIGKILL.
     - On Unix: kill process group (`killpg`). On Windows: use job objects or `TerminateProcess` on child tree.
     - Set status to `Cancelled`.
   - Monitor agent process exit: on natural exit, set status to `Completed` or `Failed` based on exit code.
3. **`scarllet-sdk` agent client helpers:**
   - `AgentContext` struct: reads `SCARLLET_CORE_ADDR`, `SCARLLET_TASK_ID`, `SCARLLET_SNAPSHOT_ID` from environment.
   - Provides typed methods: `invoke_tool()`, `report_progress()`, `get_credentials()`, `get_chat_history()`.
   - Connects to Core's gRPC server using the address from env.
4. **Test agent binary:**
   - Create `packages/rust/test-fixtures/test-agent/` binary.
   - On start: read env vars, connect to Core, call `ReportProgress("starting")`, call `InvokeTool("echo-tool", ...)`, call `ReportProgress("tool complete")`, exit.

## Verification Criteria

- Register `test-agent` in `agents/` and `echo-tool` in `tools/`.
- Call `SubmitTask(agent_name="test-agent", task_description="test")` → returns `task_id`.
- `GetAgentStatus(task_id)` shows status transition: `Pending` → `Running` → `Completed`.
- Core logs show: agent spawned, agent connected, progress reports received, tool invoked, agent exited.
- Call `SubmitTask` with a slow-running agent, then `CancelTask(task_id)` → agent process is killed, status becomes `Cancelled`.
- Verify killed agent's child processes are also dead (no orphans).
- `npx nx run scarllet-core:test` passes unit tests for task lifecycle and cancellation.

## Done

- Full agent lifecycle: spawn, connect-back, tool invocation, progress reporting, natural exit, and forced cancellation.
- Observable via GetAgentStatus RPC and Core logs.
