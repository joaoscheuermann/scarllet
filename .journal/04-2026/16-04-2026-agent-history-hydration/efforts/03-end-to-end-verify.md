---
status: done
order: 3
created: 2026-04-16 12:35
title: "End-to-end verification and edge cases"
---

## Description

Verify the full hydration pipeline end-to-end: TUI sends history on connect → Core stores it → agent registers → Core sends AgentHistorySync → agent seeds context → user sends prompt → agent responds with awareness of prior conversation. Fix any edge cases found during testing.

## Objective

A user can close the TUI, reopen it, send a follow-up message, and the agent responds with full awareness of the previous conversation — without the user repeating context.

## Implementation Details

1. **Manual end-to-end test sequence**:
   - Start Core and TUI.
   - Send a message like "My name is Alice and I'm working on project Scarllet."
   - Wait for agent response.
   - Close the TUI (CTRL+C).
   - Reopen the TUI.
   - Confirm previous messages are displayed (session persistence from earlier work).
   - Send "What is my name and what project am I working on?"
   - Verify the agent responds correctly referencing "Alice" and "Scarllet" — proving it has the conversation history.

2. **Edge cases to verify**:
   - Empty history (fresh session, no prior messages): agent starts with no history, works normally.
   - CTRL+N (new session): history clears, next agent spawn gets empty history.
   - Very first TUI connect (no session.json): TUI sends empty HistorySync, agent starts fresh.

3. **Fix any issues found**: If the agent doesn't receive history correctly, or if the role mapping is wrong (user/assistant), fix the relevant code in Efforts 1 or 2.

## Verification Criteria

- Full stack runs: Core + TUI + agent.
- The "name recall" test above succeeds: agent responds with prior context after TUI restart.
- Empty-history scenario: fresh session works without errors.
- CTRL+N scenario: new session clears history, agent starts fresh on next prompt.
- `cargo test` passes for all crates.

## Done

- End-to-end conversation continuity works across TUI restarts.
- Agent responds with awareness of prior conversation history.
- Edge cases (empty history, new session) handled gracefully.
