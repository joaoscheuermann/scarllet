---
status: done
created: 2026-04-17 12:48
slug: core-transport-refactor
---

## Prompt

I need to refactor the Core and Transport to support the flow described below. Compare with the existing architecture and define the necessary changes to achieve this goal.

Out of scope: GUI and Web UIs (but the design must not preclude them later).

### Core

The core is responsible for storing state and making it available to anything connected to it.

- Core manages all Sessions.
- A method `GetSessionState(session_id)` must return the entire state for a given session.
- Clients must be able to subscribe to any new state change for a given session.
- When emitting events for a session, only the diff (the new / changed data) should be emitted.

### Session

Sessions manage state isolated from other Sessions.

#### Queue

A queue of user Prompts. The intent is to allow the user to queue messages for the agent so they are consumed when the agent finishes another job. All prompts go through the Queue before being dispatched to an Agent. Spawning an agent follows this logic:

1. User sends a prompt.
2. It is added to the Queue.
3. Check if an agent is already running. If yes, wait for it to finish before spawning another agent. If not, spawn a new agent to handle the work.
4. The agent should update a given Message Node as its stream comes with new information.

#### Messages

A FLAT list of nodes. This stores the state of the current chat and is broadcast to connected UIs (TUI/GUI/Web) through a gRPC call.

##### Nodes

Nodes are FLAT and store the state of the chat: user prompts, user responses, tool calls, and so on.

- Node types: `User`, `Agent`, `Result`, `Thought`, `Tool`, `Debug`.
- Nodes can have a parent node, specified by an ID.
- Nodes can be updated individually. This is useful when streaming data for thoughts / messages, or updating the state of a Tool call in real time.
- There must be an RPC call to update a node individually, like `UpdateNode(...)`.

#### Agents

Agents cannot change Session state; they can only send messages and message updates. The Agents variable stores a connection and a PID for agent processes, and manages the streaming of data between the agent and the session.

A single session can have multiple agents running at the same time. Agents vs. Sub-Agents:

- Agents are usually long-lived and spawned to handle the conversation of a given session.
- Sub-Agents are created by agents to split work.
- Agents can also be spawned through Commands, but their spawn / despawn in that case is controlled by the Command, not by Core.

#### Commands

Manage the spawn / despawn of commands. Commands are packages of logic that can manage the internal state of a session: control over agents for advanced logic to perform repetitive tasks (for example, automating the Spec-Driven Development workflow by asking the agent for structured output instead of tool calls to create files), or mutating internal state such as the current active `config.provider`.

## Research

> Spec ratified on 2026-04-17 after iterative elicitation.
> See `decisions.md` for the decision log.

### Summary

Restructure the core from a single-chat daemon into a **multi-session orchestrator** whose primary data model is a per-session **flat graph of typed Nodes** streamed to UIs as **diffs**. Agents become strictly message-emitters (no session-state mutation); main agents and sub-agents are both per-turn processes identified by their parent id; prompts are queued per session. The TUI, core, proto and agent SDK all migrate atomically to the new contract.

### Current → Target gap analysis

| Concept | Today | Target |
|---|---|---|
| Session | Implicit singleton. `conversation_history`, `task_manager`, `session_registry` live flat on the core. | First-class entity. Core owns N sessions; each isolates its own Queue, Messages, Agents, Commands, Config. |
| Session discovery / attach | `AttachTui` attaches a TUI to the one global conversation. | TUI attaches to a specific session (by id); core has RPCs to list / get / destroy sessions. |
| State access | No "full state" RPC — only the event stream. TUI keeps its own `Session` / `SessionMessage` model and persists `session.json` locally. | `GetSessionState(session_id)` returns the entire state; a subscribe RPC streams **diffs** going forward. |
| Prompt ingestion | Prompt goes straight to `routing::route_prompt` → picks first agent → spawns or dispatches. | Prompt is enqueued. If no main agent is running in the session, one is spawned; otherwise wait for the current agent to finish. |
| Message model | Flat `Vec<HistoryEntry { role, content }>`. Role-based. No hierarchy. Thinking, tool-calls, etc. are transient **events**, not stored state. | Flat list of **Nodes** with `{id, parent_id?, type}`. Node types: `User`, `Agent`, `Result`, `Thought`, `Tool`, `Debug`, `TokenUsage`, `Error`. Thoughts / tool calls are first-class persisted state, not just events. |
| Per-node updates | No per-node concept. Progress events always carry full blocks. | Each node has an identity; an `UpdateNode` RPC patches an individual node (e.g. streaming tokens, tool status → result). |
| Event granularity | Core broadcasts whole event payloads (full blocks, full tool results) to every TUI every time. | Broadcast is a **diff** over session state (new nodes, updated fields). |
| Agents | One global `AgentRegistry` keyed by `agent_name`. Agents dispatched by name. Long-lived stream. | Multiple agents per session. Agents have a parent (session or another agent). Main agent = `parent = session_id`; sub-agent = `parent = <agent_id>`. |
| Agent → state | Agent can push any event; TUI is the source of truth for history (it re-syncs history on reconnect). | Agents can only emit / update **message nodes**; they cannot touch other session state (Config, Queue, etc.). |
| Commands | `ModuleKind::Command` exists in the registry but has no runtime. | Deferred — module kind stays registered, no runtime this phase. |
| Sub-agents | Not modelled. | Spawned by the core at the parent agent's request via a built-in `spawn_sub_agent` tool; parent-child relationship tracked; per-turn lifetime. |
| Cancellation | `CancelPrompt { task_id }` cancels a running task. | Session-wide Stop cancels running agent (and sub-agents) and clears the queue. |

### User stories & acceptance criteria

#### US-1 — Multi-session core with explicit or implicit creation

**As** a TUI user, **I want** the core to host multiple isolated sessions, **so that** I can run independent conversations without cross-contamination.

- **AC-1.1** **Given** the core is running, **when** a TUI calls `CreateSession`, **then** the core returns a unique session id and allocates per-session Queue, Messages (node list), Agents, Commands (registered but inert) and Config state seeded from the global defaults.
- **AC-1.2** **Given** a TUI attaches without a session id, **when** the attach is accepted, **then** the core auto-creates a session the same way as `CreateSession` and uses it as the attach target.
- **AC-1.3** **Given** sessions exist, **when** a TUI calls `ListSessions`, **then** the core returns each session's id, creation time, last-activity time, and the name of the main agent (if any is currently running).
- **AC-1.4** **Given** a session exists, **when** a TUI calls `DestroySession(session_id)`, **then** its running agent (and sub-agents) are cancelled, its queue is cleared, and the session is removed.
- **AC-1.5** **Given** two sessions exist, **when** an event occurs in session A, **then** subscribers of session B receive nothing related to session A.
- **AC-1.6** **Given** the core restarts, **when** it comes back up, **then** no previous sessions exist (in-memory only).

#### US-2 — Multi-TUI attach, full-state + diff subscription

**As** a TUI user, **I want** to get the whole session state once and then only receive diffs, **so that** late attachers, reconnects, and secondary TUIs all render without replaying history client-side.

- **AC-2.1** **Given** a session exists, **when** a TUI calls `GetSessionState(session_id)`, **then** the core returns the complete session state (Messages as a flat node list, Queue contents, connected Agents, effective Config).
- **AC-2.2** **Given** a TUI subscribes to a session, **when** any state change occurs, **then** subscribers receive a diff containing only the created / updated nodes and any changed session-level fields (queue, config, connected agents).
- **AC-2.3** **Given** multiple TUIs are attached to the same session, **when** a diff is produced, **then** all attached TUIs receive it.
- **AC-2.4** **Given** a TUI reconnects after a drop, **when** it reattaches, **then** it always calls `GetSessionState` and re-subscribes (no sequence-based resume).
- **AC-2.5** **Given** a TUI attaches, **when** it is the last TUI to detach, **then** the core cancels the running agent, clears the queue, and destroys the session.

#### US-3 — Per-session prompt queue

**As** a TUI user, **I want** my prompts to queue when the session is busy, **so that** I can pipeline work without blocking on the previous turn.

- **AC-3.1** **Given** a session has no main agent running, **when** a TUI sends a prompt, **then** the prompt is appended to the queue, a `User` node is created in Messages, and the core dispatches the prompt by spawning a fresh main-agent process (see US-4).
- **AC-3.2** **Given** a session has a main agent running, **when** a TUI sends a prompt, **then** the prompt is appended to the queue and a `User` node is created immediately; dispatch is deferred until the running main agent emits `Result` and exits.
- **AC-3.3** **Given** the configured default agent module is not registered, **when** a prompt would be dispatched, **then** a top-level `Error` node is created (no `parent_id`) with a visible message, and the prompt is removed from the queue.
- **AC-3.4** **Given** a main agent turn fails before emitting `Result`, **when** the failure is observed, **then** an `Error` node is created under that turn's Agent node, the turn is marked failed, and the queue is **paused** until the user explicitly stops / retries.
- **AC-3.5** **Given** the user triggers session-wide Stop, **when** the core handles it, **then** the running main agent (and its sub-agents) are cancelled, the queue is cleared, and a visible notice is recorded.

#### US-4 — Main agent identified by `parent = session_id`, per-turn lifetime

**As** a platform owner, **I want** the main agent to be any process registered with `parent = session_id`, **so that** "main agent" needs no separate bookkeeping and per-turn lifetime drops out naturally.

- **AC-4.1** **Given** the queue advances, **when** no agent has `parent = session_id`, **then** the core spawns a fresh process of the globally configured `default_agent` module and passes it the session id, a new agent id, and the dequeued prompt.
- **AC-4.2** **Given** the spawned agent connects, **when** it registers with `parent = session_id`, **then** the core accepts the registration and creates the Agent node that will parent the turn's Thought / Tool / Result / Debug nodes.
- **AC-4.3** **Given** the agent emits `finish_reason = stop`, **when** it also emits the `Result` node, **then** its process exits and the Agent slot for the session is empty again; the queue advances if non-empty.
- **AC-4.4** **Given** a session already has a main agent running, **when** another process attempts to register with the same `parent = session_id`, **then** the core rejects the registration. (Multi-main-agents is deferred.)

#### US-5 — Node-based Messages (create + partial-patch update)

**As** a UI, **I want** the conversation represented as a flat list of typed nodes with optional parent references and first-class per-node updates, **so that** I can render hierarchies and apply streaming updates without re-rendering the whole transcript.

- **AC-5.1** **Given** a new piece of session content is produced, **when** the agent / core creates it, **then** it calls `CreateNode` with `{id, type, parent_id?, payload}`; `type` is one of `User`, `Agent`, `Thought`, `Tool`, `Result`, `Debug`, `TokenUsage`, `Error`.
- **AC-5.2** **Given** a node exists, **when** a partial change is needed, **then** it calls `UpdateNode(id, patch)` with only the fields that changed; the core merges and broadcasts the diff.
- **AC-5.3** **Given** any node, **when** it is finalised or updated, **then** it is never deleted; edits go through `UpdateNode`.
- **AC-5.4** **Given** `User`, `Agent` and `Error` nodes are created, **then** they MAY be top-level (no `parent_id`); `Thought`, `Tool`, `Result`, `Debug`, `TokenUsage` nodes MUST reference an `Agent` node as `parent_id`. A top-level `Error` node represents a session-level error (e.g. AC-3.3); an `Error` node with an `Agent` parent represents a per-turn failure (AC-3.4).
- **AC-5.5** **Given** the agent is streaming thoughts, **when** tokens arrive, **then** one `Thought` node per contiguous thinking block is created and its content grows via `UpdateNode` (one-updated model).
- **AC-5.6** **Given** the agent invokes a tool, **when** lifecycle changes, **then** one `Tool` node is created and moves through `pending → running → done | failed` via `UpdateNode` (one-updated model); the final result is merged into the same node.
- **AC-5.7** **Given** the agent reports token usage or a recoverable error, **when** it emits one, **then** a dedicated `TokenUsage` or `Error` node is created under the Agent node.

#### US-6 — Debug nodes, always emitted, hidden by default in TUI

**As** a developer, **I want** debug messages to live in the session graph, **so that** they can be enabled / disabled in the UI without reshaping the transport.

- **AC-6.1** **Given** an agent emits a debug message, **when** it is received, **then** the core creates a `Debug` node parented to the current Agent node; no separate debug RPC is exposed.
- **AC-6.2** **Given** a TUI is connected, **when** the debug flag is off (default), **then** `Debug` nodes are hidden; **when** it is on, **then** they render inline under their Agent node.
- **AC-6.3** **Given** the agent SDK emits a debug log, **when** the old `EmitDebugLog` RPC would have been used, **then** the SDK now emits a `Debug` node through the node API; the `EmitDebugLog` RPC is removed.

#### US-7 — Agents are message-emitters only

**As** a platform owner, **I want** agents to be unable to mutate session state beyond their nodes, **so that** the session remains the single source of truth for Queue, Config, and other agents.

- **AC-7.1** **Given** an agent is connected, **when** it sends any payload other than `CreateNode` / `UpdateNode` (and built-in core tools such as `InvokeTool`, `spawn_sub_agent`), **then** the core rejects it and records the violation.
- **AC-7.2** **Given** a session has Config (e.g. active provider), **when** an agent attempts to change it, **then** the change is rejected.

#### US-8 — Sub-agents via core-spawned tool, nested under a Tool node

**As** an agent author, **I want** to spawn sub-agents through a uniform tool call, **so that** the core manages lifecycles and the session graph stays consistent.

- **AC-8.1** **Given** a running agent wants a sub-agent, **when** it calls the built-in `spawn_sub_agent` tool, **then** the core looks up the requested agent module, spawns its binary with `parent = <calling_agent_id>`, and returns once the child is connected.
- **AC-8.2** **Given** `spawn_sub_agent` is called, **when** nodes are created, **then** there is a `Tool` node (for the spawn call itself) whose `parent_id` is the calling Agent node, and an `Agent` node for the sub-agent whose `parent_id` is that Tool node; all of the sub-agent's Thought / Tool / Result / Debug nodes parent on the sub-agent's Agent node.
- **AC-8.3** **Given** a sub-agent emits `finish_reason = stop`, **when** it also emits its `Result` node, **then**:
  1. The sub-agent's full subtree (its `Agent` node + all `Thought` / `Tool` / `Result` / `Debug` / `TokenUsage` / `Error` children) remains persistent in `Messages`, AND
  2. The spawn `Tool` node (parented to the calling `Agent` node) is updated via `UpdateNode` so its result payload carries a summary of the sub-agent's final `Result`, AND
  3. The sub-agent's process exits.
- **AC-8.4** **Given** a parent agent is about to emit `finish_reason = stop`, **when** it still has running sub-agents, **then** this must not happen (invariant). If it does (bug / protocol violation), the core kills the parent and all its sub-agents and emits an `Error` node.
- **AC-8.5** **Given** session-wide Stop, **when** it is triggered, **then** cancellation cascades from the main agent through every sub-agent transitively.

#### US-9 — Per-session config inherits global defaults (override deferred)

**As** a session user, **I want** my session to start with the global default config, **so that** my sessions run against the currently configured provider without any per-session setup.

- **AC-9.1** **Given** a session is created, **when** it initialises, **then** its Config is deep-copied from the current global `ScarlletConfig` (notably `active_provider`).
- **AC-9.2** **Given** the global config is hot-reloaded, **when** the reload happens, **then** existing sessions keep whatever effective config they had at creation; the global reload only changes the default used for **new** sessions.
- **AC-9.3** *Deferred.* Per-session provider override is deferred to the Commands phase. No RPC for override is added in this refactor.

#### US-10 — Per-session tool registry

**As** a session author, **I want** the tool registry and `InvokeTool` to be session-scoped, **so that** per-session policy (allowlists, audit, quotas) is achievable without reshaping the transport later.

- **AC-10.1** **Given** a session exists, **when** a TUI or agent calls `GetToolRegistry(session_id)`, **then** the core returns the tools visible to that session (today: all registered tools).
- **AC-10.2** **Given** an agent calls `InvokeTool`, **when** it does, **then** the call carries `session_id` and the agent id so the core can associate the invocation with the correct session and `Tool` node.
- **AC-10.3** **Given** a tool is invoked, **when** the result returns, **then** the agent creates / updates the `Tool` node accordingly (one-updated model from AC-5.6).

#### US-11 — TUI migrates to the new model

**As** the TUI, **I want** to hold no local chat state, **so that** the core is authoritative and reconnects / multi-TUI work naturally.

- **AC-11.1** **Given** the TUI starts, **when** it connects, **then** it either attaches to a known session id or attaches without one and lets the core auto-create.
- **AC-11.2** **Given** the TUI receives a diff, **when** it applies the diff to its in-memory node list, **then** its render matches the core's state exactly with no replay needed.
- **AC-11.3** **Given** the TUI restarts, **when** it reopens, **then** no local `session.json` is read or written; a new session is created (old TUI persistence is dropped).
- **AC-11.4** **Given** the TUI used `HistorySync` previously, **when** the new proto is in place, **then** `HistorySync` is removed entirely (no import / export path).
- **AC-11.5** **Given** the TUI renders a `Tool` node whose invocation is `spawn_sub_agent` and whose child `Agent` subtree exists in `Messages`, **when** the sub-agent is still running (`Tool.status ∈ {pending, running}`), **then** the TUI renders the sub-agent's subtree in a truncated / collapsed form (e.g. last N lines, a spinner, or a single-line summary). **When** the sub-agent has finished (`Tool.status ∈ {done, failed}`), **then** the TUI renders only the summarised result from the `Tool` node's payload by default, and keeps the full subtree available behind an expand control.

#### US-12 — Clean proto break

**As** an integrator, **I want** one atomic migration of the wire contract, **so that** the code doesn't carry dual-mode compatibility debt.

- **AC-12.1** **Given** the refactor lands, **when** a client uses the old proto, **then** it fails fast with an incompatibility error (no compat shim).
- **AC-12.2** **Given** the core, TUI and agent SDK land together in the refactor, **when** they run, **then** they all speak the new proto.

### Non-functional & constraints

- **Security / transport**: Localhost-only; no authentication. Unchanged from today.
- **Concurrency**: No formal target; the design must cleanly support "several" concurrent sessions and multi-TUI attach without lock contention stalls (leave concrete numbers to the architecture phase).
- **Ordering**: Within one session, diffs are delivered in the order they were produced; across sessions no ordering guarantee.
- **Failure mode**: Agent crash → `Error` node + queue paused until user acts (AC-3.4). Core crash → all sessions lost (no persistence).
- **Compatibility**: Break freely; no version shim (AC-12).

### Out of scope for this refactor

- **Commands runtime**: Module kind stays discoverable; no runtime, no RPCs for commands. Spec, data model, and lifecycle for commands will be a separate phase.
- **GUI and Web UIs**: Not in this spec; the diff-based subscribe API should be general enough to support them later, but no specific requirements captured here.
- **Multi main agent per session**: Explicitly deferred. Design may leave room (e.g. allow the parent-id rule to extend), but no AC for it now.
- **Sequence / resume for diffs**: Deferred. Current model is "refetch + re-subscribe" on reconnect.
- **Core-side session persistence**: Deferred. In-memory only.
- **Tool allowlist / policy enforcement**: Registry is session-scoped at the RPC boundary so policy can be added later, but no filtering rules are part of this spec.
- **Per-session provider override**: Deferred to the Commands phase. Sessions only inherit the global default in this refactor.

### Edge cases & failure paths covered

- Session auto-created on TUI attach, destroyed on last-TUI detach.
- Explicit `DestroySession` cancels agents and drops the session.
- Queue dispatch blocked because default agent module isn't registered → top-level `Error` node + prompt removed.
- Main agent crashes mid-turn → `Error` node under the Agent node + queue paused.
- Session-wide Stop → kills running agent + cascading sub-agents, clears queue.
- Sub-agent still running when parent would stop → invariant violation → both killed + `Error` node.
- Provider global config reloaded → existing sessions keep their effective provider; new sessions pick up the new default.
- Agent attempts to mutate non-message session state → rejected.

### Completeness checklist

- [x] Session lifecycle (create, list, attach, detach, destroy, not persisted)
- [x] Prompt queue semantics (order, pause on agent failure, session-wide stop)
- [x] Node model (types, parent rules, no deletion, one-updated streaming)
- [x] Update semantics (partial patch via `UpdateNode`)
- [x] Error node placement (top-level for session-level, Agent-parented for per-turn)
- [x] Diff protocol (full-state on attach, diffs after; refetch on reconnect)
- [x] Main agent identification and per-turn lifetime
- [x] Sub-agent spawning, nesting, cascading stop, subtree + summarised Tool-result coexistence
- [x] Sub-agent TUI truncation behaviour
- [x] Agent boundary (message-emitter only)
- [x] Commands — deferred, module kind preserved
- [x] Per-session provider override — deferred to Commands phase
- [x] TUI migration (no local state, `HistorySync` removed, no import / export)
- [x] Cancellation model (session-wide Stop)
- [x] Non-functional: loopback-only auth, no formal concurrency targets, proto break
- [x] Provider / config scoping per session (inherit-only, no override)
- [x] Tool registry / InvokeTool session-scoped
- [x] Debug nodes in graph, hidden-by-default in TUI

## Architecture

> Ratified on 2026-04-17 after Architect phase (Q1-Q7 + sub-agent re-evaluation + conversation-history derivative + ready-flag gate).
> See `decisions.md` for the per-decision log.

### Summary

Refactor `scarllet-core` from a single-chat daemon into a multi-session orchestrator. Introduce per-session state (queue, flat node graph, connected agents, effective config, subscribers), replace the single global `AgentRegistry` with per-session registries, and restructure the proto around **typed nodes + partial updates + diffs**. Main agents and sub-agents are both per-turn processes distinguished by their `parent_id` (session_id or agent_id). Sub-agent spawn is a core-internal branch inside `tools::invoke`, exposed to agents through the normal tool registry. The TUI drops its local `session.json` store and becomes a thin projection of core's state via `AttachSession` (whose first diff carries the full `SessionState`). Scoped atomically; no compat shim.

### Locked decisions

| # | Decision |
|---|---|
| Framing | Confirmed per spec. |
| Boundary | New sub-directories `session/`, `agents/`, `service/` inside `scarllet-core/src/`. No new crates. |
| Q1 core layout | Grouped sub-directories (session / agents / service). |
| Q2 sub-agent surface | Core-internal branch inside `tools::invoke`; synthetic manifest entry in the tool registry so agents see it as a normal tool. |
| Q3 attach hydration | `AttachSession` first diff carries full `SessionState`; subsequent diffs are deltas. |
| Q4 agent SDK | Extend `scarllet-sdk::agent` (no new crate). |
| Q5 impl approach | Atomic PR. Intermediate commits must still compile; old proto & handlers do **not** coexist with the new ones. |
| Q6 per-turn RPCs | Keep unary `GetActiveProvider(session_id)` + `GetToolRegistry(session_id)` called by the agent per turn. |
| Q7 resume | Implicit — `StopSession` clears the Paused state along with clearing the queue. |
| Conversation history | Unary `GetConversationHistory(session_id)`; server derives from the session node graph; only main agents call it. |
| Missing-default-agent timing | Strict — emit an `Error` node immediately (no grace wait for the initial registry scan). |

### Proto surface

`scarllet-proto/proto/orchestrator.proto` is fully rewritten. Single `package scarllet`; old contents replaced.

#### Service

```proto
service Orchestrator {
  // Session lifecycle (unary)
  rpc CreateSession(CreateSessionRequest) returns (CreateSessionResponse);
  rpc ListSessions(ListSessionsRequest)   returns (ListSessionsResponse);
  rpc DestroySession(DestroySessionRequest) returns (DestroySessionResponse);
  rpc GetSessionState(GetSessionStateRequest) returns (SessionState);

  // TUI-facing
  rpc AttachSession(AttachSessionRequest) returns (stream SessionDiff);
  rpc SendPrompt(SendPromptRequest) returns (SendPromptResponse);
  rpc StopSession(StopSessionRequest) returns (StopSessionResponse);

  // Agent-facing bidi stream
  rpc AgentStream(stream AgentOutbound) returns (stream AgentInbound);

  // Session-scoped per-turn unary RPCs used by agents
  rpc GetActiveProvider(GetActiveProviderRequest) returns (ActiveProviderResponse);
  rpc GetToolRegistry(GetToolRegistryRequest) returns (GetToolRegistryResponse);
  rpc GetConversationHistory(GetConversationHistoryRequest) returns (ConversationHistoryResponse);
  rpc InvokeTool(InvokeToolRequest) returns (InvokeToolResponse);
}
```

#### Node model

```proto
enum NodeKind {
  NODE_KIND_UNSPECIFIED = 0;
  USER = 1;
  AGENT = 2;
  THOUGHT = 3;
  TOOL = 4;
  RESULT = 5;
  DEBUG = 6;
  TOKEN_USAGE = 7;
  ERROR = 8;
}

message Node {
  string id = 1;
  optional string parent_id = 2;   // absent = top-level
  NodeKind kind = 3;
  uint64 created_at = 4;
  uint64 updated_at = 5;
  oneof payload {
    UserPayload user = 10;
    AgentPayload agent = 11;
    ThoughtPayload thought = 12;
    ToolPayload tool = 13;
    ResultPayload result = 14;
    DebugPayload debug = 15;
    TokenUsagePayload token_usage = 16;
    ErrorPayload error = 17;
  }
}

message UserPayload        { string text = 1; string working_directory = 2; }
message AgentPayload       { string agent_module = 1; string agent_id = 2; string status = 3; } // running|finished|failed
message ThoughtPayload     { string content = 1; }
message ToolPayload        { string tool_name = 1; string arguments_preview = 2; string arguments_json = 3; string status = 4; uint64 duration_ms = 5; string result_json = 6; }
message ResultPayload      { string content = 1; string finish_reason = 2; }
message DebugPayload       { string source = 1; string level = 2; string message = 3; }
message TokenUsagePayload  { uint32 total_tokens = 1; uint32 context_window = 2; }
message ErrorPayload       { string source = 1; string message = 2; }
```

Partial updates use a dedicated `NodePatch`:

```proto
message NodePatch {
  // Absent fields = unchanged; present fields overwrite OR append (see merge rules below).
  optional string agent_status = 1;
  optional string thought_content = 2;        // APPEND: server concatenates
  optional string tool_status = 3;
  optional uint64 tool_duration_ms = 4;
  optional string tool_result_json = 5;
  optional string result_content = 6;         // APPEND
  optional string result_finish_reason = 7;
  optional string error_message = 8;
  optional uint32 token_total = 9;
  optional uint32 token_window = 10;
}
```

Merge rules: `thought_content` and `result_content` are **append** (server concatenates the delta); all other patch fields are **replace**. Agents send only the delta for appended fields; the `scarllet-sdk::agent` helpers are the only supported write path.

#### TUI-facing

```proto
message AttachSessionRequest { optional string session_id = 1; } // empty = auto-create
message SessionDiff {
  oneof payload {
    Attached attached = 1;                     // FIRST message only; carries SessionState
    NodeCreated node_created = 2;              // full Node
    NodeUpdated node_updated = 3;              // { node_id, patch, updated_at }
    QueueChanged queue_changed = 4;            // { queued: [QueuedPrompt] } - full queue snapshot
    AgentRegistered agent_registered = 5;      // { agent_id, agent_module, parent_id }
    AgentUnregistered agent_unregistered = 6;  // { agent_id }
    StatusChanged status_changed = 7;          // RUNNING | PAUSED
    SessionDestroyed destroyed = 8;            // terminal
  }
}

message SendPromptRequest  { string session_id = 1; string text = 2; string working_directory = 3; }
message StopSessionRequest { string session_id = 1; }
```

#### Agent-facing

```proto
message AgentOutbound {
  oneof payload {
    AgentRegister register = 1;         // { desired_agent_id (optional), agent_module, parent_id }
    CreateNode create_node = 2;         // full Node (id may be blank for server-assigned)
    UpdateNode update_node = 3;         // { node_id, patch }
    TurnFinished turn_finished = 4;     // { finish_reason } - asserts Result node already emitted
    AgentFailure failure = 5;           // { message } - unrecoverable before Result
  }
}
message AgentInbound {
  oneof payload {
    AgentTask task = 1;                 // { session_id, agent_id, parent_id, prompt, working_directory }
    CancelNow cancel = 2;               // session-wide stop / cascade kill
  }
}
```

Server validates every `CreateNode`:

- `User` / `Agent` / `Error` nodes MAY be top-level; all other kinds MUST have `parent_id`.
- `Thought` / `Tool` / `Result` / `Debug` / `TokenUsage` must parent onto an `Agent` node owned by the calling agent.
- `Agent` nodes are created by core, not agents. Core creates one when dispatching a main-agent task or when handling `spawn_sub_agent`.

#### What disappears

`HistoryEntry` (moves behind `GetConversationHistory`), `TuiMessage`, old `CoreEvent`, old `AgentMessage` / `AgentInstruction`, `HistorySync`, `AttachTui`, `AgentStartedEvent`, `AgentThinkingEvent`, `AgentResponseEvent`, `AgentErrorEvent`, `SystemEvent`, `ProviderInfoEvent`, `AgentToolCallEvent`, `DebugLogEvent`, `TokenUsageEvent`, old `AgentTask`, `AgentProgressMsg`, `AgentResultMsg`, `AgentTokenUsageMsg`, `AgentToolCallMsg`, `AgentBlock`, `EmitDebugLog`, `DebugLogRequest`, `CancelPrompt`, `PromptMessage`.

### Core internal structure

```
scarllet-core/src/
├── main.rs                  (bootstrap — adapts to the new OrchestratorService)
├── registry.rs              (unchanged — ModuleRegistry for discovered modules)
├── watcher.rs               (updated — on config reload, only new sessions inherit; existing sessions keep their snapshot)
├── tools.rs                 (updated — accepts session_id + agent_id; branches on spawn_sub_agent)
│
├── session/
│   ├── mod.rs               (Session, SessionRegistry, SessionStatus)
│   ├── state.rs             (SessionState snapshot builder for Attached first-diff + GetSessionState)
│   ├── queue.rs             (QueuedPrompt + SessionQueue FIFO)
│   ├── nodes.rs             (NodeStore — append-only with partial-patch; NodeKind/Payload helpers)
│   ├── diff.rs              (SessionDiff builders; per-mutation broadcast helper)
│   └── subscribers.rs       (SubscriberSet<SessionDiff> with try_send + auto-prune)
│
├── agents/
│   ├── mod.rs               (AgentRecord + per-session AgentRegistry)
│   ├── spawn.rs             (spawn_main_agent, spawn_sub_agent)
│   ├── stream.rs            (AgentStream handler: register / create / update / turn_finished / failure)
│   └── routing.rs           (on_send_prompt → enqueue + try_dispatch_main; invariants)
│
└── service/
    ├── mod.rs               (OrchestratorService struct + tonic trait impl; thin)
    ├── session_rpc.rs       (Create/List/Destroy/GetSessionState/AttachSession/SendPrompt/StopSession)
    ├── tool_rpc.rs          (GetToolRegistry/InvokeTool — session_id-aware; routes spawn_sub_agent)
    └── agent_rpc.rs         (GetActiveProvider/GetConversationHistory; AgentStream → agents::stream)
```

**Why this split:** `session/` owns *state* (pure data + mutation, no IO). `agents/` owns *processes* (spawn + stream + dispatch). `service/` owns *gRPC wiring* (decode / validate / delegate; no business logic). Module watcher, registry and tool invocation stay at the crate root because they are cross-cutting.

### Core data model

```rust
// session/mod.rs
pub struct Session {
    pub id: String,
    pub created_at: SystemTime,
    pub last_activity: SystemTime,
    pub status: SessionStatus,                  // Running | Paused
    pub config: SessionConfig,                  // snapshot of global provider at create time
    pub queue: SessionQueue,
    pub nodes: NodeStore,
    pub agents: AgentRegistry,                  // per-session
    pub subscribers: SubscriberSet<SessionDiff>,
}

pub struct SessionRegistry {
    sessions: RwLock<HashMap<String, Arc<RwLock<Session>>>>,
}

// session/nodes.rs
pub struct NodeStore {
    order: Vec<String>,                         // creation order
    by_id: HashMap<String, Node>,
    children_of: HashMap<String, Vec<String>>,  // for fast subtree walks
}
impl NodeStore {
    pub fn create(&mut self, node: Node) -> Result<&Node, InvariantError>;
    pub fn update(&mut self, id: &str, patch: NodePatch) -> Result<&Node, InvariantError>;
    pub fn get(&self, id: &str) -> Option<&Node>;
    pub fn all(&self) -> impl Iterator<Item = &Node>;
    pub fn descendants_of(&self, id: &str) -> impl Iterator<Item = &Node>;
}

// session/queue.rs
pub struct QueuedPrompt { pub id: String, pub text: String, pub working_directory: String, pub user_node_id: String }
pub struct SessionQueue { items: VecDeque<QueuedPrompt> }

// agents/mod.rs
pub struct AgentRecord {
    pub agent_id: String,
    pub agent_module: String,
    pub parent_id: String,              // session_id for main, agent_id for sub
    pub pid: Option<u32>,
    pub tx: mpsc::Sender<AgentInbound>,
    pub agent_node_id: String,          // the Agent node core created for this turn
}
pub struct AgentRegistry {
    by_id: HashMap<String, AgentRecord>,
    main_agent_id: Option<String>,      // at most one (multi-main deferred)
    sub_agent_waiters: HashMap<String, oneshot::Sender<Result<ResultPayload, String>>>,
}
```

### Key flows

#### Prompt → main-agent dispatch

```
TUI: SendPrompt(session_id, text, cwd)
  → service::session_rpc
      → session.write():
          nodes.create(User { text, cwd })                # → broadcast NodeCreated
          queue.push(QueuedPrompt)                        # → broadcast QueueChanged
      → agents::routing::try_dispatch_main(session)

try_dispatch_main:
  if session.status == Paused: return
  if session.agents.main_agent_id.is_some(): return       # turn still running
  let q = session.queue.pop_front()?;                     # → broadcast QueueChanged
  let Some(module) = global_config.default_agent else {
      nodes.create(Error { top-level, "default agent not registered" })  # AC-3.3
      return
  };
  let agent_id = Uuid::new_v4();
  nodes.create(Agent { id=agent_id, parent=None, module, status=running })
                                                          # → broadcast NodeCreated
  agents::spawn::spawn_main_agent(session_id, agent_id, module, prompt, cwd);
  # Process starts; connects back via AgentStream; calls Register; core sends AgentTask.
```

#### Agent turn lifecycle

```
Agent process starts → connects to SCARLLET_CORE_ADDR via gRPC → opens AgentStream.
Agent: Register { agent_id, agent_module, parent_id } (all from env vars).
Core: validates — if parent_id == session_id and a main already exists, rejects;
      otherwise stores AgentRecord, broadcasts AgentRegistered, and sends AgentTask.
Agent: GetActiveProvider(session_id), GetToolRegistry(session_id), GetConversationHistory(session_id).
Agent: runs the LLM ↔ tool loop. Each iteration:
  - CreateNode(Thought) then UpdateNode(append delta) as tokens stream
  - For each tool call: CreateNode(Tool pending), InvokeTool(session_id, agent_id, tool_name, args),
    UpdateNode(Tool status / duration / result)
  - On completion without tool_calls: CreateNode(Result), TurnFinished
Core on TurnFinished:
  - sets the Agent node's status=finished (UpdateNode server-side, broadcasts NodeUpdated)
  - removes AgentRecord, broadcasts AgentUnregistered
  - agent process exits on its own (stream closes naturally); core reaps PID
  - main_agent_id cleared → try_dispatch_main pulls next queue item (if any)
```

#### Sub-agent spawn (core-internal)

```
Parent agent: CreateNode(Tool { parent=<parent agent_node>, tool_name="spawn_sub_agent", status=pending, args_json={agent_module, prompt} })
Parent agent: InvokeTool { session_id, agent_id, tool_name="spawn_sub_agent", input_json={agent_module, prompt} }

service::tool_rpc::invoke_tool routes:
  if tool_name == "spawn_sub_agent":
      agents::spawn::handle_spawn_sub_agent(session, parent_agent_id, agent_module, prompt) -> ResultPayload
  else:
      tools::invoke_external(session_id, tool_name, input_json)

handle_spawn_sub_agent:
  let child_id = Uuid::new_v4();
  nodes.create(Agent { id=child_id, parent=<parent_tool_node_id>, module=agent_module, status=running })
  agents::spawn::spawn_sub_agent_process(session_id, child_id, parent_agent_id, agent_module, prompt)
  let (tx, rx) = oneshot::channel();
  session.agents.sub_agent_waiters.insert(child_id, tx);
  match rx.await {
    Ok(result_payload) => InvokeToolResponse { success: true,  output_json: summarise(result_payload) }
    Err(e)             => InvokeToolResponse { success: false, error_message: e }
  }

# When the sub-agent emits Result + TurnFinished, agents::stream notices the agent is in
# sub_agent_waiters, fires tx.send(result_payload), removes the waiter.
# InvokeToolResponse returns to the parent agent, which then updates its Tool node.
```

Invariant (AC-8.4): before accepting a `TurnFinished`, the stream handler checks `session.agents.any_descendant_running(parent_agent_id)`. If yes, `TurnFinished` is rejected; a top-level `Error` node is created and both parent and descendants are cancelled.

#### Session-wide Stop

```
TUI: StopSession(session_id)
Core:
  for each agent in reverse topological order (sub-agents before parents):
      tx.send(CancelNow); kill PID after grace period; emit Error node; AgentUnregistered
  session.queue.clear()                                   # → broadcast QueueChanged
  if session.status == Paused:
      session.status = Running                            # → broadcast StatusChanged
```

#### Attach + hydration

```
TUI: AttachSession(session_id or empty)
Core:
  if session_id empty: CreateSession() implicitly
  register subscriber in session.subscribers
  send Attached { SessionState { id, status, config, queue, nodes, agents } } immediately
  subsequent mutations broadcast diffs to this subscriber
On last-TUI detach:
  session.subscribers.len() == 0 → DestroySession path (cancel + drop + broadcast Destroyed)
```

#### Agent failure mid-turn (AC-3.4)

```
Agent disconnects without TurnFinished OR sends AgentFailure:
  nodes.create(Error { parent=<agent's Agent node>, source=agent_module, message })
  nodes.update(<agent's Agent node>, { agent_status="failed" })
  session.agents.deregister(agent_id)                     # → broadcast AgentUnregistered
  session.status = Paused                                 # → broadcast StatusChanged
  # queue keeps items; try_dispatch_main short-circuits on status=Paused
  # user must StopSession to clear queue + resume
```

### Agent SDK (`scarllet-sdk::agent`)

New module in the existing crate. Thin wrapper over the tonic client.

```rust
// scarllet-sdk/src/agent/mod.rs
pub struct AgentSession {
    pub session_id: String,
    pub agent_id: String,
    pub parent_id: String,
    client: OrchestratorClient<Channel>,
    out_tx: mpsc::Sender<AgentOutbound>,
    in_rx: tonic::Streaming<AgentInbound>,
}
impl AgentSession {
    pub async fn connect() -> Result<Self, AgentSdkError>;        // reads env, opens stream, registers
    pub async fn next_task(&mut self) -> Option<AgentTask>;       // blocks until Task or Cancel

    // Node emission
    pub async fn create_thought(&self, parent_agent_node: &str) -> Result<String, _>;
    pub async fn append_thought(&self, node_id: &str, chunk: &str) -> Result<(), _>;

    pub async fn create_tool(&self, parent_agent_node: &str, name: &str, preview: &str, args: &str) -> Result<String, _>;
    pub async fn update_tool_status(&self, node_id: &str, status: ToolStatus, duration_ms: u64, result_json: &str) -> Result<(), _>;

    pub async fn emit_result(&self, content: &str, finish_reason: &str) -> Result<(), _>;   // CreateNode(Result) + TurnFinished
    pub async fn emit_failure(&self, message: &str) -> Result<(), _>;

    pub async fn emit_debug(&self, level: &str, message: &str) -> Result<(), _>;
    pub async fn emit_token_usage(&self, total: u32, window: u32) -> Result<(), _>;
    pub async fn emit_error(&self, message: &str) -> Result<(), _>;

    // Per-turn unary queries
    pub async fn get_provider(&mut self) -> Result<ActiveProviderResponse, _>;
    pub async fn get_tools(&mut self) -> Result<Vec<ToolInfo>, _>;
    pub async fn get_history(&mut self) -> Result<Vec<HistoryEntry>, _>;

    // Convenience: wraps InvokeTool("spawn_sub_agent", ...)
    pub async fn spawn_sub_agent(&mut self, module: &str, prompt: &str) -> Result<String, _>;
}
```

### TUI migration

- **Delete**: `session.rs` (both `FileSessionRepository` and `NullSessionRepository`), the `SessionRepository` trait, every `save_session` / `load_from_session` call.
- **Delete**: any `HistorySync` construction in `events::handle_core_event`.
- **Replace `ChatEntry`**: TUI maintains a local `NodeStore` mirror (same shape as core's) so it can render subtrees. `tool_calls: HashMap` is removed — tool-call state lives in `Tool` nodes.
- **`connection.rs`**: after the channel is up, call `AttachSession(None)` (letting core auto-create), read the first `Attached` diff to hydrate, then loop applying diffs.
- **`events.rs`**:
  - Enter → `SendPrompt(session_id, text, cwd)`
  - Esc → `StopSession(session_id)` (replaces `CancelPrompt`)
  - Ctrl-N → `DestroySession(session_id)` then `AttachSession(None)`
- **Render**: iterate nodes in creation order, build a tree via `parent_id`, render top-level nodes in order. For a `Tool` node whose `tool_name == "spawn_sub_agent"`, render the sub-agent subtree collapsed while `tool_status ∈ {pending, running}`; when `done` / `failed`, show only the `result_json` summary (AC-11.5). `Debug` nodes are filtered out unless `SCARLLET_DEBUG=true`.

### Impacted paths

**Rewritten:**

- `packages/rust/scarllet-proto/proto/orchestrator.proto`

**Rewritten / restructured:**

- `packages/rust/scarllet-core/src/main.rs`
- `packages/rust/scarllet-core/src/service.rs` → split into `service/{mod,session_rpc,tool_rpc,agent_rpc}.rs`
- `packages/rust/scarllet-core/src/sessions.rs` → split into `session/{mod,state,queue,nodes,diff,subscribers}.rs`
- `packages/rust/scarllet-core/src/agents.rs` → split into `agents/{mod,spawn,stream,routing}.rs`
- `packages/rust/scarllet-core/src/tasks.rs` → absorbed into `agents/spawn.rs` (file removed)
- `packages/rust/scarllet-core/src/events.rs` → replaced by `session/diff.rs` (file removed)
- `packages/rust/scarllet-core/src/routing.rs` → moved into `agents/routing.rs` (file removed from root)
- `packages/rust/scarllet-core/src/tools.rs` — updated signature (`session_id`, `agent_id`); branches on `spawn_sub_agent`
- `packages/rust/scarllet-core/src/watcher.rs` — config watcher stops broadcasting globally; existing sessions keep their snapshot

**Updated in place:**

- `packages/rust/scarllet-sdk/src/lib.rs` — add `pub mod agent;`
- `packages/rust/scarllet-sdk/src/agent/mod.rs` — new
- `packages/rust/agents/default/src/main.rs` — rewrite to use `scarllet_sdk::agent::AgentSession`

**Rewritten:**

- `packages/rust/scarllet-tui/src/app.rs`
- `packages/rust/scarllet-tui/src/events.rs`
- `packages/rust/scarllet-tui/src/connection.rs`
- `packages/rust/scarllet-tui/src/render.rs` and `widgets/chat_message.rs`
- `packages/rust/scarllet-tui/src/main.rs`

**Deleted:**

- `packages/rust/scarllet-tui/src/session.rs`

**Untouched:**

- `packages/rust/scarllet-llm/*`
- `packages/rust/tools/*`
- `packages/rust/scarllet-sdk/src/{config,manifest,lockfile}.rs`
- `packages/rust/scarllet-core/src/registry.rs`

### Implementation plan (ordered efforts)

All efforts land on one feature branch and merge as a single atomic PR. Each effort leaves `cargo check --workspace` green.

| # | Effort | Outcome |
|---|---|---|
| 1 | Proto rewrite — replace `orchestrator.proto` with the new surface; `scarllet-proto` compiles. | `nx run scarllet-proto:build` green with new types; nothing uses them yet. |
| 2 | Core scaffold — session module (`session/` sub-tree): `Session`, `NodeStore`, `SessionQueue`, `SubscriberSet`, `SessionDiff` builders, `SessionRegistry`. Pure data; no RPCs wired. Unit tests. | `nx run scarllet-core:build` green. |
| 3 | Core scaffold — agents module (`agents/` sub-tree): `AgentRecord`, per-session `AgentRegistry`, `spawn.rs` stubs, `stream.rs` skeleton, `routing.rs` skeleton. | Build green; no runtime yet. |
| 4 | Service layer — session RPCs (`service/session_rpc.rs`): `CreateSession`, `ListSessions`, `DestroySession`, `GetSessionState`, `AttachSession` (first-diff hydration), `SendPrompt`, `StopSession`. Wires prompt → queue → User node → `try_dispatch_main`. | TUI can attach, get empty state, see User node + QueueChanged after SendPrompt; no agent runs yet. |
| 5 | Service layer — agent stream + unary RPCs (`service/agent_rpc.rs`): `AgentStream` handler delegating to `agents::stream`; `GetActiveProvider(session_id)`, `GetToolRegistry(session_id)`, `GetConversationHistory(session_id)`. Implement `agents::spawn::spawn_main_agent`. | External process can connect, register, receive AgentTask, emit nodes; AttachSession shows them live. |
| 6 | Service layer — tool RPC + sub-agent built-in (`service/tool_rpc.rs`): `InvokeTool(session_id, agent_id, tool_name, input)`; `tools.rs` branch for `spawn_sub_agent`; `agents::spawn::handle_spawn_sub_agent` with `oneshot` waiter; invariant check for `TurnFinished` with running sub-agents. Synthetic manifest in the tool registry. | Sub-agent end-to-end works. |
| 7 | Session-wide Stop + Paused recovery: `StopSession` cascades cancels (sub-agents first); `AgentFailure` / disconnect-before-TurnFinished flips `Paused`; `try_dispatch_main` gates on status; `StopSession` clears queue + resets to `Running`. | All lifecycle ACs pass. |
| 8 | Agent SDK + default-agent migration: add `scarllet_sdk::agent`; rewrite `agents/default/src/main.rs` to use it; remove `scarllet-proto` direct deps from `agents/default`. | `agents/default` runs end-to-end against the new core. |
| 9 | TUI migration: rewrite `app.rs`, `events.rs`, `connection.rs`, `render.rs`, `widgets/chat_message.rs`, `main.rs`; delete `session.rs`. Implement node-tree rendering + sub-agent truncation (AC-11.5) + debug-flag filtering. | TUI fully interoperable; Ctrl-N destroys + creates session; multi-TUI attach works. |
| 10 | Cleanup + E2E verification: remove any remaining `allow(dead_code)`, run clippy across the workspace, manual end-to-end with a real LLM provider, update inline docs. | All `nx` targets green; manual smoke test passes. |

### Verification plan

Per-effort baseline (PowerShell-friendly):

```powershell
npx nx run scarllet-proto:build
npx nx run scarllet-sdk:build
npx nx run scarllet-core:build
npx nx run scarllet-llm:build
npx nx run scarllet-tui:build
npx nx run scarllet-core:test
npx nx run scarllet-proto:test
npx nx run scarllet-sdk:test
npx nx run scarllet-tui:test
npx nx run scarllet-core:lint
npx nx run scarllet-tui:lint
```

Workspace-wide fallback (agents / tools do not have `project.json`):

```powershell
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

End-to-end smoke (after effort 10):

```powershell
npx nx run scarllet-tui:run
# In the TUI:
#   1. Type a prompt → User node appears → default agent spawns → streams → Result node
#   2. Press Ctrl-N → session destroyed, new empty session
#   3. Connect a second TUI → attach succeeds, receives current state, both stay in sync
#   4. Ask the agent to spawn a sub-agent → Tool(spawn_sub_agent) node with nested Agent subtree appears; collapses on completion
#   5. Press Esc mid-turn → session stops cleanly; agent + sub-agent both die
```

### Risks & trade-offs

| Area | Note |
|---|---|
| Atomic PR size | Large diff; mitigated by 10-effort review-sized commits that each still compile. |
| NodeStore unbounded growth | In-memory only; user kills the session to release. Future: per-session node cap or compaction. |
| `Attached` first-diff payload | Long sessions produce a large first message. Acceptable in-process; pagination is a future concern. |
| Append semantics on `thought_content` / `result_content` | Agent and core must agree; only the SDK helpers `append_thought` / (`emit_result` writes a final content on first call) are supported write paths. |
| +1 RPC per turn for history | Accepted per Q6 pattern; keeps `AgentTask` lean and history derivation centralised. |
| Shared `AgentStream` RPC for main + sub | `parent_id` distinguishes; simpler but any dispatcher bug affects both. |
| Missing default agent | Strict — immediate `Error` node, no grace wait. |
| Proto package stays `scarllet` | Simpler; old clients get unstructured gRPC errors rather than a pretty version-mismatch. Fine for loopback. |

### SOLID / YAGNI / KISS / DRY notes

- **SRP**: each core sub-dir owns one concern (state / processes / gRPC). `scarllet-sdk::agent` owns agent-to-core comms; nothing else.
- **OCP**: new node kinds are added by extending `NodeKind` + a payload variant + a `NodePatch` field; not by editing switch statements across layers.
- **DIP**: `scarllet-core` depends on `scarllet-proto` generated types; `agents/default` depends on `scarllet-sdk::agent` (stable surface), not raw proto. TUI depends on `scarllet-proto` directly (per current repo convention).
- **ISP**: `AgentOutbound` / `AgentInbound` / `SessionDiff` are narrow oneofs; TUI unary RPCs each do one thing.
- **KISS**: no new crates; existing Nx targets reused; single proto package; sub-agent spawn is a branch inside `tools::invoke` rather than a new surface.
- **DRY**: agent-spawn lives only in `agents::spawn`; node mutation + diff broadcast lives only in `session::nodes` + `session::diff`; no duplication between main-agent and sub-agent paths.
- **YAGNI (deferred per spec)**: Commands runtime, per-session provider override, persistence, sequence-numbered diffs, tool allowlists, multi-main-agent.
