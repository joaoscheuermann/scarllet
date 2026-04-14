use crate::error::LlmError;
use crate::types::*;
use gemini_rust::client::Gemini;
use gemini_rust::generation::builder::ContentBuilder;
use gemini_rust::generation::model::{FinishReason, GenerationResponse};
use gemini_rust::tools::model::FunctionDeclaration;
use gemini_rust::Part;
use tokio_stream::StreamExt;

pub struct GeminiProvider {
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    fn create_client(&self, model: &str) -> Result<Gemini, LlmError> {
        let model_path = if model.starts_with("models/") {
            model.to_string()
        } else {
            format!("models/{model}")
        };
        Gemini::with_model(&self.api_key, model_path)
            .map_err(|e| LlmError::NetworkError(format!("Failed to create Gemini client: {e:?}")))
    }

    fn build_request(
        &self,
        client: &Gemini,
        request: &ChatRequest,
    ) -> Result<ContentBuilder, LlmError> {
        let mut builder = client.generate_content();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    builder = builder.with_system_prompt(&msg.content);
                }
                Role::User => {
                    builder = builder.with_user_message(&msg.content);
                }
                Role::Assistant => {
                    if !msg.content.is_empty() {
                        builder = builder.with_model_message(&msg.content);
                    }

                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(serde_json::json!({}));
                            let fc = gemini_rust::tools::model::FunctionCall {
                                name: tc.function.name.clone(),
                                args,
                                thought_signature: None,
                            };
                            builder.contents.push(gemini_rust::Content {
                                parts: Some(vec![Part::FunctionCall {
                                    function_call: fc,
                                    thought_signature: tc.thought_signature.clone(),
                                }]),
                                role: Some(gemini_rust::Role::Model),
                            });
                        }
                    }
                }
                Role::Tool => {
                    let fn_name = msg.tool_call_id.as_deref().unwrap_or("unknown");
                    builder = builder
                        .with_function_response_str(fn_name, &msg.content)
                        .map_err(|e| {
                            LlmError::InvalidResponse(format!(
                                "Failed to add function response: {e}"
                            ))
                        })?;
                }
            }
        }

        if let Some(ref tools) = request.tools {
            for tool_def in tools {
                let decl_json = serde_json::json!({
                    "name": tool_def.function.name,
                    "description": tool_def.function.description,
                    "parameters": tool_def.function.parameters,
                });
                let decl: FunctionDeclaration =
                    serde_json::from_value(decl_json).unwrap_or_else(|_| {
                        FunctionDeclaration::new(
                            &tool_def.function.name,
                            &tool_def.function.description,
                            None,
                        )
                    });
                builder = builder.with_function(decl);
            }
        }

        if let Some(ref effort) = request.reasoning_effort {
            let level = match effort.to_lowercase().as_str() {
                "low" => gemini_rust::generation::model::ThinkingLevel::Low,
                "medium" => gemini_rust::generation::model::ThinkingLevel::Medium,
                _ => gemini_rust::generation::model::ThinkingLevel::High,
            };
            builder = builder
                .with_thinking_level(level)
                .with_thoughts_included(true);
        }

        if let Some(temp) = request.temperature {
            builder = builder.with_temperature(temp);
        }

        if let Some(max_tokens) = request.max_tokens {
            builder = builder.with_max_output_tokens(max_tokens as i32);
        }

        Ok(builder)
    }
}

fn extract_response(resp: &GenerationResponse) -> (Vec<ContentBlock>, Vec<ToolCall>, String) {
    let mut blocks = Vec::new();
    let mut tool_calls = Vec::new();
    let mut finish_reason = "stop".to_string();

    let candidate = match resp.candidates.first() {
        Some(c) => c,
        None => return (blocks, tool_calls, finish_reason),
    };

    if let Some(ref fr) = candidate.finish_reason {
        finish_reason = match fr {
            FinishReason::Stop => "stop".to_string(),
            FinishReason::MaxTokens => "length".to_string(),
            _ => format!("{fr:?}").to_lowercase(),
        };
    }

    let parts = match &candidate.content.parts {
        Some(p) => p,
        None => return (blocks, tool_calls, finish_reason),
    };

    for part in parts {
        match part {
            Part::Text {
                text: t, thought, ..
            } => {
                let bt = if thought.unwrap_or(false) {
                    ContentBlockType::Thought
                } else {
                    ContentBlockType::Text
                };
                blocks.push(ContentBlock {
                    block_type: bt,
                    text: t.clone(),
                });
            }
            Part::FunctionCall {
                function_call: fc,
                thought_signature: part_sig,
            } => {
                let args_str = serde_json::to_string(&fc.args).unwrap_or_else(|_| "{}".into());
                let id = uuid::Uuid::new_v4().to_string();
                let sig = fc
                    .thought_signature
                    .clone()
                    .or_else(|| part_sig.clone());

                tool_calls.push(ToolCall {
                    id,
                    tool_type: "function".to_string(),
                    function: crate::types::FunctionCall {
                        name: fc.name.clone(),
                        arguments: args_str,
                    },
                    thought_signature: sig,
                });
            }
            _ => {}
        }
    }

    if !tool_calls.is_empty() {
        finish_reason = "tool_calls".to_string();
    }

    (blocks, tool_calls, finish_reason)
}

#[async_trait::async_trait]
impl LlmProvider for GeminiProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let client = self.create_client(&request.model)?;
        let builder = self.build_request(&client, &request)?;

        let resp = builder
            .execute()
            .await
            .map_err(|e| LlmError::NetworkError(format!("{e:?}")))?;

        let (blocks, tool_calls, finish_reason) = extract_response(&resp);

        let content: String = blocks
            .iter()
            .filter(|b| b.block_type == ContentBlockType::Text)
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("");

        let usage = resp
            .usage_metadata
            .map(|u| Usage {
                prompt_tokens: u.prompt_token_count.unwrap_or(0) as u32,
                completion_tokens: u.candidates_token_count.unwrap_or(0) as u32,
                total_tokens: u.total_token_count.unwrap_or(0) as u32,
            })
            .unwrap_or_default();

        let tc = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        Ok(ChatResponse {
            message: ChatMessage {
                role: Role::Assistant,
                content,
                tool_calls: tc,
                tool_call_id: None,
            },
            blocks,
            usage,
            finish_reason,
        })
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        let client = self.create_client(&request.model)?;
        let builder = self.build_request(&client, &request)?;

        let stream = builder
            .execute_stream()
            .await
            .map_err(|e| LlmError::NetworkError(format!("{e:?}")))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatStreamEvent, LlmError>>(64);

        tokio::spawn(async move {
            tokio::pin!(stream);

            while let Some(result) = stream.next().await {
                match result {
                    Ok(resp) => {
                        let (blocks, tool_calls, finish_reason) = extract_response(&resp);

                        let usage = resp.usage_metadata.map(|u| Usage {
                            prompt_tokens: u.prompt_token_count.unwrap_or(0) as u32,
                            completion_tokens: u.candidates_token_count.unwrap_or(0) as u32,
                            total_tokens: u.total_token_count.unwrap_or(0) as u32,
                        });

                        let deltas: Vec<StreamDelta> = blocks
                            .into_iter()
                            .filter(|b| !b.text.is_empty())
                            .map(|b| match b.block_type {
                                ContentBlockType::Thought => StreamDelta::Thought(b.text),
                                ContentBlockType::Text => StreamDelta::Content(b.text),
                            })
                            .collect();

                        let tc_deltas: Vec<ToolCallDelta> = tool_calls
                            .into_iter()
                            .enumerate()
                            .map(|(i, tc)| ToolCallDelta {
                                index: i,
                                id: Some(tc.id),
                                function_name: Some(tc.function.name),
                                function_arguments: Some(tc.function.arguments),
                                thought_signature: tc.thought_signature,
                            })
                            .collect();

                        let fr = if finish_reason != "stop" || !tc_deltas.is_empty() {
                            Some(finish_reason)
                        } else if deltas.is_empty() && tc_deltas.is_empty() && usage.is_none() {
                            continue;
                        } else {
                            None
                        };

                        let event = ChatStreamEvent {
                            deltas,
                            finish_reason: fr,
                            tool_calls: tc_deltas,
                            usage,
                        };

                        if tx.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::NetworkError(e.to_string()))).await;
                        return;
                    }
                }
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn get_context_window(&self, model: &str) -> Result<u32, LlmError> {
        let model_path = if model.starts_with("models/") {
            model.to_string()
        } else {
            format!("models/{model}")
        };
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/{}?key={}",
            model_path, self.api_key
        );

        let http = reqwest::Client::new();
        let resp = http
            .get(&url)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !resp.status().is_success() {
            return Ok(0);
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        let ctx = body
            .get("inputTokenLimit")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        Ok(ctx)
    }
}
