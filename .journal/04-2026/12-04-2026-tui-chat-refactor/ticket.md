---
status: done
created: 2026-04-12 14:02
slug: tui-chat-refactor
---

## Prompt

We should update the TUI executable.

We have some problems, currently, the TUI could not conntect to the Core.

Also, I want to refactor how it looks and how we use it.

We should have just two sections, chat history and user text input, the chat history should be most of the view area, like 95%, and the user input 5%.

We should implement just a small MVP of chat history. When the user types something, we add it to the history and render it. When the agent is thinking, we should add the "thinking" status, should display the intermediate thinking of the agent and then add the output of the agent.

## Research

(empty)

## Architecture

### Overview

Refactor the TUI into a two-section chat interface (history ~95%, input ~5%) with bidirectional gRPC streaming to Core. The TUI connects via an `AttachTui` bidirectional stream, sends user prompts, and receives real-time agent events (thinking, progress, response, errors). Core routes prompts to commands or agents and broadcasts events to all attached TUI sessions.

### Crate Impact Map

| Crate | Change Scope |
|-------|-------------|
| `scarllet-proto` | Add `AttachTui` RPC + 8 new message types (additive) |
| `scarllet-sdk` | Add `lockfile::is_pid_alive()` for stale PID detection |
| `scarllet-core` | New `sessions.rs` module for TUI session broadcast; `AttachTui` impl; progress→event bridge; prompt routing |
| `scarllet-tui` | Full rewrite — new state model, two-screen layout, bidirectional stream event loop |

### Proto Contract (additive)

```protobuf
service Orchestrator {
  // ... existing RPCs unchanged ...
  rpc AttachTui(stream TuiMessage) returns (stream CoreEvent);
}

message TuiMessage {
  oneof payload {
    PromptMessage prompt = 1;
  }
}

message PromptMessage {
  string text = 1;
  string working_directory = 2;
}

message CoreEvent {
  oneof payload {
    ConnectedEvent connected = 1;
    AgentStartedEvent agent_started = 2;
    AgentThinkingEvent agent_thinking = 3;
    AgentResponseEvent agent_response = 4;
    AgentErrorEvent agent_error = 5;
    SystemEvent system = 6;
  }
}

message ConnectedEvent { uint64 uptime_secs = 1; }
message AgentStartedEvent { string task_id = 1; string agent_name = 2; }
message AgentThinkingEvent { string task_id = 1; string agent_name = 2; string content = 3; }
message AgentResponseEvent { string task_id = 1; string agent_name = 2; string content = 3; }
message AgentErrorEvent { string task_id = 1; string agent_name = 2; string error = 3; }
message SystemEvent { string message = 1; }
```

### Core — TUI Session Registry (`sessions.rs`)

Manages attached TUI sessions. Each session gets a `tokio::sync::mpsc::Sender<CoreEvent>`. Core broadcasts agent events to all sessions. Session registered on `AttachTui` connect, deregistered on stream close.

### Core — AttachTui Implementation

1. Register session sender in `TuiSessionRegistry`
2. Send `ConnectedEvent` immediately
3. Spawn reader task for incoming `TuiMessage` stream:
   - `PromptMessage` → route to command or agent → submit task
4. Outgoing stream reads from session's `mpsc::Receiver<CoreEvent>`
5. On stream close: deregister session

### Core — Progress → Event Bridge

When `ReportProgress` is called by agents, Core broadcasts to all TUI sessions:
- `status == "thinking"` → `AgentThinkingEvent`
- `status == "response"` → `AgentResponseEvent`
- `status == "error"` → `AgentErrorEvent`

### Core — Prompt Routing

```
route_prompt(text, working_dir):
    if text starts with "/" and matches registered command → spawn command
    else → pick default agent → submit task
    broadcast AgentStartedEvent
```

### SDK — Lockfile PID Check

`lockfile::is_pid_alive(pid: u32) -> bool` — platform-specific check (Windows: `OpenProcess`; Unix: `kill(pid, 0)`).

### TUI — State Model

```rust
enum Screen { Connecting { dots: usize, tick: u64 }, Chat }
enum Focus { Input, History }
enum ChatEntry {
    User { text: String },
    Agent { name: String, task_id: String, content: String, done: bool },
    System { text: String },
}
struct App {
    screen: Screen,
    messages: Vec<ChatEntry>,
    input: String,
    input_locked: bool,
    focus: Focus,
    scroll_offset: usize,
}
```

### TUI — Layout

- **Connecting screen**: Centered animated `"Connecting to agent core..."` with cycling dots
- **Chat screen**: `Layout::vertical([Constraint::Min(0), Constraint::Length(3)])`
  - History: renders ChatEntry list with labels (`You`, `<name> (<id>)`, `System`), distinct colors, auto-scroll
  - Input: bordered, shows cursor or disabled state, highlighted border when focused

### TUI — Input Handling

- Tab: toggle focus (Input ↔ History)
- Arrow keys (history focused): scroll
- Enter (input focused, not locked): send prompt or quit on "exit"
- Ctrl+C: quit from any state
- Input locked while agent is processing

### Dependency Graph (unchanged)

```
scarllet-proto (leaf)
       │
  scarllet-sdk (proto)
    ┌──┴──┐
scarllet-core  scarllet-llm
    │
scarllet-tui (sdk, proto)
```

### Design Principles

- **SRP**: Sessions separated from tasks. TUI owns presentation. Core owns routing.
- **OCP**: Additive RPC and oneof events. Existing RPCs unchanged.
- **DIP**: TUI depends on proto contracts, not Core internals.
- **ISP**: Each event variant carries only its fields. oneof for extensibility.
- **KISS**: Two screens. No plugin system. No persistence.
- **DRY**: Proto single source of truth. PID check in SDK shared.
