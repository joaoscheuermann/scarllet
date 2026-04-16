---
status: done
created: 2026-04-16 12:22
slug: agent-history-hydration
---

## Prompt

When we start the agent and we have the signal it was initiallized, we should push the current history right after it is started and before the first prompt, so we don't need to send all the history everytime.

## Research

(empty)

## Architecture

### Overview

One-time history hydration: TUI sends persisted conversation history to Core on connect. Core stores it in memory. When an agent registers via AgentStream, Core sends the history as the first `AgentInstruction::HistorySync` message before any tasks. Agent seeds its in-memory LLM context. No per-prompt history overhead.

### Proto Changes — `orchestrator.proto`

New messages:
- `HistoryEntry { role, content }` — shared type for a single conversation turn
- `HistorySync { repeated HistoryEntry }` — TUI → Core history payload
- `AgentHistorySync { repeated HistoryEntry }` — Core → Agent hydration payload
- `AgentInstruction { oneof: AgentTask | AgentHistorySync }` — replaces `AgentTask` as `AgentStream` return type

Modified:
- `TuiMessage` oneof — add `HistorySync history_sync = 3`
- `AgentStream` RPC — return type changes to `stream AgentInstruction`

### Data Flow

```
TUI starts → loads session.json
TUI connects → Core sends Connected
TUI sends HistorySync(messages) → Core stores in memory
User sends prompt → Core routes to agent
Agent registers → Core sends AgentInstruction::HistorySync (one-time)
Agent seeds history vec
Core sends AgentInstruction::Task(prompt) → Agent processes with full context
Core appends user/assistant messages to stored history on each turn
```

### Impacted Files

| File | Change |
|------|--------|
| `orchestrator.proto` | New messages, modified TuiMessage, changed AgentStream return |
| `scarllet-core/src/service.rs` | Store history, handle HistorySync from TUI, send to agent on register, change stream type |
| `scarllet-core/src/agents.rs` | Channel type: `Sender<Result<AgentTask>>` → `Sender<Result<AgentInstruction>>` |
| `scarllet-core/src/routing.rs` | Wrap AgentTask in AgentInstruction::Task when sending |
| `scarllet-tui/src/events.rs` | On Connected event, send HistorySync from app.messages |
| `agents/default/src/main.rs` | Match AgentInstruction payload: Task → existing logic, HistorySync → seed history |

### Design Decisions

1. **AgentInstruction wrapper** (not field on AgentTask) — clean ISP separation between tasks and hydration
2. **TUI sends history to Core** (not Core reads session file) — DIP: agent doesn't know about TUI storage
3. **Core also accumulates** history from prompt/response events — stays current between TUI syncs
4. **One-time push** per agent init — no per-prompt overhead

### Principles Applied

- SRP: TUI owns persistence, Core owns routing, agent owns LLM context
- DIP: History flows through proto contract
- ISP: Distinct AgentInstruction types for tasks vs hydration
- KISS: One-time push, single global history store
- DRY: Reuses session.json data
- OCP: Additive proto changes
