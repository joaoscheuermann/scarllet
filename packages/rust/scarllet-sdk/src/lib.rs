/// User-facing configuration (provider keys, model selection, etc.).
pub mod config;
/// Lockfile management for the core daemon's singleton instance.
pub mod lockfile;
/// Module manifest schema used by agents, tools, and commands to declare
/// their identity and capabilities.
pub mod manifest;

/// Re-export the protobuf bindings so downstream crates only need `scarllet-sdk`.
pub use scarllet_proto::proto;
