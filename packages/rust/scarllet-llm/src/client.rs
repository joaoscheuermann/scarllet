use crate::error::LlmError;
use crate::openai::OpenAiProvider;
use crate::types::*;

pub struct LlmClient {
    provider: OpenAiProvider,
}

impl LlmClient {
    pub fn new(api_url: String, api_key: String) -> Self {
        Self {
            provider: OpenAiProvider::new(api_key, api_url),
        }
    }

    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.provider.chat(request).await
    }

    pub async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        self.provider.chat_stream(request).await
    }
}
