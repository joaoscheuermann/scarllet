# Scarllet

A personal **Coding Agent AI Harness** for experimenting with harnesses, tooling, and customized agents.

> **Not for production use.** This is an experimental prototyping playground. APIs, structure, and conventions may change without notice.

Scarllet is built to enable fast iteration on ideas around AI agent orchestration, tool integration, and terminal-based interfaces — without the overhead of production-grade concerns. If you want to try a new agent loop, swap an LLM provider, or wire up a custom tool, this architecture gets out of your way.

## Architecture

The system is composed of **nine Rust crates** inside an **Nx monorepo** (`packages/rust/`), connected via **gRPC** (`scarllet-proto`):

```mermaid
flowchart TB
  subgraph workspace ["Nx Monorepo — packages/rust/ (Rust via @monodon/rust)"]
    proto["scarllet-proto\n(gRPC types, orchestrator.proto)"]
    sdk["scarllet-sdk\n(config, manifest, lockfile)"]
    core["scarllet-core\n(Orchestrator server)"]
    tui["scarllet-tui\n(Terminal UI client)"]
    llm["scarllet-llm\n(OpenAI + Gemini LLM providers)"]
    agent["agents/default\n(Reference agent binary)"]
    tools["tools/\n(6 tool binaries)"]
    tools --> core

    proto --> sdk
    sdk --> core
    sdk --> tui
    proto --> core
    proto --> tui
    proto --> agent
    llm --> agent
  end

  user(["User"]) --> tui
  tui -- "gRPC\nAttachSession diff stream" --> core
  core -- "spawn / AgentStream" --> agent
  agent -- "HTTP / SSE" --> llmAPI(["LLM API\n(OpenAI / Gemini)"])
```

| Crate            | Path                           | Role                                                                                                                                                                                              |
| ---------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `scarllet-proto` | `packages/rust/scarllet-proto` | Protobuf definitions and `tonic` codegen for the `Orchestrator` gRPC service                                                                                                                      |
| `scarllet-sdk`   | `packages/rust/scarllet-sdk`   | Shared types: config loading (`config.json`), module manifests, lockfile for core address discovery                                                                                               |
| `scarllet-core`  | `packages/rust/scarllet-core`  | Orchestrator binary — gRPC server, multi-session registry, per-session node graph / queue / agents, filesystem watcher for hot-reloading plugins                                                  |
| `scarllet-tui`   | `packages/rust/scarllet-tui`   | Terminal UI built with **ratatui + crossterm** — chat interface that hydrates from `AttachSession` and applies diffs to a local node-graph mirror                                                 |
| `scarllet-llm`   | `packages/rust/scarllet-llm`   | Standalone LLM client library exposing the `LlmProvider` async trait; ships with `OpenAiProvider` (OpenAI-compatible HTTP + SSE streaming) and `GeminiProvider` (Google Gemini via `gemini-rust`) |
| `agents/default` | `packages/rust/agents/default` | Reference agent — uses `scarllet_sdk::agent::AgentSession` to register, fetch provider / history / tools, and emit node mutations back                                                            |
| `tools/*`        | `packages/rust/tools/*`        | Six plugin tools: `edit`, `find`, `grep`, `terminal`, `tree`, `write` — each is a standalone binary invoked by core via stdin/stdout JSON                                                         |

### gRPC Service (`Orchestrator`)

The `Orchestrator` service in `orchestrator.proto` defines these RPCs:

| RPC                        | Direction            | Purpose                                                          |
| -------------------------- | -------------------- | ---------------------------------------------------------------- |
| `CreateSession`            | unary                | Allocate a new session and return its id                         |
| `ListSessions`             | unary                | Enumerate active sessions                                        |
| `DestroySession`           | unary                | Cancel agents and drop a session                                 |
| `GetSessionState`          | unary                | Return a full snapshot of a session                              |
| `AttachSession`            | server stream        | First message hydrates; subsequent messages are `SessionDiff`s   |
| `SendPrompt`               | unary                | Append a `User` node, enqueue a prompt, trigger dispatch         |
| `StopSession`              | unary                | Cascade cancel main + sub-agents and clear the queue             |
| `AgentStream`              | bidirectional stream | Agent register / node create-and-update / turn-finished / failure |
| `GetActiveProvider`        | unary                | Per-turn provider snapshot (session-scoped)                      |
| `GetToolRegistry`          | unary                | Per-session tool list (external tools + `spawn_sub_agent`)       |
| `GetConversationHistory`   | unary                | Chronological history derived from the session node graph        |
| `InvokeTool`               | unary                | Execute a tool by name within a session; `spawn_sub_agent` is a core-internal branch |

All conversation state lives in a per-session flat graph of typed `Node`s
(`User`, `Agent`, `Thought`, `Tool`, `Result`, `Debug`, `TokenUsage`, `Error`).
Clients hydrate once via `AttachSession` and receive incremental `SessionDiff`
messages (`NodeCreated`, `NodeUpdated`, `QueueChanged`, `AgentRegistered`,
`AgentUnregistered`, `StatusChanged`, `SessionDestroyed`) thereafter.

## Runtime Layout

When you run `npx nx run scarllet:release`, Cargo builds all crates in release mode and the release script (`scripts/release.ps1`) assembles them into a flat `release/` folder:

```
release/
  core.exe            # scarllet-core orchestrator
  tui.exe             # scarllet-tui terminal client
  agents/
    default.exe       # default agent
  tools/
    edit.exe          # file editor (patch-based)
    find.exe          # glob search
    grep.exe          # regex search
    terminal.exe      # shell executor
    tree.exe          # directory tree
    write.exe         # file writer
```

> **Note:** `commands/` directories are watched at runtime for plugin binaries but no command plugins are currently shipped.

### Configuration (`config.json`)

Core loads its configuration from `<OS config dir>/scarllet/config.json` (e.g. `%APPDATA%/scarllet/config.json` on Windows). The file defines LLM providers — each with an API URL, API key, model list, and optional settings like reasoning effort or extra body parameters. One provider is marked as `active_provider`.

If the file does not exist, core creates it with empty defaults. Core watches `config.json` for changes and hot-reloads it. Per AC-9.2 the reload only affects **new** sessions — existing sessions keep the provider snapshot they captured at create-time so long-running conversations are not disrupted.

### Lockfile (`core.lock`)

When core starts, it binds to a random local port and writes a `core.lock` file next to `config.json` (i.e. `<OS config dir>/scarllet/core.lock`). This file contains the process PID, bound address, and start timestamp. The TUI reads `core.lock` to discover which address to connect to. When core shuts down, it removes the lockfile.

### Module Discovery

Core watches three sets of directories for plugin binaries:

1. **Local** — sibling directories next to the `core.exe` binary (i.e. inside `release/`): `commands/`, `tools/`, `agents/`
2. **User** — under `<OS config dir>/scarllet/` (e.g. `%APPDATA%/scarllet/agents/`)

User directories are scanned after local ones, so user-placed modules can override shipped defaults. For each file found (or created/modified at runtime), core runs `<binary> --manifest` and parses the JSON output as a `ModuleManifest` with fields like `name`, `kind` (`command` / `tool` / `agent`), `version`, `description`, and optional `input_schema`, `timeout_ms`, `capabilities`, and `aliases`. If the probe succeeds, the module is registered; if a file is deleted, it is deregistered.

## Key Architectural Patterns

- **gRPC boundary** — All inter-process communication goes through `orchestrator.proto`. Core, TUI, and agents are separate binaries that speak a single well-defined protocol.
- **Plugin model** — Tools, commands, and agents are standalone executables discovered via `--manifest` JSON output. Core watches filesystem directories and hot-reloads new modules as they appear.
- **Process isolation** — Agents and tools run as child processes. Core communicates with tools via stdin/stdout JSON and with agents via bidirectional gRPC streams (`AgentStream`).
- **LLM abstraction** — The `LlmProvider` async trait in `scarllet-llm` decouples agent logic from any specific provider. Currently ships with `OpenAiProvider` (OpenAI-compatible HTTP + SSE) and `GeminiProvider` (Google Gemini), both selectable at runtime via `config.json`.
- **Broadcast to UIs** — Each session multiplexes `SessionDiff`s (node creates, node updates, queue changes, agent register/unregister, status changes) to every TUI attached via `AttachSession`, so multiple terminals can observe the same session in sync.
- **Node graph in core** — `scarllet-core` maintains a per-session append-only flat node graph (`session/nodes.rs`) and streams diffs to connected TUIs. `GetSessionState` returns a full snapshot for on-demand hydration.

## How Agents Work

1. Core watches `agents/` directories, probes each binary with `--manifest`, and registers it in the global module registry.
2. The user types a prompt in the TUI. The TUI calls `SendPrompt(session_id, text, cwd)`; core appends a `User` node, enqueues the prompt, and broadcasts `NodeCreated` + `QueueChanged` diffs to every attached TUI.
3. If no main agent is running for the session, core spawns the configured `default_agent` module binary, creates the turn's `Agent` node, and hands out the queued prompt as an `AgentTask`.
4. The agent process connects back via `AgentStream`, sends `Register { parent_id = session_id, … }`, then calls `GetActiveProvider(session_id)`, `GetToolRegistry(session_id)`, and `GetConversationHistory(session_id)` to set up the per-turn LLM request.
5. The agent uses `scarllet-llm` to stream tokens. For each streamed block it emits `CreateNode(Thought)` + `UpdateNode(thought_content: delta)`; for tool calls it emits `CreateNode(Tool pending)`, calls `InvokeTool`, then `UpdateNode(tool_status / result_json)`. On completion it emits `CreateNode(Result)` + `TurnFinished`.
6. Each accepted mutation turns into the matching `SessionDiff` payload (`NodeCreated` / `NodeUpdated`) broadcast to every attached TUI.

## Fast Prototyping

The architecture is designed to minimize friction when experimenting:

- **New agent** — Write a Rust binary that prints a `--manifest` JSON and connects to the core `AgentStream`. Drop it in the `agents/` directory and core picks it up automatically.
- **New tool** — Even simpler: a binary that reads JSON from stdin and writes JSON to stdout. Place it in the `tools/` directory.
- **Hot-reload** — The filesystem watcher detects new or changed binaries and re-probes manifests without restarting core.
- **Independent evolution** — TUI connects to core over gRPC, so UI changes never require touching orchestration logic (and vice versa).
- **New LLM provider** — Implement the `LlmProvider` async trait in `scarllet-llm` and add a provider variant; swap providers by editing `config.json`.

## Tech Stack

| Layer               | Technology                                                                                                           |
| ------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Language            | **Rust** (edition 2021) inside an **Nx** monorepo managed by [`@monodon/rust`](https://github.com/cammisuli/monodon) |
| RPC                 | **tonic / prost** — gRPC server and client codegen                                                                   |
| Async runtime       | **tokio** (full feature set)                                                                                         |
| Terminal UI         | **ratatui + crossterm** — markdown rendering, scrollable chat history, status bar with git info                      |
| HTTP client         | **reqwest** — for LLM API calls with SSE streaming                                                                   |
| Filesystem watching | **notify** — cross-platform plugin discovery                                                                         |
| LLM providers       | `gemini-rust` for Gemini; `reqwest` for OpenAI-compatible APIs                                                       |

## Getting Started

```sh
npm install
npx nx run scarllet:release
```

This builds all crates and assembles the `release/` folder. Then:

1. Edit `<OS config dir>/scarllet/config.json` to set up at least one LLM provider (API URL, key, model, and mark it as `active_provider`). Supported providers: **OpenAI-compatible** and **Google Gemini**.
2. Run `release/core.exe` — it starts the orchestrator, writes `core.lock`, and begins watching for modules.
3. Run `release/tui.exe` — it reads `core.lock`, connects to core, and opens the chat interface.

## Disclaimer

This project is a **personal experiment**. It is not intended for production use. There are no stability guarantees — APIs, data formats, and project structure may change at any time.
