use serde::{Deserialize, Serialize};

/// Metadata that every loadable module (agent, tool, or command) must declare.
///
/// The manifest drives discovery, routing, and capability negotiation inside
/// the core daemon. It is typically embedded in the module binary and reported
/// during the registration handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub name: String,
    pub kind: ModuleKind,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Categorises a module so the core daemon can apply the correct lifecycle
/// and routing semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleKind {
    /// Slash-command that runs synchronously and returns once.
    Command,
    /// Callable tool exposed to LLM function-calling.
    Tool,
    /// Long-running conversational agent backed by an LLM.
    Agent,
}

#[cfg(test)]
mod tests;
