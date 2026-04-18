//! `scarllet-core` library surface.
//!
//! The orchestrator daemon in `src/main.rs` re-uses every module exposed
//! from this crate root; integration tests under `tests/` link against
//! this library to drive the `OrchestratorService` directly (for example
//! the bidi `AgentStream` handshake test at
//! `tests/agent_stream_handshake.rs`).

/// Per-session agent orchestration (main + sub-agents, routing, stream).
pub mod agents;
/// Module manifest registry populated by the filesystem watcher.
pub mod registry;
/// gRPC service wiring — thin delegation layer over the sibling modules.
pub mod service;
/// Per-session state (node graph, queue, subscribers).
pub mod session;
/// Built-in tool invocation machinery (including `spawn_sub_agent`).
pub mod tools;
/// Filesystem watchers for module manifests and global config.
pub mod watcher;
