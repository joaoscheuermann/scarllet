---
status: done
order: 5
created: 2026-04-10 19:48
title: "Tool invocation with stdin/stdout JSON and timeout enforcement"
---

## Description

Implement the Core's tool execution engine. When an `InvokeTool` RPC is received, Core spawns the requested tool binary, pipes JSON input via stdin, captures JSON output from stdout, and returns the result. If the tool exceeds its manifest-declared `timeout_ms`, Core hard-kills the process and returns a `ToolRunFailure`. Implement point-in-time tool snapshots: each tool invocation context is bound to a registry version, so newly registered tools are invisible to in-flight sessions.

## Objective

A gRPC client calls `InvokeTool(tool_name, input_json)` and receives either a `ToolResult` with the tool's JSON output, or a `ToolRunFailure` with an error description. Tools that exceed their timeout are killed immediately and reported as failures.

## Implementation Details

1. **`scarllet-proto` additions:**
   - `rpc InvokeTool(ToolInvocation) returns (ToolResult)`.
   - `ToolInvocation { tool_name, input_json, snapshot_id }`.
   - `ToolResult { success, output_json, error_message, duration_ms }`.
2. **`scarllet-core` tool executor:**
   - Resolve tool binary path from the module registry.
   - Validate `snapshot_id`: if the tool was not registered at the given snapshot version, return error.
   - Spawn tool binary as a child process (`tokio::process::Command`).
   - Write `input_json` to stdin, close stdin.
   - Race stdout capture against `tokio::time::timeout(timeout_ms)`.
   - On timeout: hard kill via `child.kill()` (sends SIGKILL on Unix, TerminateProcess on Windows). Return `ToolRunFailure` with "timeout exceeded" message.
   - On success: parse stdout as JSON. If invalid JSON, return `ToolRunFailure` with "invalid output" message.
   - Capture stderr for diagnostic logging.
3. **Point-in-time snapshots:**
   - Module registry maintains a monotonic version counter, incremented on every register/deregister.
   - `snapshot_id` is the version at which a session was created.
   - `InvokeTool` checks: was `tool_name` registered at or before `snapshot_id`? If not, reject.
4. **Test fixtures:**
   - `echo-tool`: reads stdin JSON, writes it back to stdout (from Effort 3).
   - `slow-tool`: reads stdin, sleeps for a configurable duration, then writes output. Used to test timeout enforcement.

## Verification Criteria

- Register `echo-tool` in `tools/`. Call `InvokeTool("echo-tool", '{"hello":"world"}')` → returns `output_json: '{"hello":"world"}'`, `success: true`.
- Register `slow-tool` with `timeout_ms: 1000`. Call `InvokeTool("slow-tool", '{"sleep_ms": 5000}')` → returns `success: false`, `error_message` contains "timeout". Verify slow-tool process is no longer running (PID check).
- Tool that writes invalid stdout → returns `success: false`, `error_message` contains "invalid output".
- Tool that exits with non-zero code → returns `success: false` with stderr captured.
- Register a new tool AFTER creating a snapshot. Call `InvokeTool` with the old `snapshot_id` for the new tool → rejected.
- `npx nx run scarllet-core:test` passes unit tests for tool execution, timeout, and snapshot validation.

## Done

- Core can execute tools via stdin/stdout JSON with hard-kill timeout enforcement.
- Point-in-time snapshots prevent newly registered tools from being visible to existing sessions.
- Observable via gRPC calls and process inspection.
