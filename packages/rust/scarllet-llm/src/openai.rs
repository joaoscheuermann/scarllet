use crate::error::LlmError;
use crate::types::*;
use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

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
    ) -> Option<String> {
        let has_thinking_config = extra_body
            .as_ref()
            .and_then(|v| v.get("google"))
            .and_then(|v| v.get("thinking_config"))
            .is_some();

        if has_thinking_config {
            None
        } else {
            reasoning_effort
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_body: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OaiToolDef>>,
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

fn parse_stream_chunk(data: &str) -> Option<ChatStreamEvent> {
    let chunk: OaiStreamChunk = serde_json::from_str(data).ok()?;
    let choice = chunk.choices.into_iter().next()?;

    let content = choice.delta.content.unwrap_or_default();
    let reasoning = choice.delta.reasoning_content.unwrap_or_default();

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

    if deltas.is_empty() && choice.finish_reason.is_none() && tool_calls.is_empty() {
        return None;
    }

    Some(ChatStreamEvent {
        deltas,
        finish_reason: choice.finish_reason,
        tool_calls,
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
        let reasoning = Self::resolve_reasoning(request.reasoning_effort, &request.extra_body);
        let tools = convert_tools(&request.tools);
        let oai_req = OaiRequest {
            model: request.model,
            messages: Self::build_oai_messages(&request.messages),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: false,
            reasoning_effort: reasoning,
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

        let content_text = choice.message.content.unwrap_or_default();
        let blocks = if content_text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock {
                block_type: ContentBlockType::Text,
                text: content_text.clone(),
            }]
        };

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
        let reasoning = Self::resolve_reasoning(request.reasoning_effort, &request.extra_body);
        let tools = convert_tools(&request.tools);
        let oai_req = OaiRequest {
            model: request.model,
            messages: Self::build_oai_messages(&request.messages),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
            reasoning_effort: reasoning,
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
    fn parse_reasoning_as_thought_delta() {
        let data = r#"{"choices":[{"delta":{"reasoning_content":"thinking...","content":""},"finish_reason":null}]}"#;
        let event = parse_stream_chunk(data).unwrap();
        assert_eq!(event.deltas.len(), 1);
        assert!(matches!(&event.deltas[0], StreamDelta::Thought(t) if t == "thinking..."));
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
}
