---
status: done
created: 2026-04-10 19:31
slug: multi-process-ai-agent-harness
---

## Prompt

### System Overview
**What:** A highly modular, multi-process AI coding agent harness consisting of:
1.  **TUI/CLI:** The persistent, isolated user interface.
2.  **Core Orchestrator:** The central nervous system managing process lifecycles, global state, and configuration.
3.  **Commands & Tools:** Standalone, dynamically discovered binaries for extensible logic.
4.  **Agents:** Specialized workers orchestrated by the Core.
5.  **LLM Abstraction Library:** A normalized internal library used to standardize external AI provider communications.

**Why:** To ensure maximum fault isolation, allow unbounded extensibility via drop-in executables, centralize system-wide visibility, and decouple agent logic from specific LLM vendor API formats.

**System Constraints & Paradigms:**
*   *Implementation:* Requires a compiled, memory-safe, highly performant language capable of native multi-process orchestration and low-latency RPC (Rust).
*   *Security & Resources (The "YOLO" Doctrine):* The system lacks sandboxing or artificial resource limits. Unintended background exhaustion is an accepted risk.

---

### User Stories & Acceptance Criteria

#### 1. Interface Initialization, Interaction, & Visibility
**As a user,** I want an intelligent interface that survives closures, strictly auto-completes commands, and clearly visualizes exactly what agents and tools are doing.

*   **Scenario: Reconnecting and Context Management**
    *   **Given** a Core process is already running
    *   **When** a new TUI instance is launched from a new working directory
    *   **Then** the TUI attaches to the existing Core, running new tasks in the new directory while existing tasks finish in their original directories.
*   **Scenario: Strict Command Auto-completion**
    *   **Given** the TUI is active
    *   **When** the user types `/`
    *   **Then** the TUI displays a dynamically filtered list of commands without attempting natural language auto-completion.
*   **Scenario: Visualizing Concurrent Agent and Tool Execution**
    *   **Given** an Agent is actively running
    *   **When** the Agent invokes an external Tool
    *   **Then** the Core detects this state change
    *   **And** the TUI updates its hierarchical tree-view to explicitly show the user that "Agent X" is executing "Tool Y", alongside buffered thinking/output.

#### 2. Module Discovery & Hot-Reloading
**As the Core orchestrator,** I want to passively watch for new binaries and silently ignore invalid files, **so that** the system can hot-load modules without crashing.

*   **Scenario: Dynamic Directory Watching**
    *   **Given** the Core is running
    *   **When** a new executable is added to the `commands` or `tools` directory
    *   **Then** the Core executes it with a `--manifest` flag to register its capabilities.
*   **Scenario: Silent Manifest Failure**
    *   **Given** an invalid file (e.g., text file) is placed in a hot-reload directory
    *   **When** the Core attempts to execute it with `--manifest`
    *   **Then** the Core silently ignores the file without crashing or displaying an error.

#### 3. Granular State Management & Delegation
**As a Command or parent Agent,** I want to request specific state data on-demand and delegate work, **so that** I don't overwhelm the RPC layer.

*   **Scenario: Granular State Requests via RPC API**
    *   **Given** a Command initializes
    *   **When** it needs context
    *   **Then** it makes specific RPC calls to the Core (e.g., `GetChatHistory()`)
    *   **And** the Core returns only the requested data.
*   **Scenario: Forceful Lifecycle Termination (Cancellation)**
    *   **Given** a running task
    *   **When** the user issues a cancellation request via the TUI
    *   **Then** the Core immediately kills the relevant Command and all its child processes.

#### 4. Isolated Tool Utilization & Point-in-Time Availability
**As an Agent,** I want to invoke stateless external tools based on what was available when I started, **so that** my execution context remains predictable.

*   **Scenario: Point-in-Time Tool Availability**
    *   **Given** an Agent begins executing its task
    *   **When** a new Tool is dynamically registered by the Core
    *   **Then** the new Tool is only available to Agents spawned *after* the registration.
*   **Scenario: Stateless Tool Invocation**
    *   **Given** an Agent requires a tool
    *   **When** the Agent invokes the tool via RPC
    *   **Then** the Tool process executes statelessly based strictly on passed arguments, terminating and returning an error if it exceeds its predefined timeout.

#### 5. Normalized LLM Communication & Credential Management
**As a system,** I want to manage API credentials globally and normalize external AI calls, **so that** the user only has to configure keys once and developers can write vendor-agnostic agent logic.

*   **Scenario: Initializing Global Credentials**
    *   **Given** the Core process is starting up
    *   **When** it initializes its global state
    *   **Then** it reads a JSON configuration file located in the OS-standard user configuration directory (e.g., `~/.config/...` or `%APPDATA%\...`)
    *   **And** loads the API provider credentials into its internal state.
*   **Scenario: Standardizing External AI Calls**
    *   **Given** an Agent uses the internal LLM API Library
    *   **When** the Agent requests a response
    *   **Then** the library retrieves the necessary credentials from the Core's state via RPC
    *   **And** translates the vendor-agnostic request into the external provider's format, normalizing the response back into the system's standard format.
*   **Scenario: Credential Setup Command**
    *   **Given** the user invokes a built-in setup command
    *   **When** the command executes
    *   **Then** it prompts the user for an AI provider name and API key via the TUI
    *   **And** updates the Core's internal state
    *   **And** writes the updated credentials back to the OS-standard JSON configuration file.

---

### Resolved Edge Cases
*   **LLM Provider Outages / Rate Limits:** The Normalized LLM API Library will bubble HTTP errors (429, 500) back to the Agent. The Agent's logic is responsible for deciding whether to retry or fail gracefully.
*   **Non-Standard Tool Responses:** If an LLM hallucinates a tool format, the parsing library returns an error string to the Agent, allowing the Agent to self-correct by feeding the error back to the LLM.

---

### Quality Assurance Checklist (Final)
*   [x] Core process lifecycle, persistence, and TUI decoupling defined.
*   [x] Command AND Tool registration (hot-reloading, failure handling) defined.
*   [x] Point-in-time Tool availability & Stateless Tool execution defined.
*   [x] Granular RPC State API defined.
*   [x] LLM API Normalization Library behavior defined.
*   [x] Global API Key / Credential Management (JSON config + Setup Command) defined.
*   [x] Agent/Sub-agent centralized management and Tree-view buffering defined.
*   [x] Cancellation/Termination and Context Divergence defined.
*   [x] Security and Resource paradigms defined (Unbounded / YOLO).

## Research

(empty)

## Architecture

### Overview

A multi-process AI coding agent harness implemented entirely in Rust, organized as 5 Nx-managed crates under `packages/rust/`. All inter-process communication uses gRPC (tonic/prost). The system consists of a persistent Core daemon, a decoupled TUI client, hot-reloadable module binaries (Commands, Tools, Agents), and a shared LLM normalization library.

### Crate Map

| Crate | Type | Nx Path | Purpose |
|-------|------|---------|---------|
| `scarllet-proto` | library | `packages/rust/scarllet-proto` | `.proto` files + tonic/prost codegen. Single source of truth for all gRPC service definitions and message types. |
| `scarllet-sdk` | library | `packages/rust/scarllet-sdk` | Shared types, manifest schema, gRPC client helpers, module traits. Everything a Command/Tool/Agent binary needs to interact with Core. |
| `scarllet-core` | binary | `packages/rust/scarllet-core` | Core orchestrator daemon. gRPC server, process lifecycle, directory watcher, state management, credential store. |
| `scarllet-tui` | binary | `packages/rust/scarllet-tui` | TUI client. Connects to Core via gRPC, renders tree-view, handles command auto-completion. |
| `scarllet-llm` | library | `packages/rust/scarllet-llm` | LLM normalization library. Vendor-agnostic request/response, provider adapters, credential retrieval from Core via gRPC. |

### Dependency Graph

```
scarllet-proto          (no workspace deps)
       │
  scarllet-sdk          (depends on: proto)
    ┌──┴──┐
scarllet-core  scarllet-llm   (core: sdk, proto │ llm: sdk, proto)
    │               │
    │          scarllet-tui    (depends on: sdk, proto)
    │
  [External module binaries depend on: sdk, proto, optionally llm]
```

No dependency cycles. Proto is the leaf. Core never depends on LLM (Core doesn't make LLM calls — agents do).

### Communication Architecture

#### 1. TUI ↔ Core: Bidirectional gRPC streaming

- `AttachTui(stream TuiMessage) returns (stream CoreEvent)` — long-lived bidirectional stream.
- TUI sends user input, cancellation requests, command queries.
- Core pushes real-time events: agent state changes, tool execution start/stop, output buffers, tree-view updates.
- Multiple TUI instances can attach concurrently; each gets its own stream.

#### 2. Commands & Agents → Core: gRPC client (unary RPCs + selective streaming)

- Core spawns module binary, passes gRPC address via `SCARLLET_CORE_ADDR` environment variable.
- Module connects back as a gRPC client and makes request/response calls.
- Granular state queries: `GetChatHistory`, `GetToolRegistry`, `GetAgentStatus`, `GetCredentials`.
- Agent-specific: `InvokeTool` (Core spawns the tool, returns result), `ReportProgress` (state updates pushed to TUI).
- Core→Agent cancellation delivered via a server-streaming RPC that the agent monitors.

#### 3. Core → Tools: stdin/stdout JSON (no gRPC)

- Core spawns tool binary with CLI arguments.
- Input: JSON payload piped via stdin.
- Output: JSON result captured from stdout. Stderr captured for diagnostics.
- Timeout: Hard kill (SIGKILL / TerminateProcess) when `timeout_ms` from manifest expires. Reported as `ToolRunFailure` to the requesting Agent.
- Tools are language-agnostic — any executable that reads stdin JSON and writes stdout JSON.

### Core Daemon Discovery (TUI Reconnection)

- Core binds to a random available port on `127.0.0.1`.
- Writes lockfile to OS-standard config directory (`dirs::config_dir()` → `scarllet/core.lock`):
  ```json
  { "pid": 12345, "address": "127.0.0.1:49832", "started_at": "..." }
  ```
- TUI reads lockfile to find Core's address.
- Stale PID detection: if lockfile exists but PID is dead, TUI cleans up lockfile and starts a new Core.
- New TUI from a different working directory attaches to the existing Core; new tasks run in the new directory while existing tasks finish in their original directories.

### Module Discovery & Hot-Reload

All three module types share one discovery protocol:

1. Core watches three directories via `notify` crate:
   - `commands/`
   - `tools/`
   - `agents/`
   (Paths configured in Core config; defaults to subdirectories of the OS-standard config directory.)

2. New/modified file detected → Core spawns: `./binary --manifest`
   - Timeout: 5 seconds → hard kill if exceeded.
   - Success: Parse JSON manifest from stdout → register module.
   - Failure (non-zero exit, invalid JSON, timeout, non-executable): Silent ignore, log at debug level.

3. File removed → Core deregisters module.

4. Manifest JSON schema (defined in `scarllet-sdk`):
   ```json
   {
     "name": "...",
     "kind": "command | tool | agent",
     "version": "...",
     "description": "...",
     "input_schema": { },
     "timeout_ms": 30000,
     "capabilities": ["..."],
     "aliases": ["..."]
   }
   ```
   - `input_schema`: Tool-specific. JSON Schema describing expected stdin input.
   - `timeout_ms`: Tool-specific. Hard kill deadline.
   - `capabilities`: Agent-specific. What this agent kind can do.
   - `aliases`: Command-specific. Slash-command aliases for TUI auto-completion.

### Point-in-Time Tool Snapshots

When Core spawns an Agent, it captures the current tool registry as a frozen snapshot (version ID + tool list). The Agent receives this snapshot ID. All `InvokeTool` calls from the Agent are validated against this snapshot — tools registered after the Agent spawned are invisible to it.

### Process Lifecycle & Cancellation

- User cancels via TUI → `CancelTask` RPC → Core.
- Core sends SIGTERM (or TerminateProcess on Windows) to target process.
- 2-second grace period.
- SIGKILL (hard kill) if still alive.
- Kill all child processes (process group on Unix, job object on Windows).
- Update state, notify all attached TUIs.

Tool timeouts: No grace period. Hard kill immediately when `timeout_ms` expires. Report `ToolRunFailure` to requesting Agent.

### gRPC Service Definition (scarllet-proto)

```protobuf
service Orchestrator {
  // TUI
  rpc AttachTui(stream TuiMessage) returns (stream CoreEvent);
  rpc ListCommands(ListCommandsRequest) returns (ListCommandsResponse);

  // Granular state queries
  rpc GetChatHistory(ChatHistoryQuery) returns (ChatHistoryResponse);
  rpc GetToolRegistry(ToolRegistryQuery) returns (ToolRegistryResponse);
  rpc GetAgentStatus(AgentStatusQuery) returns (AgentStatusResponse);
  rpc GetCredentials(CredentialQuery) returns (CredentialResponse);

  // Task lifecycle
  rpc SubmitTask(TaskSubmission) returns (TaskReceipt);
  rpc CancelTask(CancelRequest) returns (CancelResponse);

  // Agent operations
  rpc InvokeTool(ToolInvocation) returns (ToolResult);
  rpc ReportProgress(ProgressReport) returns (Ack);

  // Credential management
  rpc SetCredential(SetCredentialRequest) returns (SetCredentialResponse);
}
```

### Credential Management

- **Storage:** JSON file at `dirs::config_dir()` → `scarllet/config.json`.
- **Load:** Core reads on startup, holds in memory.
- **Update:** `SetCredential` RPC writes to memory + flushes to disk.
- **Access:** Agents/LLM library call `GetCredentials` to retrieve provider keys.
- **Setup:** A built-in command binary that prompts for provider + key via TUI, calls `SetCredential`.

### Key External Dependencies

| Crate | Purpose |
|-------|---------|
| `tonic` + `prost` | gRPC server/client + protobuf codegen |
| `tonic-build` + `prost-build` | Build-time proto codegen |
| `tokio` | Async runtime |
| `ratatui` + `crossterm` | TUI rendering |
| `notify` | Filesystem watching for hot-reload |
| `reqwest` | HTTP client for LLM provider APIs |
| `serde` + `serde_json` | JSON serialization |
| `tracing` + `tracing-subscriber` | Structured logging |
| `clap` | CLI argument parsing |
| `dirs` | OS-standard directory resolution |

### Nx Integration

- Each crate is an Nx project with `@monodon/rust` executors: `build`, `test`, `lint`, `clean`.
- Root `Cargo.toml` workspace members: all 5 crate paths.
- Build artifacts under `dist/target/<crate-name>` (already configured in `.cargo/config.toml`).
- CI extends existing `.github/workflows/ci.yml` with `npx nx run-many -t build test lint` covering Rust crates.

### Design Principles Applied

- **SRP**: Each crate has one owner and one reason to change (proto=schema, sdk=contracts, core=orchestration, tui=presentation, llm=vendor abstraction).
- **OCP**: New capabilities via drop-in binaries in watched directories — no Core source edits. New LLM providers via new adapter modules.
- **DIP**: All modules depend on proto/sdk contracts, never on Core internals. LLM library depends on a provider trait, not vendor SDKs directly.
- **ISP**: Granular RPC methods (not one mega-request). Manifest fields are kind-specific (tools don't carry agent fields).
- **KISS**: 5 crates not 15. stdin/stdout for tools. Lockfile for discovery. No service mesh.
- **DRY**: Proto is single schema source. Manifest protocol shared across module types. SDK provides common types once.
