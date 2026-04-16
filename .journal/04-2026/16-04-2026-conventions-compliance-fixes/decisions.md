# Decision Log: conventions-compliance-fixes

### 2026-04-16 09:50 - Architect

**Context**: OCP Fix 8 — string-based status dispatch in report_progress uses match on string literals from the agent protocol.
**Decision**: Defer. Add doc comment explaining the protocol contract instead.
**Rationale**: Fixing properly requires a proto schema change (adding a status enum), which is out of scope for this refactor. The string contract is documented in the proto and changing it risks breaking agent compatibility.
**Alternatives considered**: Local enum with `From<&str>` — adds indirection without proto-level safety, net negative.

### 2026-04-16 09:50 - Architect

**Context**: KISS Fix 9 — route_prompt uses a polling loop (20 iterations x 500ms) to wait for an agent to register after spawning.
**Decision**: Implement Alternative A — tokio::sync::Notify. Agent registration handler triggers notify, route_prompt awaits with timeout.
**Rationale**: Deterministic, no wasted cycles, simpler control flow. Timeout preserves the same 10s window as the old polling.
**Alternatives considered**: Alternative B — buffered task queue where agents pull tasks after registration. Better long-term but larger scope; deferred with a code comment for future reference.

### 2026-04-16 09:50 - Architect

**Context**: Doc comments scope — whether to document all 31 .rs files or only those being refactored.
**Decision**: Document all 31 files. Every fn, pub fn, async fn, pub async fn, trait method, and impl method gets a `///` doc comment.
**Rationale**: User explicitly requested full coverage. Done as the last effort after all structural refactors so comments describe the final state.
**Alternatives considered**: Partial coverage (only refactored files) — rejected by user.

### 2026-04-16 10:02 - Decomposer

**Context**: Breaking approved architecture into incremental deliverables with runnable outcomes.
**Decision**: 7 efforts in dependency order: (1) DRY extractions, (2) core SRP split, (3) TUI SRP split + scroll helper, (4) ISP AgentContext, (5) DIP explicit deps, (6) KISS Notify registration, (7) doc comments on all files. Each effort yields a buildable/testable/runnable state.
**Rationale**: DRY extractions first because the SRP splits depend on `events.rs` existing. SRP splits before DIP/KISS because those efforts reference the new module layout. Doc comments last because they describe the final state after all structural changes. Each effort is a vertical slice — the codebase compiles and runs after every effort.
**Alternatives considered**: Horizontal slicing (all DRY fixes, then all SRP, etc.) — rejected because it delays runnable verification; interleaving principle-based fixes with the modules they affect is more incremental.
