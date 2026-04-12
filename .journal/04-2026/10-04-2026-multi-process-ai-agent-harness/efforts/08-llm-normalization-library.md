---
status: done
order: 8
created: 2026-04-10 19:48
title: "LLM normalization library with provider adapters"
---

## Description

Implement the `scarllet-llm` crate as the vendor-agnostic LLM abstraction library. It defines a normalized request/response model for chat completions (and streaming), retrieves credentials from Core via gRPC, and translates calls into provider-specific HTTP APIs. The first provider adapter is OpenAI-compatible (covers OpenAI, Azure OpenAI, and any OpenAI-compatible endpoint). Error bubbling (HTTP 429, 500, etc.) returns structured errors to the caller without retry — the agent decides retry policy.

## Objective

An agent binary can use `scarllet-llm` to make a vendor-agnostic `ChatCompletion` request. The library fetches the API key from Core, translates the request to the OpenAI HTTP API, streams the response back as normalized chunks, and surfaces HTTP errors as typed Rust errors.

## Implementation Details

1. **`scarllet-llm` core types:**
   - `ChatRequest { provider, model, messages: Vec<ChatMessage>, temperature, max_tokens, tools: Vec<ToolDefinition> }`.
   - `ChatMessage { role: Role, content: String, tool_calls: Option<Vec<ToolCall>> }`.
   - `Role` enum: `System`, `User`, `Assistant`, `Tool`.
   - `ChatResponse { message: ChatMessage, usage: Usage, finish_reason: FinishReason }`.
   - `ChatResponseStream` — `Stream<Item = Result<ChatResponseDelta, LlmError>>`.
   - `LlmError` enum: `Unauthorized`, `RateLimited { retry_after }`, `ServerError { status, body }`, `NetworkError`, `InvalidResponse`, `ProviderNotConfigured`.
2. **Provider trait:**
   ```rust
   #[async_trait]
   trait LlmProvider: Send + Sync {
       async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;
       async fn chat_stream(&self, request: ChatRequest) -> Result<ChatResponseStream, LlmError>;
   }
   ```
3. **OpenAI adapter:**
   - Implements `LlmProvider` for the OpenAI chat completions API.
   - Uses `reqwest` for HTTP. Supports streaming via SSE (server-sent events) parsing.
   - Maps HTTP 401 → `Unauthorized`, 429 → `RateLimited`, 5xx → `ServerError`.
   - Configurable `base_url` to support Azure OpenAI and compatible endpoints.
4. **Credential integration:**
   - `LlmClient` struct wraps a gRPC connection to Core.
   - Before each request: call `GetCredentials(provider)` from Core. Cache credentials in memory with a TTL to avoid per-request RPC overhead.
   - If credential not found: return `ProviderNotConfigured` immediately.
5. **`scarllet-llm` dependencies:** `scarllet-proto`, `scarllet-sdk`, `tonic`, `tokio`, `reqwest`, `serde`, `serde_json`, `async-trait`, `futures`.
6. **Test agent (extended):**
   - Extend `test-agent` (from Effort 6) or create `test-llm-agent` that uses `scarllet-llm` to make a chat completion request and logs the response.
   - Requires a real API key set via `SetCredential` (Effort 4) for integration testing.

## Verification Criteria

- Set an OpenAI credential via `SetCredential`. Register `test-llm-agent`.
- Submit a task with `test-llm-agent` → agent fetches credentials from Core, makes an OpenAI chat completion request, receives a response, logs the normalized output.
- Verify streaming: agent receives response chunks incrementally (observable in agent logs or TUI output).
- Remove the credential, submit again → agent receives `ProviderNotConfigured` error, reports failure gracefully.
- Set an invalid API key → agent receives `Unauthorized` error from the library.
- `npx nx run scarllet-llm:test` passes unit tests for request translation, response normalization, and error mapping (using mock HTTP responses).

## Done

- Agents can make vendor-agnostic LLM calls through `scarllet-llm`, with credentials managed centrally by Core.
- Observable via test-llm-agent making a real API call and receiving a normalized response through the full stack.
