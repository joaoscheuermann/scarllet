# Decision Log: tui-chat-refactor

### 2026-04-12 14:10 - Architect

**Context**: TUI refactor requires real-time communication between TUI and Core for chat-style agent interaction. Three options evaluated: server-streaming RPC, polling, bidirectional streaming.
**Decision**: Bidirectional streaming (`AttachTui(stream TuiMessage) returns (stream CoreEvent)`) — Option C.
**Rationale**: Most powerful and extensible. Supports future multi-prompt, server-initiated events, concurrent agents. Aligns with original architecture vision. User explicitly chose this option over simpler alternatives.
**Alternatives considered**: (A) Server-streaming RPC — simpler but one-directional, would need replacement later. (B) Polling via GetAgentStatus — simplest but high latency, poor UX for chat.

### 2026-04-12 14:18 - Decomposer

**Context**: Breaking the approved TUI chat refactor architecture into incremental, runnable deliverables. Architecture touches 4 crates (proto, sdk, core, tui) with a full TUI rewrite, new bidirectional streaming RPC, session registry, and prompt routing.
**Decision**: 4 vertical-slice efforts: (1) Proto + Core stub + TUI connecting screen, (2) TUI chat layout with local message entry, (3) Core session registry + prompt routing + TUI event rendering, (4) Input locking + error handling + polish.
**Rationale**: Each effort yields a runnable observable result. Effort 1 establishes the connection foundation (avoids a "types only" effort by including the connecting screen). Effort 2 builds the full local UI independently of Core integration. Effort 3 is the largest but cannot be split without creating a "sends prompt but can't see response" non-observable half. Effort 4 layers behavioral safeguards on top of the functional pipeline.
**Alternatives considered**: 5-effort split (separating proto/SDK from TUI connecting screen) — rejected because proto-only effort has nothing runnable. 3-effort split (merging efforts 3+4) — rejected because effort 3 is already the largest slice and adding error handling would make it too big for one focused pass.

### 2026-04-12 14:30 - Executor

**Context**: Effort 1 (Proto streaming contract + Core AttachTui stub + TUI connecting screen) completed.
**Decision**: Marked done — all verification criteria met (build passes, proto codegen works, Core compiles with AttachTui, TUI compiles with new two-screen model).
**Rationale**: All 4 crates build successfully. Proto has 8 new message types + AttachTui RPC. SDK has is_pid_alive. Core has minimal AttachTui stub. TUI fully rewritten with connecting/chat screens.
**Alternatives considered**: None

### 2026-04-12 14:45 - Executor

**Context**: All 4 efforts completed for tui-chat-refactor.
**Decision**: Marked ticket as done. All efforts pass build and tests (10 total: 6 Core + 4 SDK, 0 failures).
**Rationale**: Effort 2 (chat layout), 3 (session registry + prompt routing + event rendering), and 4 (input locking + error handling + polish) all build cleanly with zero warnings. Full pipeline wired end-to-end.
**Alternatives considered**: None
