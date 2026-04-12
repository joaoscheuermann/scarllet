use crate::error::LlmError;
use crate::types::*;
use serde::{Deserialize, Serialize};

pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            api_key,
            base_url,
            http,
        }
    }
}

#[derive(Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct OaiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

fn role_to_string(r: &Role) -> String {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
    .to_string()
}

fn string_to_role(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let oai_req = OaiRequest {
            model: request.model,
            messages: request
                .messages
                .iter()
                .map(|m| OaiMessage {
                    role: role_to_string(&m.role),
                    content: m.content.clone(),
                })
                .collect(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let url = format!("{}/chat/completions", self.base_url);
        tracing::debug!("POST {url} model={}", oai_req.model);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&oai_req)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        tracing::debug!("Response status: {}", resp.status());

        let status = resp.status().as_u16();

        if status == 401 {
            return Err(LlmError::Unauthorized);
        }
        if status == 429 {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok());
            return Err(LlmError::RateLimited { retry_after });
        }
        if status >= 500 {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::ServerError { status, body });
        }
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::ServerError { status, body });
        }

        let oai_resp: OaiResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        let choice = oai_resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::InvalidResponse("No choices in response".into()))?;

        let usage = oai_resp.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }).unwrap_or_default();

        Ok(ChatResponse {
            message: ChatMessage {
                role: string_to_role(&choice.message.role),
                content: choice.message.content,
            },
            usage,
            finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_roundtrip() {
        assert_eq!(role_to_string(&Role::System), "system");
        assert!(matches!(string_to_role("assistant"), Role::Assistant));
        assert!(matches!(string_to_role("unknown"), Role::User));
    }
}
