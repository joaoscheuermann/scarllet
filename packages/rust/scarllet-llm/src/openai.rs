use crate::error::LlmError;
use crate::types::*;
use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

/// Endpoint paths that users commonly paste as part of the base URL.
/// [`normalize_base_url`] strips these so the client can append them itself.
const KNOWN_ENDPOINT_SUFFIXES: &[&str] = &["/chat/completions", "/completions", "/models"];

/// Strips trailing slashes and known endpoint paths so users can paste the full
/// URL from documentation (e.g. `https://openrouter.ai/api/v1/chat/completions`)
/// and still get a valid base URL (`https://openrouter.ai/api/v1`).
fn normalize_base_url(url: &str) -> String {
    let mut url = url.trim().to_string();
    for suffix in KNOWN_ENDPOINT_SUFFIXES {
        if let Some(stripped) = url.strip_suffix(suffix) {
            tracing::warn!(
                "api_url contained endpoint path '{}', stripping it — use the base URL instead (e.g. https://openrouter.ai/api/v1)",
                suffix,
            );
            url = stripped.to_string();
            break;
        }
    }
    url.trim_end_matches('/').to_string()
}

/// [`LlmProvider`] targeting the OpenAI chat-completions API.
///
/// Also works with any compatible endpoint (OpenRouter, vLLM, Ollama, etc.)
/// by accepting a custom `base_url`.
pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiProvider {
    /// Initialises the provider with credentials and a base URL that will be
    /// normalised (trailing slashes / endpoint paths stripped).
    pub fn new(api_key: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        let base_url = normalize_base_url(&base_url);
        Self {
            api_key,
            base_url,
            http,
        }
    }

    /// Validates that the provider has a non-empty URL with an HTTP(S) scheme
    /// and a non-empty API key before making any network calls.
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.base_url.is_empty() {
            return Err(LlmError::InvalidConfig(
                "api_url is empty — set it to the provider's base URL (e.g. https://openrouter.ai/api/v1)".into(),
            ));
        }
        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            return Err(LlmError::InvalidConfig(format!(
                "api_url must start with http:// or https://, got: {}",
                self.base_url,
            )));
        }
        if self.api_key.is_empty() {
            return Err(LlmError::InvalidConfig("api_key is empty".into()));
        }
        Ok(())
    }

    /// Fallback: fetches the full `/models` list and searches for the model
    /// entry when the direct `/models/{id}` endpoint is not available.
    async fn get_context_window_from_list(&self, model: &str) -> Result<u32, LlmError> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !resp.status().is_success() {
            tracing::debug!("GET {url} returned {}, cannot determine context window", resp.status());
            return Ok(0);
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        Ok(find_model_in_response(&body, model)
            .and_then(extract_context_length)
            .unwrap_or(0))
    }

    /// Converts the provider-agnostic message list into the OpenAI wire format.
    fn build_oai_messages(messages: &[ChatMessage]) -> Vec<OaiMessage> {
        messages
            .iter()
            .map(|m| {
                let tool_calls = m.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| OaiToolCall {
                            id: tc.id.clone(),
                            r#type: tc.tool_type.clone(),
                            function: OaiFunctionCall {
                                name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            },
                        })
                        .collect()
                });

                OaiMessage {
                    role: role_to_string(&m.role),
                    content: if m.content.is_empty() && tool_calls.is_some() {
                        None
                    } else {
                        Some(m.content.clone())
                    },
                    tool_calls,
                    tool_call_id: m.tool_call_id.clone(),
                }
            })
            .collect()
    }

    /// Produces an [`OaiReasoningConfig`] unless the caller already supplied a
    /// Google-style `thinking_config` in `extra_body` (avoids double-sending).
    fn resolve_reasoning(
        reasoning_effort: Option<String>,
        extra_body: &Option<serde_json::Value>,
    ) -> Option<OaiReasoningConfig> {
        let has_thinking_config = extra_body
            .as_ref()
            .and_then(|v| v.get("google"))
            .and_then(|v| v.get("thinking_config"))
            .is_some();

        if has_thinking_config {
            None
        } else {
            reasoning_effort.map(|effort| OaiReasoningConfig { effort })
        }
    }
}

/// Reasoning-effort hint sent as the `reasoning` field in the request body.
#[derive(Serialize)]
struct OaiReasoningConfig {
    effort: String,
}

/// Wire-format body sent to `/chat/completions`.
#[derive(Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OaiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OaiReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_body: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OaiToolDef>>,
}

/// Options appended when `stream: true` to request usage metadata in the
/// final SSE chunk.
#[derive(Serialize)]
struct OaiStreamOptions {
    include_usage: bool,
}

/// Single message in the OpenAI chat-completions wire format.
#[derive(Serialize, Deserialize)]
struct OaiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// Tool declaration in the OpenAI `tools` array.
#[derive(Serialize, Deserialize)]
struct OaiToolDef {
    r#type: String,
    function: OaiFunctionDef,
}

/// Function schema within an [`OaiToolDef`].
#[derive(Serialize, Deserialize)]
struct OaiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Tool invocation returned by the model in a non-streaming response.
#[derive(Serialize, Deserialize, Clone)]
struct OaiToolCall {
    id: String,
    r#type: String,
    function: OaiFunctionCall,
}

/// Name + serialised arguments for a single function call.
#[derive(Serialize, Deserialize, Clone)]
struct OaiFunctionCall {
    name: String,
    arguments: String,
}

/// Top-level non-streaming response from `/chat/completions`.
#[derive(Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

/// A single completion choice from the response.
#[derive(Deserialize)]
struct OaiChoice {
    message: OaiChoiceMessage,
    finish_reason: Option<String>,
}

/// Message body inside a non-streaming choice, including optional reasoning
/// fields used by o-series and compatible models.
#[derive(Deserialize)]
struct OaiChoiceMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OaiToolCall>>,
}

/// Token-usage counters returned by the provider.
#[derive(Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

/// Single SSE chunk in a streaming response.
#[derive(Deserialize)]
struct OaiStreamChunk {
    choices: Vec<OaiStreamChoice>,
    usage: Option<OaiUsage>,
}

/// Choice wrapper inside a streaming chunk.
#[derive(Deserialize)]
struct OaiStreamChoice {
    delta: OaiStreamDelta,
    finish_reason: Option<String>,
}

/// Incremental delta payload within a streaming choice.
#[derive(Deserialize, Default)]
struct OaiStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OaiStreamToolCall>>,
}

/// Partial tool-call emitted over multiple streaming chunks.
#[derive(Deserialize)]
struct OaiStreamToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OaiStreamFunctionCall>,
}

/// Partial function call data within a streaming tool-call delta.
#[derive(Deserialize)]
struct OaiStreamFunctionCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Maps a [`Role`] to the lowercase string the OpenAI API expects.
fn role_to_string(r: &Role) -> String {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
    .to_string()
}

/// Parses an OpenAI role string back into a [`Role`], defaulting unknown
/// values to [`Role::User`].
fn string_to_role(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}

/// Inspects the HTTP status for auth (401) or rate-limit (429) errors before
/// the body is consumed, returning `None` for all other statuses.
fn check_response_status(resp: &reqwest::Response) -> Option<LlmError> {
    let status = resp.status().as_u16();
    if status == 401 {
        return Some(LlmError::Unauthorized);
    }
    if status == 429 {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        return Some(LlmError::RateLimited { retry_after });
    }
    None
}

/// Consumes the response body and wraps it in a [`LlmError::ServerError`].
async fn read_error_body(resp: reqwest::Response) -> LlmError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    LlmError::ServerError { status, body }
}

/// Extracts complete `data:` payloads from the SSE byte buffer, removing
/// consumed bytes and leaving any partial block for the next read.
fn drain_sse_events(buf: &mut BytesMut) -> Vec<String> {
    let mut events = Vec::new();
    while let Ok(text) = std::str::from_utf8(buf) {
        let Some(pos) = text.find("\n\n") else {
            break;
        };
        let block: String = text[..pos].to_string();
        let drain_len = pos + 2;
        let _ = buf.split_to(drain_len);

        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                let trimmed = data.trim();
                if !trimmed.is_empty() && trimmed != "[DONE]" {
                    events.push(trimmed.to_string());
                }
            }
        }
    }
    events
}

/// Converts the generic tool definitions into the OpenAI wire-format `tools` array.
fn convert_tools(tools: &Option<Vec<ToolDefinition>>) -> Option<Vec<OaiToolDef>> {
    tools.as_ref().map(|defs| {
        defs.iter()
            .map(|t| OaiToolDef {
                r#type: t.tool_type.clone(),
                function: OaiFunctionDef {
                    name: t.function.name.clone(),
                    description: t.function.description.clone(),
                    parameters: t.function.parameters.clone(),
                },
            })
            .collect()
    })
}

/// Extracts `context_length` or `context_window` from a single model JSON object,
/// falling back to `top_provider.context_length` (OpenRouter-specific).
fn extract_context_length(obj: &serde_json::Value) -> Option<u32> {
    obj.get("context_length")
        .or_else(|| obj.get("context_window"))
        .and_then(|v| v.as_u64())
        .or_else(|| {
            obj.get("top_provider")
                .and_then(|tp| tp.get("context_length"))
                .and_then(|v| v.as_u64())
        })
        .map(|v| v as u32)
        .filter(|&v| v > 0)
}

/// Finds a model by `id` inside a `{ "data": [...] }` wrapper response.
fn find_model_in_response<'a>(body: &'a serde_json::Value, model: &str) -> Option<&'a serde_json::Value> {
    body.get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.iter().find(|m| m.get("id").and_then(|v| v.as_str()) == Some(model)))
}

/// Deserializes a single SSE `data:` payload into a [`ChatStreamEvent`],
/// returning `None` for empty or no-op chunks.
fn parse_stream_chunk(data: &str) -> Option<ChatStreamEvent> {
    let chunk: OaiStreamChunk = serde_json::from_str(data).ok()?;

    let usage = chunk.usage.map(|u| Usage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
    });

    let Some(choice) = chunk.choices.into_iter().next() else {
        if usage.is_some() {
            return Some(ChatStreamEvent {
                deltas: Vec::new(),
                finish_reason: None,
                tool_calls: Vec::new(),
                usage,
            });
        }
        return None;
    };

    let content = choice.delta.content.unwrap_or_default();
    let reasoning = choice
        .delta
        .reasoning_content
        .or(choice.delta.reasoning)
        .unwrap_or_default();

    let mut deltas = Vec::new();
    if !reasoning.is_empty() {
        deltas.push(StreamDelta::Thought(reasoning));
    }
    if !content.is_empty() {
        deltas.push(StreamDelta::Content(content));
    }

    let tool_calls: Vec<ToolCallDelta> = choice
        .delta
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tc| ToolCallDelta {
            index: tc.index,
            id: tc.id,
            function_name: tc.function.as_ref().and_then(|f| f.name.clone()),
            function_arguments: tc.function.and_then(|f| f.arguments),
            thought_signature: None,
        })
        .collect();

    if deltas.is_empty() && choice.finish_reason.is_none() && tool_calls.is_empty() && usage.is_none() {
        return None;
    }

    Some(ChatStreamEvent {
        deltas,
        finish_reason: choice.finish_reason,
        tool_calls,
        usage,
    })
}

/// Spawns a tokio task that reads the byte stream and sends parsed events through a channel.
/// Returns the receiving end as a `ChatStream`.
fn spawn_sse_reader(resp: reqwest::Response) -> ChatStream {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatStreamEvent, LlmError>>(64);

    tokio::spawn(async move {
        let mut byte_stream = resp.bytes_stream();
        let mut buf = BytesMut::new();

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    for data in drain_sse_events(&mut buf) {
                        if let Some(event) = parse_stream_chunk(&data) {
                            if tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(LlmError::NetworkError(e.to_string()))).await;
                    return;
                }
            }
        }

        for data in drain_sse_events(&mut buf) {
            if let Some(event) = parse_stream_chunk(&data) {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
        }
    });

    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    /// Sends a non-streaming chat-completions request and maps the response
    /// into the unified [`ChatResponse`], including reasoning blocks if present.
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.validate()?;
        let reasoning = Self::resolve_reasoning(request.reasoning_effort, &request.extra_body);
        let tools = convert_tools(&request.tools);
        let oai_req = OaiRequest {
            model: request.model,
            messages: Self::build_oai_messages(&request.messages),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: false,
            stream_options: None,
            reasoning,
            extra_body: request.extra_body,
            tools,
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

        if let Some(err) = check_response_status(&resp) {
            return Err(err);
        }
        if !resp.status().is_success() {
            return Err(read_error_body(resp).await);
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

        let usage = oai_resp
            .usage
            .map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            })
            .unwrap_or_default();

        let tool_calls = choice.message.tool_calls.map(|tcs| {
            tcs.into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    tool_type: tc.r#type,
                    function: crate::types::FunctionCall {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    },
                    thought_signature: None,
                })
                .collect()
        });

        let reasoning_text = choice
            .message
            .reasoning_content
            .or(choice.message.reasoning)
            .unwrap_or_default();
        let content_text = choice.message.content.unwrap_or_default();

        let mut blocks = Vec::new();
        if !reasoning_text.is_empty() {
            blocks.push(ContentBlock {
                block_type: ContentBlockType::Thought,
                text: reasoning_text,
            });
        }
        if !content_text.is_empty() {
            blocks.push(ContentBlock {
                block_type: ContentBlockType::Text,
                text: content_text.clone(),
            });
        }

        Ok(ChatResponse {
            message: ChatMessage {
                role: string_to_role(&choice.message.role),
                content: content_text,
                tool_calls,
                tool_call_id: None,
            },
            blocks,
            usage,
            finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".into()),
        })
    }

    /// Opens a streaming chat-completions request and returns the event
    /// stream via [`spawn_sse_reader`].
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        self.validate()?;
        let reasoning = Self::resolve_reasoning(request.reasoning_effort, &request.extra_body);
        let tools = convert_tools(&request.tools);
        let oai_req = OaiRequest {
            model: request.model,
            messages: Self::build_oai_messages(&request.messages),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
            stream_options: Some(OaiStreamOptions { include_usage: true }),
            reasoning,
            extra_body: request.extra_body,
            tools,
        };

        let url = format!("{}/chat/completions", self.base_url);
        tracing::debug!("POST {url} (stream) model={}", oai_req.model);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&oai_req)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        tracing::debug!("Stream response status: {}", resp.status());

        if let Some(err) = check_response_status(&resp) {
            return Err(err);
        }
        if !resp.status().is_success() {
            return Err(read_error_body(resp).await);
        }

        Ok(spawn_sse_reader(resp))
    }

    /// Queries the provider for the model's context window size, trying the
    /// direct `/models/{id}` endpoint first and falling back to the full list.
    async fn get_context_window(&self, model: &str) -> Result<u32, LlmError> {
        let url = format!("{}/models/{}", self.base_url, model);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !resp.status().is_success() {
            tracing::debug!("GET {url} returned {}, falling back to /models list", resp.status());
            return self.get_context_window_from_list(model).await;
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        if let Some(ctx) = extract_context_length(&body) {
            return Ok(ctx);
        }

        if let Some(model_obj) = find_model_in_response(&body, model) {
            if let Some(ctx) = extract_context_length(model_obj) {
                return Ok(ctx);
            }
        }

        Ok(0)
    }
}

#[cfg(test)]
mod tests;
