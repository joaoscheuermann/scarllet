//! Default chat agent binary.
//!
//! Uses [`scarllet_sdk::agent::AgentSession`] to dial core, read the
//! per-turn provider + history + tool registry, and drive the LLM ↔
//! tool loop. All node writes (Thought / Tool / Result / TokenUsage /
//! Error) go through the SDK helpers so the agent never reaches into
//! the raw proto surface.

use std::time::Instant;

use scarllet_llm::types::{
    ChatMessage, ChatRequest, ChatStreamEvent, FunctionCall, FunctionDefinition, Role,
    StreamDelta, ToolCall, ToolCallDelta, ToolDefinition, Usage,
};
use scarllet_llm::LlmClient;
use scarllet_sdk::agent::{AgentSdkError, AgentSession, ToolStatus};
use scarllet_sdk::proto::{ActiveProviderResponse, AgentTask, HistoryEntry, ToolInfo};
use tokio_stream::StreamExt;

/// Prints the agent manifest JSON to stdout for Core auto-discovery.
fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "default",
        "kind": "agent",
        "version": "0.1.0",
        "description": "Default chat agent — answers questions using an LLM with tool support"
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

/// Wire string used by `ActiveProviderResponse` to identify the Gemini
/// provider type (matches `ProviderType::Gemini`).
const PROVIDER_TYPE_GEMINI: &str = "gemini";

/// Maximum number of LLM ↔ tool iterations per turn before the agent
/// short-circuits with a failure. Guards against runaway tool loops if the
/// model keeps requesting calls without ever producing a final answer.
const MAX_TOOL_ITERATIONS: usize = 24;

/// Maximum number of arguments-preview characters surfaced on the Tool
/// node's header. Long JSON inputs are truncated for display only — the
/// full `arguments_json` still rides through `InvokeTool`.
const ARGS_PREVIEW_CHARS: usize = 40;

/// Entry point for the default agent process.
///
/// When invoked with `--manifest`, prints the agent descriptor and exits.
/// Otherwise opens an `AgentStream` to the core orchestrator, fetches the
/// active provider snapshot + history + tool registry, and drives a full
/// LLM ↔ tool loop: each LLM completion either emits a `Result` (turn
/// done) or a batch of tool calls — every tool call materialises as a
/// `Tool` node, runs through `InvokeTool`, and feeds back into the LLM via
/// the local in-turn `ChatMessage` history.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return Ok(());
    }

    tracing_subscriber::fmt::init();

    let mut session = AgentSession::connect().await?;
    let _ = session
        .emit_debug(
            "info",
            &format!(
                "Default agent connected: session={} agent={}",
                session.session_id, session.agent_id
            ),
        )
        .await;

    let Some(task) = session.next_task().await else {
        let _ = session
            .emit_debug("warn", "Agent stream closed before AgentTask arrived; exiting")
            .await;
        return Ok(());
    };

    if let Err(err) = run_turn(&mut session, &task).await {
        let _ = session.emit_debug("error", &format!("Turn failed: {err}")).await;
        let _ = session.emit_error(&err.to_string()).await;
        let _ = session.emit_failure(&err.to_string()).await;
    }

    Ok(())
}

/// Runs a single LLM turn: fetches provider + tools + history, then loops
/// LLM completions and tool invocations until the model produces a Result
/// (no tool_calls). On the no-tool branch emits the final `Result` +
/// `TurnFinished` so the per-turn process can exit cleanly.
async fn run_turn(session: &mut AgentSession, task: &AgentTask) -> Result<(), TurnError> {
    let provider = session.get_provider().await?;
    if !provider.configured {
        return Err(TurnError::Other(
            "no LLM provider configured in scarllet config".to_string(),
        ));
    }

    let tools = session.get_tools().await?;
    let history = session.get_history().await?;
    let llm = build_llm_client(&provider);
    let tool_definitions = tools_to_definitions(&tools);
    let system_prompt = build_system_prompt(&tools);
    let agent_node_id = session.agent_node_id.clone();

    let mut messages = build_initial_messages(&system_prompt, &history, &task.prompt);

    session
        .emit_debug(
            "info",
            &format!("Using provider type: {}", provider.provider_type),
        )
        .await?;
    session
        .emit_debug(
            "info",
            &format!("Tools available: {}", format_tool_names(&tools)),
        )
        .await?;
    session
        .emit_debug(
            "info",
            &format!(
                "Agent '{}' starting LLM turn (model={}, history_len={}, tools={})",
                task.agent_id,
                provider.model,
                history.len(),
                tool_definitions.len()
            ),
        )
        .await?;

    // Fetch the model's context window once per turn so TokenUsage nodes
    // emitted below carry the correct denominator. Failures are surfaced
    // as a Debug node but don't abort the turn — the UI falls back to a
    // 0-window display.
    let context_window = match llm.get_context_window(&provider.model).await {
        Ok(v) => v,
        Err(err) => {
            let _ = session
                .emit_debug(
                    "warn",
                    &format!("get_context_window failed: {err}; falling back to 0"),
                )
                .await;
            0
        }
    };

    let mut last_usage: Option<Usage> = None;
    let mut iteration = 0usize;
    loop {
        iteration += 1;
        if iteration > MAX_TOOL_ITERATIONS {
            return Err(TurnError::Other(format!(
                "exceeded {MAX_TOOL_ITERATIONS} LLM ↔ tool iterations without a final result"
            )));
        }

        let request = build_chat_request(&provider, &messages, &tool_definitions);
        let stream_outcome = stream_completion(session, &llm, request, &agent_node_id).await?;
        if let Some(u) = stream_outcome.usage.clone() {
            last_usage = Some(u);
        }

        session
            .emit_debug(
                "info",
                &format!(
                    "Stream ended: finish_reason={}, tool_calls={}",
                    stream_outcome.finish_reason,
                    stream_outcome.tool_calls.len()
                ),
            )
            .await?;

        if stream_outcome.tool_calls.is_empty() {
            let reason = if stream_outcome.finish_reason.is_empty() {
                "stop".to_string()
            } else {
                stream_outcome.finish_reason
            };
            if let Some(u) = last_usage.as_ref() {
                session
                    .emit_token_usage(u.total_tokens, context_window)
                    .await?;
            }
            session.emit_result(&stream_outcome.content, &reason).await?;
            session
                .emit_debug(
                    "info",
                    &format!(
                        "Agent '{}' finished turn (iterations={iteration}, finish_reason={reason}, content_len={})",
                        task.agent_id,
                        stream_outcome.content.len()
                    ),
                )
                .await?;
            return Ok(());
        }

        messages.push(ChatMessage {
            role: Role::Assistant,
            content: stream_outcome.content.clone(),
            tool_calls: Some(stream_outcome.tool_calls.clone()),
            tool_call_id: None,
        });

        for call in &stream_outcome.tool_calls {
            let result = run_tool_call(session, &agent_node_id, call, &task.working_directory).await?;
            messages.push(ChatMessage {
                role: Role::Tool,
                content: result,
                tool_calls: None,
                tool_call_id: Some(call.id.clone()),
            });
        }
    }
}

/// Streams a single LLM completion, growing a `Thought` node from each
/// content delta and accumulating any tool-call deltas into complete
/// [`ToolCall`] values. Returns the aggregated content, tool calls, and
/// finish reason.
async fn stream_completion(
    session: &AgentSession,
    llm: &LlmClient,
    request: ChatRequest,
    agent_node_id: &str,
) -> Result<StreamOutcome, TurnError> {
    let mut stream = match llm.chat_stream(request).await {
        Ok(s) => s,
        Err(e) => {
            let msg = e.to_string();
            // Belt-and-suspenders (effort 07): write an Error node now so
            // TUIs see the failure even if this process dies before core
            // notices the stream close.
            let _ = session.emit_error(&msg).await;
            return Err(TurnError::Llm(msg));
        }
    };

    let mut thought_id: Option<String> = None;
    let mut content = String::new();
    let mut accumulated: Vec<AccumulatedCall> = Vec::new();
    let mut finish_reason = String::new();
    let mut last_usage: Option<Usage> = None;

    while let Some(event) = stream.next().await {
        let evt = match event {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                // Belt-and-suspenders (effort 07): write an Error node now so
                // TUIs see the failure even if this process dies before core
                // notices the stream close. Ignore the SDK error here —
                // effort 06's Paused path still handles the disconnect.
                let _ = session.emit_error(&msg).await;
                return Err(TurnError::Llm(msg));
            }
        };
        let ChatStreamEvent {
            deltas,
            finish_reason: fr,
            tool_calls,
            usage,
        } = evt;

        if let Some(u) = usage {
            last_usage = Some(u);
        }

        if !deltas.is_empty() && thought_id.is_none() {
            let id = session.create_thought(agent_node_id).await?;
            thought_id = Some(id);
        }

        if let Some(id) = thought_id.as_deref() {
            for delta in &deltas {
                let chunk = match delta {
                    StreamDelta::Thought(t) | StreamDelta::Content(t) => t.as_str(),
                };
                if chunk.is_empty() {
                    continue;
                }
                session.append_thought(id, chunk).await?;
                if matches!(delta, StreamDelta::Content(_)) {
                    content.push_str(chunk);
                }
            }
        }

        if !tool_calls.is_empty() {
            accumulate_tool_call_deltas(&mut accumulated, &tool_calls);
        }

        if let Some(reason) = fr {
            finish_reason = reason;
        }
    }

    Ok(StreamOutcome {
        content,
        tool_calls: finalize_tool_calls(accumulated),
        finish_reason,
        usage: last_usage,
    })
}

/// Creates the per-call `Tool` node, marks it `running`, dispatches the
/// invocation through `InvokeTool`, and patches the node with the final
/// status / duration / result. Returns the textual payload to feed back
/// into the LLM (output JSON on success, error message on failure).
async fn run_tool_call(
    session: &mut AgentSession,
    agent_node_id: &str,
    call: &ToolCall,
    working_directory: &str,
) -> Result<String, TurnError> {
    let preview = truncate_preview(&call.function.arguments, ARGS_PREVIEW_CHARS);
    let args_json = inject_working_directory(&call.function.arguments, working_directory);

    let tool_node_id = session
        .create_tool(agent_node_id, &call.function.name, &preview, &args_json)
        .await?;
    session
        .update_tool_status(&tool_node_id, ToolStatus::Running, 0, "")
        .await?;

    let start = Instant::now();
    let resp = session.invoke_tool(&call.function.name, &args_json).await?;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (status, payload) = if resp.success {
        (ToolStatus::Done, resp.output_json)
    } else {
        (ToolStatus::Failed, resp.error_message)
    };
    session
        .update_tool_status(&tool_node_id, status, duration_ms, &payload)
        .await?;

    session
        .emit_debug(
            "info",
            &format!(
                "Tool '{}' finished in {duration_ms}ms (status={status})",
                call.function.name
            ),
        )
        .await?;
    Ok(payload)
}

/// Builds an [`LlmClient`] from the provider snapshot core returned for
/// this session.
fn build_llm_client(provider: &ActiveProviderResponse) -> LlmClient {
    if provider.provider_type == PROVIDER_TYPE_GEMINI {
        return LlmClient::new_gemini(provider.api_key.clone());
    }
    LlmClient::new_openai(provider.api_url.clone(), provider.api_key.clone())
}

/// Builds the initial [`ChatMessage`] vector used to seed the local LLM
/// loop: a system prompt, then the prior conversation derived by core,
/// then the new user prompt.
fn build_initial_messages(
    system_prompt: &str,
    history: &[HistoryEntry],
    new_prompt: &str,
) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(history.len() + 2);
    messages.push(ChatMessage {
        role: Role::System,
        content: system_prompt.to_string(),
        tool_calls: None,
        tool_call_id: None,
    });
    for entry in history {
        messages.push(history_entry_to_chat_message(entry));
    }
    messages.push(ChatMessage {
        role: Role::User,
        content: new_prompt.to_string(),
        tool_calls: None,
        tool_call_id: None,
    });
    messages
}

/// Builds the per-iteration [`ChatRequest`] sent to the LLM, attaching the
/// available [`ToolDefinition`] list so the model can request tool calls.
fn build_chat_request(
    provider: &ActiveProviderResponse,
    messages: &[ChatMessage],
    tool_definitions: &[ToolDefinition],
) -> ChatRequest {
    let reasoning_effort = if provider.reasoning_effort.is_empty() {
        None
    } else {
        Some(provider.reasoning_effort.clone())
    };
    let tools = if tool_definitions.is_empty() {
        None
    } else {
        Some(tool_definitions.to_vec())
    };
    ChatRequest {
        model: provider.model.clone(),
        messages: messages.to_vec(),
        temperature: None,
        max_tokens: None,
        reasoning_effort,
        extra_body: None,
        tools,
    }
}

/// Converts a single [`HistoryEntry`] into a [`ChatMessage`]. Tool call
/// entries (`role == "assistant"` with `tool_calls_json`) are parsed into
/// `Vec<ToolCall>`; tool result entries (`role == "tool"`) carry their
/// originating `tool_call_id` so the LLM can correlate.
fn history_entry_to_chat_message(entry: &HistoryEntry) -> ChatMessage {
    let role = history_role(&entry.role);
    let tool_calls = entry
        .tool_calls_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<ToolCall>>(raw).ok());
    let tool_call_id = entry.tool_call_id.clone();
    ChatMessage {
        role,
        content: entry.content.clone(),
        tool_calls,
        tool_call_id,
    }
}

/// Maps the wire-level role string (`"user"` / `"assistant"` / …) onto
/// the LLM client's [`Role`] enum. Unknown values default to assistant.
fn history_role(role: &str) -> Role {
    match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "tool" => Role::Tool,
        _ => Role::Assistant,
    }
}

/// Converts the core `ToolInfo` records into LLM-compatible
/// [`ToolDefinition`] values consumable by `LlmProvider`.
fn tools_to_definitions(tools: &[ToolInfo]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|t| {
            let parameters: serde_json::Value = if t.input_schema_json.is_empty() {
                serde_json::json!({"type": "object", "properties": {}})
            } else {
                serde_json::from_str(&t.input_schema_json)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}))
            };
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters,
                },
            }
        })
        .collect()
}

/// Returns the system prompt that primes every turn. Lists every
/// available tool so the model knows what it can call without re-deriving
/// from the function-calling schema alone.
fn build_system_prompt(tools: &[ToolInfo]) -> String {
    let mut prompt = format!("Current operating system: {}\n\n", std::env::consts::OS);
    if !tools.is_empty() {
        prompt.push_str("You have access to the following tools:\n");
        for tool in tools {
            prompt.push_str(&format!("- {}: {}\n", tool.name, tool.description));
        }
        prompt.push('\n');
    }
    prompt.push_str(
        "You are Scarllet's default chat agent. Respond to the user concisely and accurately. \
         Use the available tools when they help; otherwise answer directly from the prior \
         conversation context.",
    );
    prompt
}

/// Truncates `s` to at most `max` chars, appending `…` when shortened.
fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

/// Injects the agent task's `working_directory` into the tool arguments
/// JSON when the model omitted it (preserves the convenience that lets
/// tools default their cwd to the session's). Returns the original string
/// unchanged when the JSON cannot be parsed as an object.
fn inject_working_directory(arguments_json: &str, working_directory: &str) -> String {
    if working_directory.is_empty() {
        return arguments_json.to_string();
    }
    let mut value: serde_json::Value = match serde_json::from_str(arguments_json) {
        Ok(v) => v,
        Err(_) => return arguments_json.to_string(),
    };
    let serde_json::Value::Object(ref mut map) = value else {
        return arguments_json.to_string();
    };
    if !map.contains_key("working_directory") {
        map.insert(
            "working_directory".to_string(),
            serde_json::Value::String(working_directory.to_string()),
        );
    }
    serde_json::to_string(&value).unwrap_or_else(|_| arguments_json.to_string())
}

/// Per-iteration accumulator for the `(id, name, arguments,
/// thought_signature)` of one tool call as the LLM streams its deltas.
#[derive(Default)]
struct AccumulatedCall {
    id: String,
    name: String,
    arguments: String,
    thought_signature: Option<String>,
}

/// Merges streaming [`ToolCallDelta`]s into per-index accumulators. The
/// LLM emits id / name once and arguments incrementally, so each field is
/// either set-on-first or appended.
fn accumulate_tool_call_deltas(accumulated: &mut Vec<AccumulatedCall>, deltas: &[ToolCallDelta]) {
    for delta in deltas {
        while accumulated.len() <= delta.index {
            accumulated.push(AccumulatedCall::default());
        }
        let entry = &mut accumulated[delta.index];
        if let Some(id) = delta.id.as_ref() {
            if entry.id.is_empty() {
                entry.id = id.clone();
            }
        }
        if let Some(name) = delta.function_name.as_ref() {
            entry.name.push_str(name);
        }
        if let Some(args) = delta.function_arguments.as_ref() {
            entry.arguments.push_str(args);
        }
        if delta.thought_signature.is_some() {
            entry.thought_signature.clone_from(&delta.thought_signature);
        }
    }
}

/// Finalises accumulated tool-call fragments into [`ToolCall`] values.
fn finalize_tool_calls(accumulated: Vec<AccumulatedCall>) -> Vec<ToolCall> {
    accumulated
        .into_iter()
        .map(|c| ToolCall {
            id: c.id,
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: c.name,
                arguments: c.arguments,
            },
            thought_signature: c.thought_signature,
        })
        .collect()
}

/// Outcome of one LLM completion stream, ready to drive the loop's next
/// branch.
struct StreamOutcome {
    /// User-visible text accumulated from `StreamDelta::Content` chunks.
    content: String,
    /// Fully-assembled tool calls from `tool_calls` deltas.
    tool_calls: Vec<ToolCall>,
    /// Last `finish_reason` reported by the stream.
    finish_reason: String,
    /// Most recent [`Usage`] reported by the provider during this stream,
    /// if any. Emitted as a `TokenUsage` node at turn end.
    usage: Option<Usage>,
}

/// Formats the tool-registry response as `[name, name, …]` for the
/// `Tools available: …` debug node emitted at turn start.
fn format_tool_names(tools: &[ToolInfo]) -> String {
    let joined = tools
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{joined}]")
}

/// Local error type that distinguishes SDK failures from LLM / config
/// failures so the call site can pick the right `emit_failure` message.
#[derive(Debug)]
enum TurnError {
    Sdk(AgentSdkError),
    Llm(String),
    Other(String),
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sdk(e) => write!(f, "agent SDK error: {e}"),
            Self::Llm(msg) => write!(f, "LLM error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TurnError {}

impl From<AgentSdkError> for TurnError {
    fn from(e: AgentSdkError) -> Self {
        Self::Sdk(e)
    }
}

#[cfg(test)]
mod tests;
