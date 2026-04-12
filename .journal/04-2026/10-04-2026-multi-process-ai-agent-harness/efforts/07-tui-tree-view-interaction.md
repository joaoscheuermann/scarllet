---
status: done
order: 7
created: 2026-04-10 19:48
title: "TUI tree-view, auto-completion, and reconnection"
---

## Description

Upgrade the TUI from a basic connection display (Effort 2) to the full interactive interface. Implement the hierarchical tree-view that visualizes concurrent agent and tool execution, slash-command auto-completion driven by Core's command registry, task submission and cancellation from the TUI, buffered output display, and reconnection from a different working directory.

## Objective

The user can type `/` in the TUI to see auto-completed commands, submit tasks, observe agent→tool execution in a live tree-view with buffered thinking/output, cancel running tasks, and reconnect from a different directory while existing tasks continue in their original directories.

## Implementation Details

1. **`scarllet-proto` additions:**
   - Extend `CoreEvent` (the TUI streaming response) with event variants: `AgentSpawned`, `AgentProgress`, `ToolInvocationStarted`, `ToolInvocationCompleted`, `TaskCompleted`, `TaskCancelled`, `OutputBuffer`.
   - Extend `TuiMessage` (the TUI streaming request) with: `SubmitTaskRequest`, `CancelTaskRequest`, `UserInput`.
2. **`scarllet-core` TUI event broadcasting:**
   - When task state changes (agent spawn, progress, tool invocation, completion), broadcast `CoreEvent` to all attached TUI streams.
   - Buffer recent events so a newly attached TUI gets current state.
3. **`scarllet-tui` layout redesign:**
   - **Left panel:** Tree-view widget showing active tasks. Each task node expands to show the agent and its current/past tool invocations. Use indented tree rendering with status icons (spinner for running, checkmark for done, X for failed/cancelled).
   - **Right/main panel:** Output buffer for the selected task. Shows agent progress messages and tool outputs as they arrive.
   - **Bottom bar:** Input field with slash-command auto-completion.
   - **Status bar:** Connection status (from Effort 2).
4. **Slash-command auto-completion:**
   - On `/` keystroke: fetch `ListCommands` from Core, display filtered dropdown as the user types.
   - Strict matching only — no fuzzy or natural language completion.
   - Enter on a command → submit it.
5. **Task interaction:**
   - `/submit <agent> <description>` → sends `SubmitTaskRequest` via the TUI stream.
   - `/cancel <task_id>` → sends `CancelTaskRequest`.
   - Tree-view updates in real time as CoreEvents arrive.
6. **Reconnection:**
   - TUI sends its current working directory on `AttachTui`.
   - Core tracks per-TUI working directory context.
   - New TUI from a different directory: Core keeps existing tasks in their original directories; new tasks from this TUI run in the new directory.

## Verification Criteria

- Start Core with registered agent + tools. Start TUI.
- Type `/` → see list of available commands filtered as you type.
- Submit a task via `/submit test-agent "hello"` → tree-view shows the task with agent node.
- As agent runs: tree-view updates to show tool invocations under the agent node, output panel shows progress.
- Cancel a task via `/cancel <id>` → tree-view shows cancelled status.
- Open a second TUI from a different directory → second TUI shows existing running tasks.
- Submit a task from the second TUI → task runs in the second TUI's working directory (verified by agent's working directory in logs).
- Close second TUI → first TUI and existing tasks are unaffected.

## Done

- Full interactive TUI with tree-view, auto-completion, task management, and multi-directory reconnection.
- Observable by running the TUI and interacting with agents and tools in real time.
