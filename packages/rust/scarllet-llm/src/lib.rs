/// Provider-agnostic LLM client facade.
pub mod client;
/// Unified error type for all LLM operations.
pub mod error;
/// Google Gemini / Generative Language provider implementation.
pub mod gemini;
/// OpenAI-compatible chat-completions provider implementation.
pub mod openai;
/// Shared request, response, and streaming types used across providers.
pub mod types;

pub use client::LlmClient;
pub use error::LlmError;
pub use types::*;
