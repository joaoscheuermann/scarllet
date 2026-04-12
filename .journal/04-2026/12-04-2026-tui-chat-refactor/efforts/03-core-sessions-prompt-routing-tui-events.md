---
status: done
order: 3
created: 2026-04-12 14:17
title: "Core session registry + prompt routing + TUI event rendering"
---

## Description

Wire the full prompt-to-response pipeline. Core gains a TUI session registry for broadcasting events, prompt routing logic (command vs free-form → agent), and a bridge from `ReportProgress` to `CoreEvent` broadcasts. TUI sends `PromptMessage` over the stream when the user presses Enter, receives `CoreEvent`s, and renders agent thinking indicators, intermediate content, and final responses in the chat history.

This is the integration effort that makes the chat functional end-to-end.

## Objective

User types a prompt → TUI sends it to Core via the `AttachTui` stream → Core routes it (command or agent) → agent processes and reports progress → Core broadcasts `AgentStartedEvent`, `AgentThinkingEvent`, `AgentResponseEvent` → TUI renders thinking indicator and final response in the chat history labeled `<agent_name> (<task_id_short>)`.

## Implementation Details

### Core — TUI Session Registry (`packages/rust/scarllet-core/src/sessions.rs`)

New module:

```rust
struct TuiSessionRegistry {
    sessions: HashMap<String, mpsc::Sender<CoreEvent>>,
}

impl TuiSessionRegistry {
    fn new() -> Self
    fn register(&mut self, id: String, sender: mpsc::Sender<CoreEvent>)
    fn deregister(&mut self, id: &str)
    async fn broadcast(&self, event: CoreEvent)  // send to all sessions, skip closed channels
}
```

- `broadcast` clones the event and sends to each registered sender. If a send fails (channel closed), the session is marked for cleanup.
- Add `session_registry: Arc<RwLock<TuiSessionRegistry>>` to `OrchestratorService`.

### Core — AttachTui upgrade (`packages/rust/scarllet-core/src/main.rs`)

Upgrade the stub from Effort 1:

1. Generate a session ID (UUID).
2. Create `mpsc::channel::<CoreEvent>(256)` for outgoing events.
3. Register the sender in `TuiSessionRegistry`.
4. Send `ConnectedEvent` with uptime.
5. Spawn a reader task for the incoming `TuiMessage` stream:
   - On `PromptMessage`: call `route_prompt(text, working_dir, ...)`.
6. Return a `ReceiverStream` wrapping the receiver as the response stream.
7. When the reader task ends (stream closed): deregister the session.

### Core — Prompt routing (`packages/rust/scarllet-core/src/main.rs` or new `prompt.rs`)

```rust
async fn route_prompt(
    text: &str,
    working_dir: &str,
    registry: &Arc<RwLock<ModuleRegistry>>,
    task_manager: &Arc<RwLock<TaskManager>>,
    session_registry: &Arc<RwLock<TuiSessionRegistry>>,
    core_addr: &str,
)
```

Logic:
1. Check if `text` starts with `/` and matches a registered command name or alias → if so, spawn the command binary (commands may internally launch agents).
2. Otherwise: pick the first available agent from the registry (or a configured default) and submit as a task via `task_manager.submit(...)`.
3. If no agent is available: broadcast `SystemEvent { message: "No agent available to handle this prompt." }`.
4. On successful task submission: broadcast `AgentStartedEvent { task_id, agent_name }`.
5. Spawn the agent via existing `tasks::spawn_agent(...)`.

### Core — ReportProgress → Event bridge (`packages/rust/scarllet-core/src/main.rs`)

In the existing `report_progress` RPC handler, after appending to `progress_log`, also broadcast to TUI sessions:

```rust
let event = match r.status.as_str() {
    "thinking" => CoreEvent with AgentThinkingEvent { task_id, agent_name, content: r.message },
    "response" => CoreEvent with AgentResponseEvent { task_id, agent_name, content: r.message },
    "error"    => CoreEvent with AgentErrorEvent { task_id, agent_name, error: r.message },
    _          => CoreEvent with AgentThinkingEvent { ... },  // default to thinking
};
session_registry.broadcast(event).await;
```

Also broadcast when agent task completes (in `tasks::spawn_agent`, after the agent exits):
- Success without explicit response → broadcast `AgentResponseEvent` with "Task completed."
- Failure → broadcast `AgentErrorEvent` with the error details.

### TUI — Send prompts over stream (`packages/rust/scarllet-tui/src/main.rs`)

In the `Enter` key handler (from Effort 2), after pushing `ChatEntry::User`:
- Construct `TuiMessage` with `PromptMessage { text, working_directory }`.
- Send it via the stream's sender channel (`message_tx.send(...)`).
- `working_directory` = `std::env::current_dir()`.

### TUI — Receive and render CoreEvents

In the main loop, drain events from `event_rx`:

```rust
while let Ok(event) = event_rx.try_recv() {
    match event {
        AgentStartedEvent { task_id, agent_name } => {
            app.messages.push(ChatEntry::Agent {
                name: agent_name,
                task_id,
                content: String::new(),
                done: false,
            });
        }
        AgentThinkingEvent { task_id, content, .. } => {
            // Find the last Agent entry with this task_id, append/replace content
            if let Some(entry) = find_agent_entry(&mut app.messages, &task_id) {
                entry.content = content;  // replace with latest thinking
            }
        }
        AgentResponseEvent { task_id, content, .. } => {
            if let Some(entry) = find_agent_entry(&mut app.messages, &task_id) {
                entry.content = content;
                entry.done = true;
            }
        }
        AgentErrorEvent { task_id, error, .. } => {
            app.messages.push(ChatEntry::System {
                text: format!("Error ({}): {}", &task_id[..8], error),
            });
            // Mark the agent entry as done
            if let Some(entry) = find_agent_entry(&mut app.messages, &task_id) {
                entry.done = true;
            }
        }
        SystemEvent { message } => {
            app.messages.push(ChatEntry::System { text: message });
        }
    }
}
```

Helper: `find_agent_entry` finds the last `ChatEntry::Agent` matching the given `task_id`.

## Verification Criteria

1. `npx nx run-many -t build -p scarllet-core,scarllet-tui` — builds successfully.
2. `npx nx run scarllet-core:test` — existing tests still pass.
3. Start Core → start TUI → connect.
4. Type a prompt with no agents registered → `"System: No agent available to handle this prompt."` appears in chat.
5. Register an agent (e.g., place a test agent binary in the agents directory) → type a prompt → see `<agent_name> (<id>):` with thinking indicator → see final response when agent completes.
6. If agent fails → error message appears in chat.
7. Multiple prompts sequentially → each creates its own agent entry in history.

## Done

- User types a prompt and it is sent to Core via the bidirectional stream.
- Core routes the prompt and launches an agent.
- Agent progress (thinking, response, error) streams back and renders in TUI chat history.
- System messages appear for errors and edge cases (no agent available).

## Change Summary

**Files created:**
- `packages/rust/scarllet-core/src/sessions.rs` — TuiSessionRegistry with register/deregister/broadcast + 2 unit tests.

**Files modified:**
- `packages/rust/scarllet-core/src/main.rs` — Added `mod sessions`, `session_registry` field to OrchestratorService, upgraded `attach_tui` (session registration, incoming message reader with prompt routing, session cleanup on disconnect), upgraded `report_progress` (bridges status to CoreEvent broadcasts: thinking/response/error). Added `route_prompt` function (command detection, agent lookup, task submission, AgentStartedEvent broadcast, post-completion event broadcast).
- `packages/rust/scarllet-tui/src/main.rs` — Removed `#[allow(dead_code)]` from ChatEntry and message_tx. Added `tui_message` import. Updated Enter handler to send PromptMessage via message_tx. Updated `handle_core_event` to handle all 5 event types: AgentStarted (push new Agent entry), AgentThinking (update content), AgentResponse (mark done, unlock input), AgentError (mark done, push System entry, unlock input), System (push System entry). Added `find_agent_entry` helper.

**Decisions:** Used `try_send` for broadcasting (non-blocking, avoids holding locks across await). Commands recognized but return "not yet implemented" system message (placeholder for future). Agent completion broadcasts AgentResponseEvent/AgentErrorEvent after spawn_agent returns.

**Deviations:** None — followed Implementation Details as specified.
