use crate::error::LlmError;
use crate::gemini::GeminiProvider;
use crate::openai::OpenAiProvider;
use crate::types::*;

/// Provider-agnostic entry point for LLM interactions.
///
/// Wraps a concrete provider behind the [`LlmProvider`] trait so callers can
/// switch between OpenAI-compatible and Gemini backends without changing
/// application code.
pub struct LlmClient {
    provider: Box<dyn LlmProvider>,
}

impl LlmClient {
    /// Constructs a client targeting an OpenAI-compatible endpoint.
    pub fn new_openai(api_url: String, api_key: String) -> Self {
        Self {
            provider: Box::new(OpenAiProvider::new(api_key, api_url)),
        }
    }

    /// Constructs a client targeting the Google Gemini API.
    pub fn new_gemini(api_key: String) -> Self {
        Self {
            provider: Box::new(GeminiProvider::new(api_key)),
        }
    }

    /// Sends a chat request and waits for the complete response.
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.provider.chat(request).await
    }

    /// Sends a chat request and returns a streaming response.
    pub async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        self.provider.chat_stream(request).await
    }

    /// Queries the provider for the model's maximum input token limit.
    pub async fn get_context_window(&self, model: &str) -> Result<u32, LlmError> {
        self.provider.get_context_window(model).await
    }
}
