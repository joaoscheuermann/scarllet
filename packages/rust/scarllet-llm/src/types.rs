use std::pin::Pin;

use futures_core::Stream;
use serde::{Deserialize, Serialize};

/// Provider-agnostic chat request carrying the model name, conversation
/// history, generation parameters, and optional tool declarations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
}

/// Single message in a conversation, optionally carrying tool call results
/// or tool invocations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Participant role in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System-level instructions that prime the model.
    System,
    /// End-user input.
    User,
    /// Model-generated output.
    Assistant,
    /// Result returned from a tool invocation.
    Tool,
}

/// Declaration of a callable tool exposed to the model for function-calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

/// Schema describing a single function the model may invoke.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Concrete tool invocation emitted by the model, pairing an id with the
/// function name and serialised arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionCall,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Name and JSON-encoded arguments for a function the model wants to call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Incremental tool-call fragment received during streaming, identified by
/// its positional index within the current response.
#[derive(Debug, Clone, Default)]
pub struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub function_name: Option<String>,
    pub function_arguments: Option<String>,
    pub thought_signature: Option<String>,
}

/// Discriminates between model reasoning (chain-of-thought) and visible text
/// output within a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentBlockType {
    /// Internal reasoning / chain-of-thought.
    Thought,
    /// User-facing text content.
    Text,
}

/// Typed segment of a model response — either reasoning or text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    pub block_type: ContentBlockType,
    pub text: String,
}

/// Incremental content fragment received during streaming.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// Chain-of-thought reasoning fragment.
    Thought(String),
    /// User-facing text fragment.
    Content(String),
}

/// Complete (non-streaming) response from an LLM provider, combining the
/// assistant message, structured content blocks, token usage, and stop reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub message: ChatMessage,
    pub blocks: Vec<ContentBlock>,
    pub usage: Usage,
    pub finish_reason: String,
}

/// Token-usage counters for a single request/response round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Single event emitted on the streaming channel, carrying content deltas,
/// tool-call fragments, an optional finish reason, and optional usage stats.
#[derive(Debug, Clone)]
pub struct ChatStreamEvent {
    pub deltas: Vec<StreamDelta>,
    pub finish_reason: Option<String>,
    pub tool_calls: Vec<ToolCallDelta>,
    pub usage: Option<Usage>,
}

/// Async stream of [`ChatStreamEvent`]s, pin-boxed for object safety.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, crate::LlmError>> + Send>>;

/// Backend-agnostic interface every LLM provider must implement.
///
/// Supports non-streaming chat, streaming chat, and context-window queries.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Sends a chat request and returns the complete response.
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, crate::LlmError>;
    /// Sends a chat request and returns an async stream of incremental events.
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, crate::LlmError>;
    /// Returns the model's maximum input token limit.
    async fn get_context_window(&self, model: &str) -> Result<u32, crate::LlmError>;
}
