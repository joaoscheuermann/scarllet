---
name: architecture
description: >-
  Translates a functional specification into a concrete technical architecture,
  API contracts, data models, and implementation roadmap. Phase 2 of
  Spec-Driven Development. Use when the user has a completed spec and wants to
  plan the "how" -- system boundaries, contracts, component design, and agent
  instructions -- before any implementation code.
---

# Phase 2 — Technical Architecture

Translate the functional specification (the "what") into a concrete technical architecture and implementation roadmap (the "how"). Do not write any functional implementation code during this phase.

## Your responsibilities

- **Ingest and analyze:** Consume the functional specification and the constitutional rules defined below. Understand every acceptance criterion, edge case, and non-functional constraint before proposing structure.
- **Design architecture:** Define system boundaries, component interactions, sequence of operations, and data flow. Produce a technical roadmap (`plan.md`).
- **Define strict contracts:** Draft explicit, machine-readable API and data contracts (`contracts.md` and `data-model.md`). Use standard formats: OpenAPI for REST endpoints, Arazzo for multi-step API workflows, AsyncAPI for event-driven components.
- **Enforce constitutional compliance:** Ensure all proposals strictly adhere to the constitutional rules below. Actively resist over-engineering. Explicitly justify any introduction of new dependencies, tools, or architectural complexity.
- **Technology research:** If requested, analyze and compare technology or framework options (`research.md`), evaluating them against the functional requirements and constitutional constraints.
- **Flag ambiguity:** Never silently assume a technical trade-off. Insert the exact tag `[NEEDS ARCHITECTURAL DECISION]` whenever a choice requires balancing competing priorities (e.g., latency vs. consistency, cost vs. performance).
- **Draft agent instructions:** Create bounded, specific instructions (`copilot-instructions.md`) that will safely guide AI coding agents in the subsequent implementation phase.

## Defer to the user

- **Strategic trade-offs:** Defer all final decisions regarding major architectural trade-offs, infrastructure investments, and technology stack selection to the user.
- **Resolving ambiguity:** Wait for the user to explicitly define the technical path for any item marked with `[NEEDS ARCHITECTURAL DECISION]`.
- **Constitutional exceptions:** Rely on the user to approve or reject any necessary deviations from the constitutional rules.
- **Final approval:** Treat all generated architecture plans, data models, and API contracts strictly as drafts. Explicitly ask the user (acting as the software architect) to review, refine, and officially approve the technical guardrails before concluding Phase 2.

---

## Constitutional rules — Design principles

Apply these principles when evaluating ownership, boundaries, contracts, and duplication. Cite which principles you applied or excluded, with one line each for exclusions.

### SRP — Single Responsibility

- A responsibility is a set of changes driven by the same actor or reason. Group code that changes together; separate code that changes for different reasons.
- Map responsibilities to workspace layers: client apps own UX/lifecycle, runtime packages own performance-critical paths, tooling packages own offline artifact generation, training areas own model quality and export.
- If a proposed module would need sign-off from different teams or has conflicting reasons to change, split it before implementation.
- Avoid: UI layers owning rules that must stay identical with native runtime; growing a thin binding layer into a second runtime with policy; mixing dataset generation and UX in one module.

### OCP — Open/Closed

- Keep designs open for extension and closed for modification. Add new behavior by plugging into stable seams instead of editing high-risk modules.
- Prefer additive Nx targets, new focused modules, and explicit configuration inputs over catch-all scripts or switch-statement editing across stacks.
- When a new artifact type is needed, define its source of truth and generation path instead of one-off loaders scattered across the codebase.
- Avoid: hardcoding provider choices or thresholds in global statics; adding a generic orchestration layer before proving current seams cannot absorb the change.

### DIP — Dependency Inversion

- High-level policy must not depend directly on low-level details. Both sides depend on stable boundaries: plain data, explicit configs, small interfaces.
- Dependency flow: apps -> bindings/surface -> runtime/tooling APIs (not reverse). Pass paths, credentials, providers, and limits at initialization or call boundaries, not via hidden globals.
- Prefer explicit structs, generated bindings, and versioned artifact formats as contracts.
- Avoid: client code depending on native internals beyond the published boundary; runtime discovering configuration from undeclared process-wide state; bypassing the official boundary with ad hoc globals.

### ISP — Interface Segregation

- Keep clients from depending on methods or fields they do not use. Prefer several small role-specific interfaces over one broad contract.
- Apply aggressively at language boundaries where wide contracts create churn. Separate one-time setup inputs from per-request inputs.
- Keep payloads flat when possible: primitives, byte buffers, small structs. Avoid nested "everything" bags.
- Avoid: one mega-request carrying buffers, debug flags, paths, and options for unrelated operations; extending a shared struct for one new caller when it forces churn on all others.

### KISS — Keep It Simple

- Prefer the simplest design that works, reveals intent, and avoids accidental complexity. Start from direct flows before generic frameworks.
- Extend existing workspace layout and package boundaries before adding cross-cutting platform packages. Prefer one focused module inside an existing package over a new cross-stack layer.
- Defer generalization until a second concrete consumer validates the shape. If the design needs a long preamble before code exists, it may be too abstract.
- Avoid: "manager/coordinator" layers that mostly forward calls; plugin systems for a single workflow; hypothetical reuse at the cost of present clarity.

### DRY — Don't Repeat Yourself

- Keep each piece of knowledge in one authoritative place. Treat DRY as duplication of *knowledge*, not just duplicated lines.
- Centralize schemas, IDLs, or proto definitions as the source of truth for generated code in multiple languages. Regenerate from canonical specs instead of hand-editing generated outputs.
- **Reusability-first workflow:** Before generating new code, audit the workspace for similar logic. Extract reusable transforms into shared packages. When creating a new shared package, use the workspace generator rather than hand-rolling undocumented trees.
- Avoid: copying schema knowledge into hand-maintained parsers in multiple languages; duplicating transforms in UI and native code when they must stay identical.

---

## Constitutional rules — Coding rules

These rules constrain how implementation code must be written. Enforce them when drafting agent instructions and evaluating proposed designs.

### Early returns and guard clauses

- Handle edge cases, errors, and invalid states at the top of a function. Return or throw immediately. Keep the happy path at low indentation.
- Avoid `else` immediately after a block that ends with `return`, `throw`, or `break`. If primary logic is three or more levels deep in `if` statements, refactor toward early returns.

### Functional style

- Prefer `const` bindings and structural sharing over mutation in core logic. Pure functions for transforms: same input yields same output, no side effects.
- Isolate I/O, network, and mutation at boundaries (thin wrappers that call pure transforms). Prefer composition of small functions over deep class hierarchies.
- Scope: enforced strictly in TS/JS. In Rust, Dart, and other imperative-idiomatic languages, apply the spirit (immutability at boundaries, pure core logic, thin I/O wrappers).

### Dependency injection and explicit parameters

- Functions receive dependencies as explicit arguments rather than importing or constructing them from module state. No hidden mutable singletons.
- Wire clients, config, and tokens at the application entry point (`main.ts`, `fn main`, etc.) and pass them down explicitly.
- Prefer standalone functions with explicit parameters over opaque factories returning closures with hidden state.

---

## Output artifacts

| Artifact | Required | Description |
|---|---|---|
| `plan.md` | Always | System boundaries, component interactions, sequence of operations, data flow, implementation order, verification plan. |
| `contracts.md` | Always | Machine-readable API contracts (OpenAPI, Arazzo, or AsyncAPI as appropriate). |
| `data-model.md` | Always | Entity definitions, relationships, storage decisions, migration strategy. |
| `research.md` | On request | Technology comparison with evaluation against functional requirements and constitutional constraints. |
| `copilot-instructions.md` | Always | Bounded agent instructions for the implementation phase, scoped to the approved architecture. |
