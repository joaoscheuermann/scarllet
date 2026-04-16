# Decision Log: agent-history-hydration

### 2026-04-16 12:30 - Architect

**Context**: Need to send conversation history to agents. Two approaches: add field to AgentTask (A) vs new AgentInstruction wrapper (B).
**Decision**: Option B — new `AgentInstruction` oneof wrapping `AgentTask` and `AgentHistorySync`.
**Rationale**: Clean ISP — distinct message types for tasks vs hydration. The stream return type explicitly communicates that agents receive more than just tasks.
**Alternatives considered**: Option A (add `repeated HistoryEntry` to `AgentTask`) — simpler but semantically overloads AgentTask with a non-task purpose.

### 2026-04-16 12:30 - Architect

**Context**: Where does Core get conversation history for agent hydration?
**Decision**: TUI sends `HistorySync` to Core on connect. Core also accumulates from prompt/response events.
**Rationale**: DIP — agent doesn't need to know about TUI's session file. TUI is the source of truth for persisted history; Core supplements with in-session accumulation.
**Alternatives considered**: Core reads session.json directly — violates SRP; Core accumulates only — doesn't work across Core restarts.

### 2026-04-16 12:35 - Decomposer

**Context**: Breaking the approved architecture into incremental deliverables.
**Decision**: Three efforts — (1) Proto changes + Core plumbing + agent handler (full compile), (2) TUI sends history on connect, (3) End-to-end verification and edge cases.
**Rationale**: Effort 1 is the largest but must be atomic — proto + Core + agent channel type changes all need to compile together. Effort 2 is the TUI-side wiring. Effort 3 is the integration test pass. Each effort has an observable outcome: compile, Core logs, conversation continuity.
**Alternatives considered**: Splitting proto and Core into separate efforts — rejected because changing the `AgentStream` return type and the `AgentRegistry` channel type must happen atomically for the code to compile.

### 2026-04-16 12:45 - Executor

**Context**: Effort 1 (Proto changes and Core plumbing) completed.
**Decision**: Marked done — all crates compile, 12/12 Core tests pass.
**Rationale**: Proto schema updated (AgentInstruction wrapper, HistoryEntry, HistorySync). Core stores history, sends hydration on agent register, accumulates from prompts/responses. Agent handles HistorySync to seed history. User history appended in attach_tui handler (simpler than threading through route_prompt).

### 2026-04-16 12:50 - Executor

**Context**: Effort 2 (TUI sends history to Core on connect) completed.
**Decision**: Marked done — TUI compiles, 10/10 tests pass.
**Rationale**: On `Connected` event, TUI maps User and completed Agent entries to `HistoryEntry` values and sends `HistorySync` to Core. Only Text blocks included (not Thought), empty content filtered out.
