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
