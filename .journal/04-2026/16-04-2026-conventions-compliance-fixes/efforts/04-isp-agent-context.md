---
status: done
order: 4
created: 2026-04-16 09:58
title: "ISP: Extract AgentContext struct in default-agent"
---

## Description

Replace the 11-parameter `run_tool_loop` function in `default-agent` with a struct that groups the related parameters. Remove the `#[allow(clippy::too_many_arguments)]` suppression.

## Objective

After this effort, `run_tool_loop` takes 3 parameters (`ctx: &mut AgentContext`, `history: &mut Vec<ChatMessage>`, `blocks: &mut Vec<AgentBlock>`) instead of 11. The clippy allow is gone. The agent compiles and handles prompts identically.

## Implementation Details

### AgentContext struct

Add to `packages/rust/agents/default/src/main.rs`:

```rust
struct AgentContext<'a> {
    llm: &'a LlmClient,
    client: &'a mut OrchestratorClient<tonic::transport::Channel>,
    msg_tx: &'a tokio::sync::mpsc::Sender<AgentMessage>,
    task: &'a AgentTask,
    tool_definitions: &'a [ToolDefinition],
    system_prompt: &'a str,
    model: &'a str,
    reasoning_effort: &'a Option<String>,
    context_window: u32,
}
```

### Update run_tool_loop

1. Remove `#[allow(clippy::too_many_arguments)]`.
2. Change signature to: `async fn run_tool_loop(ctx: &mut AgentContext<'_>, history: &mut Vec<ChatMessage>, blocks: &mut Vec<AgentBlock>) -> Result<(), Box<dyn std::error::Error>>`.
3. Inside the function, replace all parameter references with `ctx.field` (e.g. `ctx.llm`, `ctx.task.task_id`, `ctx.msg_tx`).

### Update call site in main

In the `while let Some(task)` loop, construct `AgentContext` and pass it to `run_tool_loop`.

## Verification Criteria

1. `npx nx run default-agent:build` succeeds.
2. `cargo clippy -p default-agent` passes with no warnings (the `too_many_arguments` allow is gone).
3. Start core + TUI, send a prompt with tool calls — agent processes the full tool loop correctly, responses and tool call results appear in the TUI.

## Done

- `AgentContext` struct exists in `default-agent/src/main.rs`.
- `run_tool_loop` takes 3 params instead of 11.
- `#[allow(clippy::too_many_arguments)]` is removed.
- Agent handles prompts, streams responses, and executes tool calls correctly.
