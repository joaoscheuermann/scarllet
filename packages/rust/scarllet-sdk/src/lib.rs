//! Scarllet SDK — stable surface shared between the core daemon and
//! agent / tool / command modules.
//!
//! Exposes configuration (`config`), manifest parsing (`manifest`), the
//! daemon lockfile (`lockfile`), and the agent-side gRPC helper
//! (`agent`) used by agent binaries to talk to core.

/// Agent-side helper for talking to the core orchestrator.
pub mod agent;
/// User-facing configuration (provider keys, model selection, etc.).
pub mod config;
/// Lockfile management for the core daemon's singleton instance.
pub mod lockfile;
/// Module manifest schema used by agents, tools, and commands to declare
/// their identity and capabilities.
pub mod manifest;

/// Re-export the protobuf bindings so downstream crates only need `scarllet-sdk`.
pub use scarllet_proto::proto;

/// Re-export the SDK error type at the crate root for ergonomics.
pub use agent::AgentSdkError;
