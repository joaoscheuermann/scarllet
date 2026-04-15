use crate::error::LlmError;
use crate::types::*;
use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

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
        let base_url = normalize_base_url(&base_url);
        Self {
            api_key,
            base_url,
            http,
        }
    }

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

#[derive(Serialize)]
struct OaiReasoningConfig {
    effort: String,
}

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

#[derive(Serialize)]
struct OaiStreamOptions {
    include_usage: bool,
}

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

#[derive(Serialize, Deserialize)]
struct OaiToolDef {
    r#type: String,
    function: OaiFunctionDef,
}

#[derive(Serialize, Deserialize)]
struct OaiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct OaiToolCall {
    id: String,
    r#type: String,
    function: OaiFunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
struct OaiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiChoiceMessage,
    finish_reason: Option<String>,
}

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

#[derive(Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Deserialize)]
struct OaiStreamChunk {
    choices: Vec<OaiStreamChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Deserialize)]
struct OaiStreamChoice {
    delta: OaiStreamDelta,
    finish_reason: Option<String>,
}

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

#[derive(Deserialize)]
struct OaiStreamToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OaiStreamFunctionCall>,
}

#[derive(Deserialize)]
struct OaiStreamFunctionCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
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

async fn read_error_body(resp: reqwest::Response) -> LlmError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    LlmError::ServerError { status, body }
}

fn drain_sse_events(buf: &mut BytesMut) -> Vec<String> {
    let mut events = Vec::new();
    loop {
        let text = match std::str::from_utf8(buf) {
            Ok(t) => t,
            Err(_) => break,
        };
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
mod tests {
    use super::*;

    #[test]
    fn role_roundtrip() {
        assert_eq!(role_to_string(&Role::System), "system");
        assert!(matches!(string_to_role("assistant"), Role::Assistant));
        assert!(matches!(string_to_role("unknown"), Role::User));
    }

    #[test]
    fn parse_sse_basic() {
        let mut buf = BytesMut::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
        );
        let events = drain_sse_events(&mut buf);
        assert_eq!(events.len(), 1);
        let event = parse_stream_chunk(&events[0]).unwrap();
        assert_eq!(event.deltas.len(), 1);
        assert!(matches!(&event.deltas[0], StreamDelta::Content(t) if t == "Hello"));
        assert!(event.finish_reason.is_none());
        assert!(event.tool_calls.is_empty());
    }

    #[test]
    fn parse_sse_done_ignored() {
        let mut buf = BytesMut::from("data: [DONE]\n\n");
        let events = drain_sse_events(&mut buf);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_sse_partial_buffer() {
        let mut buf = BytesMut::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"");
        let events = drain_sse_events(&mut buf);
        assert!(events.is_empty(), "incomplete SSE block should not yield events");
        assert!(!buf.is_empty(), "partial data should remain in buffer");
    }

    #[test]
    fn parse_reasoning_content_as_thought_delta() {
        let data = r#"{"choices":[{"delta":{"reasoning_content":"thinking...","content":""},"finish_reason":null}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert_eq!(event.deltas.len(), 1);
        assert!(matches!(&event.deltas[0], StreamDelta::Thought(t) if t == "thinking..."));
    }

    #[test]
    fn parse_reasoning_field_as_thought_delta() {
        let data = r#"{"choices":[{"delta":{"reasoning":"step by step...","content":""},"finish_reason":null}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert_eq!(event.deltas.len(), 1);
        assert!(matches!(&event.deltas[0], StreamDelta::Thought(t) if t == "step by step..."));
    }

    #[test]
    fn parse_reasoning_content_preferred_over_reasoning() {
        let data = r#"{"choices":[{"delta":{"reasoning_content":"native","reasoning":"normalized","content":""},"finish_reason":null}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert_eq!(event.deltas.len(), 1);
        assert!(matches!(&event.deltas[0], StreamDelta::Thought(t) if t == "native"));
    }

    #[test]
    fn resolve_reasoning_builds_config() {
        let config = OpenAiProvider::resolve_reasoning(Some("high".into()), &None);
        assert!(config.is_some());
        let json = serde_json::to_value(config.unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({"effort": "high"}));
    }

    #[test]
    fn resolve_reasoning_none_when_no_effort() {
        let config = OpenAiProvider::resolve_reasoning(None, &None);
        assert!(config.is_none());
    }

    #[test]
    fn resolve_reasoning_none_with_google_thinking_config() {
        let extra = Some(serde_json::json!({"google": {"thinking_config": {}}}));
        let config = OpenAiProvider::resolve_reasoning(Some("high".into()), &extra);
        assert!(config.is_none());
    }

    #[test]
    fn oai_request_serializes_reasoning_object() {
        let req = OaiRequest {
            model: "test".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            stream_options: None,
            reasoning: Some(OaiReasoningConfig { effort: "high".into() }),
            extra_body: None,
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["reasoning"], serde_json::json!({"effort": "high"}));
        assert!(json.get("reasoning_effort").is_none());
    }

    #[test]
    fn oai_request_omits_reasoning_when_none() {
        let req = OaiRequest {
            model: "test".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            stream_options: None,
            reasoning: None,
            extra_body: None,
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("reasoning").is_none());
    }

    #[test]
    fn parse_content_only() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert_eq!(event.deltas.len(), 1);
        assert!(matches!(&event.deltas[0], StreamDelta::Content(t) if t == "Hello"));
    }

    #[test]
    fn parse_tool_call_delta() {
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_123","function":{"name":"terminal","arguments":"{\"co"}}]},"finish_reason":null}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert!(event.deltas.is_empty());
        assert_eq!(event.tool_calls.len(), 1);
        assert_eq!(event.tool_calls[0].index, 0);
        assert_eq!(event.tool_calls[0].id.as_deref(), Some("call_123"));
        assert_eq!(
            event.tool_calls[0].function_name.as_deref(),
            Some("terminal")
        );
        assert_eq!(
            event.tool_calls[0].function_arguments.as_deref(),
            Some("{\"co")
        );
    }

    #[test]
    fn parse_finish_reason_tool_calls() {
        let data =
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert_eq!(event.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn normalize_strips_chat_completions_suffix() {
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1/chat/completions"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn normalize_strips_multiple_trailing_slashes() {
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1///"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn normalize_strips_completions_suffix() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/completions"),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn normalize_strips_models_suffix() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/models"),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn normalize_preserves_correct_base_url() {
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_base_url("  https://openrouter.ai/api/v1  "),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn normalize_handles_localhost() {
        assert_eq!(
            normalize_base_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1"
        );
    }

    #[test]
    fn validate_rejects_empty_url() {
        let p = OpenAiProvider::new("sk-test".into(), "".into());
        assert!(matches!(p.validate(), Err(LlmError::InvalidConfig(_))));
    }

    #[test]
    fn validate_rejects_missing_scheme() {
        let p = OpenAiProvider::new("sk-test".into(), "openrouter.ai/api/v1".into());
        assert!(matches!(p.validate(), Err(LlmError::InvalidConfig(_))));
    }

    #[test]
    fn validate_rejects_empty_api_key() {
        let p = OpenAiProvider::new("".into(), "https://openrouter.ai/api/v1".into());
        assert!(matches!(p.validate(), Err(LlmError::InvalidConfig(_))));
    }

    #[test]
    fn validate_accepts_valid_config() {
        let p = OpenAiProvider::new("sk-test".into(), "https://openrouter.ai/api/v1".into());
        assert!(p.validate().is_ok());
    }

    #[test]
    fn extract_context_length_from_flat_object() {
        let body = serde_json::json!({"id": "gpt-4", "context_length": 128000});
        assert_eq!(extract_context_length(&body), Some(128000));
    }

    #[test]
    fn extract_context_length_from_context_window_field() {
        let body = serde_json::json!({"id": "gpt-4", "context_window": 64000});
        assert_eq!(extract_context_length(&body), Some(64000));
    }

    #[test]
    fn extract_context_length_from_top_provider() {
        let body = serde_json::json!({
            "id": "qwen/qwen3.6-plus",
            "top_provider": {"context_length": 131072}
        });
        assert_eq!(extract_context_length(&body), Some(131072));
    }

    #[test]
    fn extract_context_length_prefers_root_over_top_provider() {
        let body = serde_json::json!({
            "id": "model",
            "context_length": 200000,
            "top_provider": {"context_length": 100000}
        });
        assert_eq!(extract_context_length(&body), Some(200000));
    }

    #[test]
    fn extract_context_length_none_when_missing() {
        let body = serde_json::json!({"id": "model"});
        assert_eq!(extract_context_length(&body), None);
    }

    #[test]
    fn extract_context_length_none_when_zero() {
        let body = serde_json::json!({"id": "model", "context_length": 0});
        assert_eq!(extract_context_length(&body), None);
    }

    #[test]
    fn find_model_in_data_array() {
        let body = serde_json::json!({
            "data": [
                {"id": "model-a", "context_length": 4096},
                {"id": "qwen/qwen3.6-plus", "context_length": 131072},
                {"id": "model-c", "context_length": 8192}
            ]
        });
        let found = find_model_in_response(&body, "qwen/qwen3.6-plus");
        assert!(found.is_some());
        assert_eq!(extract_context_length(found.unwrap()), Some(131072));
    }

    #[test]
    fn find_model_not_in_data_array() {
        let body = serde_json::json!({
            "data": [
                {"id": "model-a", "context_length": 4096}
            ]
        });
        assert!(find_model_in_response(&body, "missing-model").is_none());
    }

    #[test]
    fn find_model_no_data_field() {
        let body = serde_json::json!({"id": "model-a", "context_length": 4096});
        assert!(find_model_in_response(&body, "model-a").is_none());
    }
}
