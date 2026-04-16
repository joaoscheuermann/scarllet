---
status: planning
created: 2026-04-16 19:07
slug: tui-prompt-queue
---

## Prompt

I want to be able to queue prompts instead of blocking the TUI user text input.

## Research

### Problem statement

Today, while an agent task is running, the TUI input is locked (`input_locked = true` in `packages/rust/scarllet-tui/src/app.rs`). The input shows "Waiting for agent..." and rejects keystrokes and paste. This forces the user to wait for each agent response before typing the next instruction, breaking flow for the common pattern of "let me queue up follow-ups while it works."

We want the user to keep typing during agent runs, with submitted prompts held in an ordered queue and dispatched one at a time.

### User stories

**US-1 — Queueing follow-up prompts**
- **As a** TUI user with a running agent task,
- **I want to** type and submit additional prompts without waiting for the current task to finish,
- **so that** my follow-up instructions are staged in order and fire automatically as the agent becomes free.

**US-2 — Seeing what's pending**
- **As a** TUI user,
- **I want to** see a list of the prompts I've queued but which haven't started yet,
- **so that** I know what's about to run and in what order.

**US-3 — Removing a queued prompt I no longer want**
- **As a** TUI user,
- **I want to** remove a specific pending prompt before it runs,
- **so that** I can revise my plan without having to cancel the in-flight task.

**US-4 — Pausing everything when I need to reconsider**
- **As a** TUI user,
- **I want to** cancel the currently running task and temporarily halt the queue with one action,
- **so that** I can review what's queued, remove items, and resume on my own schedule.

### Acceptance criteria

#### AC-1 — Input remains editable during execution
- **Given** an agent task is currently running and the user's input focus is on the input pane,
- **When** the user types characters, pastes text, or uses normal editing keys,
- **Then** the input accepts them exactly as it does when no task is running; no "Waiting for agent..." state is shown.

#### AC-2 — Submitting while a task runs enqueues the prompt
- **Given** an agent task is running (or the queue is non-empty for any reason),
- **When** the user presses Enter on a non-empty input,
- **Then** the input clears, the submitted text is sent to Core, and Core holds it in a pending queue behind the active task. No new user message appears in chat history yet.

#### AC-3 — Pending list is visible only when needed
- **Given** the queue contains one or more pending prompts,
- **When** the TUI renders,
- **Then** a "Pending" section appears directly above the input pane showing a numbered list (1 = next to run, top of section) of single-line truncated previews of each pending prompt.
- **And given** the queue is empty,
- **When** the TUI renders,
- **Then** the Pending section is not shown at all (no empty placeholder).

#### AC-4 — Pending list has a capped scrollable height
- **Given** the queue has more than 5 pending prompts,
- **When** the TUI renders,
- **Then** the visible portion shows up to 5 entries; remaining entries are summarised as a single "… and N more" indicator at the bottom of the section.

#### AC-5 — Strictly serial execution (FIFO)
- **Given** an agent task completes successfully (streaming done, all tool calls finished),
- **When** the queue has one or more pending prompts and is not paused,
- **Then** Core immediately begins executing the first pending prompt. At the moment it starts, that prompt is removed from the Pending section and appears in chat history as a normal user message, followed by the agent's streaming response.

#### AC-6 — First prompt conceptually transits the queue
- **Given** no agent task is running and the queue is empty,
- **When** the user submits a prompt,
- **Then** the prompt is queued in Core and, because nothing is ahead of it and the queue is not paused, it begins executing immediately. Visually, the user experience is indistinguishable from today's direct-submit behaviour.

#### AC-7 — ESC cancels current and pauses the queue
- **Given** an agent task is running and the queue has one or more pending prompts,
- **When** the user presses ESC,
- **Then** Core cancels the currently running task (same mechanism as today's `CancelPrompt`), AND the queue enters a paused state. Pending items are retained but will not auto-fire when the cancelled task finishes.

#### AC-8 — ESC with empty queue behaves as today
- **Given** an agent task is running and the queue is empty,
- **When** the user presses ESC,
- **Then** Core cancels the running task, no paused-state is entered, and behaviour matches today.

#### AC-9 — Paused state is visually distinguishable
- **Given** the queue is in a paused state,
- **When** the TUI renders,
- **Then** the Pending section header reads `Pending (paused)`, AND the input's placeholder text reads `Press Enter to resume queue…` while the input is empty.

#### AC-10 — Resuming the paused queue
- **Given** the queue is paused and the input is empty,
- **When** the user presses Enter,
- **Then** the paused state clears and Core resumes draining the queue (next pending prompt begins executing if no task is running).
- **And given** the queue is paused and the input has text content,
- **When** the user presses Enter,
- **Then** the typed content is submitted and appended to the end of the queue. The queue remains paused. The submitted prompt does not auto-fire.
- **And given** the queue is not paused and the input is empty,
- **When** the user presses Enter,
- **Then** nothing happens (no-op, matching today's behaviour).

#### AC-11 — Failure flushes the queue silently
- **Given** an agent task ends with a failure (error, not user-cancel),
- **When** the failure event is processed,
- **Then** the Pending section is emptied (all pending prompts are dropped) and the queue is left idle (not paused). No system message is added to chat history; the dropped prompts disappear silently.

#### AC-12 — Focusing and deleting pending items
- **Given** the user has focus on the input pane and the Pending section contains items,
- **When** the user presses Up at the top of the input text (matching today's History entry behaviour),
- **Then** focus moves into the Pending section, navigating pending items from the bottom of the section (closest to input) upward. Continuing Up past the top of Pending enters the chat History, same as today. Down reverses.
- **And given** a pending item is focused,
- **When** the user presses Delete,
- **Then** that specific pending prompt is removed from the queue. No other keys perform actions on pending items in this feature's scope.

#### AC-13 — Running task cannot be deleted from Pending
- **Given** a prompt has started executing,
- **When** the user looks at the Pending section,
- **Then** that prompt is no longer listed there — it lives in chat history. The only way to stop it is ESC (which also pauses the queue per AC-7/AC-8).

#### AC-14 — Disconnect reuses the existing lock
- **Given** the TUI loses its connection to Core,
- **When** disconnect is detected,
- **Then** the input is locked (reusing today's `input_locked` mechanism) AND any pending prompts that had not been acknowledged by Core are dropped from the TUI's view.

#### AC-15 — Exit discards the queue silently
- **Given** pending prompts exist in the queue,
- **When** the user quits (Ctrl+C, typing `exit`, or closing the terminal),
- **Then** the TUI exits without a warning or confirmation. Pending items are dropped.

#### AC-16 — Slash commands and all prompt kinds queue uniformly
- **Given** any user input that the TUI would normally submit to Core (plain prompts, and any future slash-style inputs),
- **When** the user submits while a task is running,
- **Then** the input is treated identically to a plain prompt — queued and dispatched FIFO. No category of input bypasses the queue.

### Edge cases

- **EC-1 Repeat ESC while paused** — First ESC cancels current task and enters paused state. Subsequent ESC presses while already paused are idempotent no-ops.
- **EC-2 Cancel before Core ACK** — User submits a prompt and presses ESC before Core has acknowledged it. The prompt appears in chat history as a cancelled user entry, consistent with today's mid-stream cancel behaviour.
- **EC-3 Empty + paused** — When the last pending item is deleted while the queue is paused, the queue auto-transitions to "empty + not paused." The placeholder hint disappears since there is nothing to resume.
- **EC-4 Multi-line prompts** — Truncated previews in the Pending section use only the first line of the prompt; embedded newlines are stripped from the preview (the stored prompt itself is unchanged).
- **EC-5 Very long pending queue** — No queue size limit is imposed. Rendering must remain responsive; in practice queue depth is bounded by user typing speed.
- **EC-6 Focus follows deletion** — When a focused pending item is deleted, focus moves to the next item upward, or back to the input if the list becomes empty.
- **EC-7 Failure during Pending focus** — When AC-11 fires while the user has focus on a pending item, focus snaps back to Input gracefully.
- **EC-8 Disconnect mid-execution** — Per AC-14, input is locked and pending items clear. On reconnect, the TUI starts fresh; there is no attempt to replay or recover the dropped queue.
- **EC-9 Instant agent completion** — If the agent sends `AgentCompleted` very quickly (cached reply) while the user is mid-typing, typing continues uninterrupted; the next Enter submits a normal (non-queued) prompt.
- **EC-10 ESC with no task running** — When ESC is pressed and no task is running (queue may be empty, paused with items, or anything else), it is a no-op. There is nothing to cancel.

### Non-functional constraints

- **NFR-1 Single source of truth** — The authoritative queue lives in Core, not in the TUI. The TUI's Pending section is a rendered view of Core-reported state.
- **NFR-2 Single TUI assumption** — Behaviour with multiple concurrent TUIs attached to Core is not specified by this feature.
- **NFR-3 No durable persistence for pending prompts** — Pending prompts live only in memory. They are not persisted across Core restarts, TUI restarts, or connection drops.
- **NFR-4 Input must remain responsive during streaming** — Removing `input_locked` must not introduce blocking behaviour in the input handler, even while the agent is streaming large responses or running tool calls.
- **NFR-5 Cancellation latency** — ESC must produce a visible paused-state transition without waiting for the underlying task to actually stop. The pause flag is set immediately; the task cancel completes asynchronously.

### Out of scope

- Multi-TUI coordination of the queue.
- Queue persistence across disconnects/restarts.
- Editing a queued prompt in-place (pulling it back into input, revising, re-queuing).
- Reordering queued prompts (drag, move up/down).
- Per-prompt priorities or fast-lane prompts.
- Hard queue-size limit.
- Confirmation prompts on exit or disconnect.
- Separate "cancel current" and "flush queue" controls beyond ESC's combined effect.

### Completeness checklist

- [x] Happy path covered (AC-1, AC-2, AC-3, AC-5).
- [x] Error path covered (AC-11 failure flush, AC-14 disconnect).
- [x] User-initiated cancellation covered (AC-7, AC-8).
- [x] Resume path covered (AC-10).
- [x] Visibility/feedback covered (AC-3, AC-4, AC-9).
- [x] Focus/keyboard accessibility covered (AC-12).
- [x] Boundary conditions on empty queue, first prompt, repeat ESC, empty+paused, ESC with no task (AC-6, AC-8, EC-1, EC-3, EC-10).
- [x] Multi-line prompt rendering covered (EC-4).
- [x] Non-functional constraints captured (NFR section).
- [x] Out of scope explicitly enumerated.
- [x] Security/privacy — no new attack surface; prompts transit the existing `AttachTui` gRPC stream. No additional credentials, files, or network endpoints are introduced.

## Architecture

(empty)
