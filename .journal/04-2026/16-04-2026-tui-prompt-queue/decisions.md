# Decision Log: tui-prompt-queue

### 2026-04-16 19:07 - Spec

**Context**: Choosing the execution model for queued prompts (serial vs parallel agent runs).
**Decision**: Strictly serial / FIFO. The next queued prompt only starts after the current task fully completes (streaming + tool calls done).
**Rationale**: Matches the user's mental model of "stack up follow-ups for the agent." Avoids race conditions and concurrent context-mutation issues. Keeps Core's task lifecycle simple.
**Alternatives considered**: Parallel concurrent agent tasks — rejected for complexity and unclear UX around interleaved streaming responses.

### 2026-04-16 19:07 - Spec

**Context**: Where the pending-prompt queue should live.
**Decision**: Queue is owned by Core. The TUI sends prompts immediately on submit; Core serializes execution behind the active task and reports queue state back.
**Rationale**: Single source of truth (NFR-1). Aligns with the existing pattern where Core owns the agent task lifecycle and broadcasts state to all attached TUIs. Avoids divergence between TUI-local state and Core's actual scheduling.
**Alternatives considered**: TUI-local queue that releases prompts to Core one at a time — simpler proto changes but splits queue truth across two processes; harder to reason about disconnects and multi-TUI evolution.

### 2026-04-16 19:07 - Spec

**Context**: User interaction with queued prompts (view, cancel, edit, reorder).
**Decision**: View pending prompts in a dedicated section above the input; allow deletion of a focused pending item via the Delete key. No editing or reordering.
**Rationale**: Initial scope kept minimal but with enough escape hatches that users aren't forced to wait out unwanted prompts. Delete is the only mutating interaction to keep the surface small.
**Alternatives considered**: Read-only display only (rejected — user wanted ability to remove items); full edit/reorder (deferred to later iterations).

### 2026-04-16 19:07 - Spec

**Context**: Keybinding for cancelling the current task and pausing the queue. Today ESC cancels the running task and Ctrl+C quits the app.
**Decision**: ESC cancels the currently running task AND, if the queue has pending items, transitions the queue into a paused state. Resume is performed by pressing Enter on an empty input. While paused, the input placeholder reads `Press Enter to resume queue…` and the section header reads `Pending (paused)`.
**Rationale**: Reuses the existing ESC = cancel mechanism users already know. Adds a single-state extension (paused) rather than introducing new key bindings. Resume via Enter-on-empty is discoverable through the placeholder hint and uses an otherwise no-op key.
**Alternatives considered**: Two-step ESC escalation (first cancels, second flushes), confirmation modal, dedicated resume key combo — all rejected as more cognitive overhead than necessary.

### 2026-04-16 19:07 - Spec

**Context**: Queue behaviour when an agent task fails (error, not user-cancel).
**Decision**: Failure flushes the queue silently — all pending prompts are dropped, no system message is added to chat history, the queue is left idle (not paused).
**Rationale**: An agent failure is a strong signal that downstream queued prompts (which depend on the failed task's outcome) are likely invalid. Asymmetry with user-initiated cancel (which only pauses) is intentional: the user's cancel is a "wait, let me reconsider" signal, while a failure is a "the train derailed" signal. Silent drop avoids cluttering chat history with bookkeeping noise.
**Alternatives considered**: Pause on failure (forces user to acknowledge before draining — too slow); continue on failure (likely fires invalid follow-ups); add a system message (clutter, low value).

### 2026-04-16 19:07 - Spec

**Context**: Persistence of pending prompts across disconnects, exits, and Core restarts.
**Decision**: Pending prompts live only in memory. On disconnect, they are dropped from the TUI's view and the input is locked using today's `input_locked` mechanism. On exit, no warning is shown.
**Rationale**: Keeps the feature scope tight. Persistence introduces complexity (where to store, conflict resolution on reconnect, multi-TUI implications) for a feature whose primary value is in-session ergonomics. Reusing the existing disconnect-lock keeps the disconnect UX consistent with today.
**Alternatives considered**: Persist queue with the session, warn on exit — both deferred to a later iteration if usage warrants it.

### 2026-04-16 19:07 - Spec

**Context**: Multi-TUI scope.
**Decision**: This feature assumes a single TUI is attached to Core at any given time.
**Rationale**: Multi-TUI queue semantics (shared queue vs per-TUI queue, fairness, visibility of other TUIs' submissions) is a substantial design topic in its own right. Scoping it out keeps Phase 1 focused and unblocks the common case.
**Alternatives considered**: Shared queue across TUIs, per-TUI queues — both deferred and explicitly noted in NFR-2 / Out of Scope.
