use scarllet_llm::types::{
    ChatMessage, ChatRequest, ChatStreamEvent, FunctionDefinition, Role, StreamDelta, ToolCall,
    ToolCallDelta, ToolDefinition,
};
use scarllet_llm::LlmClient;
use scarllet_proto::proto::agent_message;
use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::*;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tracing::info;

/// Sends a debug log event to the Core for display in the TUI debug panel.
async fn debug_log(
    client: &mut OrchestratorClient<tonic::transport::Channel>,
    level: &str,
    message: &str,
) {
    let _ = client
        .emit_debug_log(DebugLogRequest {
            source: "default-agent".into(),
            level: level.into(),
            message: message.into(),
        })
        .await;
}

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

/// Assembles the system prompt with OS info and available tool descriptions.
fn build_system_prompt(tools: &[ToolInfo]) -> String {
    let mut prompt = format!("Operating system: {}\n\n", std::env::consts::OS);

    if !tools.is_empty() {
        prompt.push_str("You have access to the following tools:\n");
        for tool in tools {
            prompt.push_str(&format!("- {}: {}\n", tool.name, tool.description));
        }
        prompt.push('\n');
    }

    prompt.push_str("Achieve the goal of this session.");
    prompt
}

/// Converts Core `ToolInfo` records into LLM-compatible `ToolDefinition` values.
fn tools_to_definitions(tools: &[ToolInfo]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|t| {
            let parameters: serde_json::Value = if t.input_schema_json.is_empty() {
                serde_json::json!({"type": "object", "properties": {}})
            } else {
                serde_json::from_str(&t.input_schema_json)
                    .unwrap_or(serde_json::json!({"type": "object", "properties": {}}))
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

/// Truncates a string to `max` characters, appending `"..."` if shortened.
fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}...")
    }
}

/// Accumulates streaming tool call deltas into complete ToolCall objects.
fn accumulate_tool_calls(
    accumulated: &mut Vec<(String, String, String, Option<String>)>,
    deltas: &[ToolCallDelta],
) {
    for delta in deltas {
        while accumulated.len() <= delta.index {
            accumulated.push((String::new(), String::new(), String::new(), None));
        }
        let entry = &mut accumulated[delta.index];
        if let Some(ref id) = delta.id {
            entry.0.clone_from(id);
        }
        if let Some(ref name) = delta.function_name {
            entry.1.push_str(name);
        }
        if let Some(ref args) = delta.function_arguments {
            entry.2.push_str(args);
        }
        if delta.thought_signature.is_some() {
            entry.3.clone_from(&delta.thought_signature);
        }
    }
}

/// Converts accumulated `(id, name, arguments, thought_signature)` tuples into `ToolCall` values.
fn finalize_tool_calls(
    accumulated: Vec<(String, String, String, Option<String>)>,
) -> Vec<ToolCall> {
    accumulated
        .into_iter()
        .map(|(id, name, arguments, thought_signature)| ToolCall {
            id,
            tool_type: "function".to_string(),
            function: scarllet_llm::types::FunctionCall { name, arguments },
            thought_signature,
        })
        .collect()
}

/// Merges streaming thought/content deltas into the running list of `AgentBlock`s.
fn accumulate_stream_deltas(blocks: &mut Vec<AgentBlock>, deltas: &[StreamDelta]) {
    for delta in deltas {
        let (bt, text) = match delta {
            StreamDelta::Thought(t) => ("thought", t.as_str()),
            StreamDelta::Content(t) => ("text", t.as_str()),
        };
        if text.is_empty() {
            continue;
        }
        match blocks.last_mut() {
            Some(b) if b.block_type == bt => b.content.push_str(text),
            _ => blocks.push(AgentBlock {
                block_type: bt.into(),
                content: text.to_string(),
            }),
        }
    }
}

/// Borrowed references passed through the tool-loop to avoid threading individual parameters.
struct AgentContext<'a> {
    llm: &'a LlmClient,
    client: &'a mut OrchestratorClient<tonic::transport::Channel>,
    msg_tx: &'a tokio::sync::mpsc::Sender<AgentMessage>,
    task: &'a AgentTask,
    tool_definitions: &'a [ToolDefinition],
    system_prompt: &'a str,
    model: &'a str,
    reasoning_effort: &'a Option<String>,
    context_window: u32,
}

/// Extracts the concatenated text content from agent blocks, ignoring thoughts.
fn blocks_to_content(blocks: &[AgentBlock]) -> String {
    blocks
        .iter()
        .filter(|b| b.block_type == "text")
        .map(|b| b.content.as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// Entry point for the default agent process.
///
/// When invoked with `--manifest`, prints the agent descriptor and exits.
/// Otherwise connects to the Core orchestrator, registers as `"default"`,
/// and processes incoming tasks in a loop using the configured LLM provider.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return Ok(());
    }

    tracing_subscriber::fmt::init();

    let core_addr =
        std::env::var("SCARLLET_CORE_ADDR").unwrap_or_else(|_| "127.0.0.1:50051".to_string());

    info!("Default agent starting, connecting to Core at {core_addr}");

    let endpoint = format!("http://{core_addr}");
    let mut client = OrchestratorClient::connect(endpoint)
        .await?
        .max_decoding_message_size(64 * 1024 * 1024)
        .max_encoding_message_size(64 * 1024 * 1024);

    let (msg_tx, msg_rx) = tokio::sync::mpsc::channel::<AgentMessage>(64);
    let outgoing = ReceiverStream::new(msg_rx);

    let response = client.agent_stream(outgoing).await?;
    let mut task_stream = response.into_inner();

    let register = AgentMessage {
        payload: Some(agent_message::Payload::Register(AgentRegister {
            agent_name: "default".into(),
        })),
    };
    msg_tx.send(register).await?;
    info!("Registered as 'default' agent");

    let mut history: Vec<ChatMessage> = Vec::new();

    while let Some(task) = task_stream.message().await? {
        info!(
            "Received task {}: {}",
            task.task_id,
            &task.prompt[..task.prompt.len().min(50)]
        );

        let provider_resp = client
            .get_active_provider(ActiveProviderQuery {})
            .await
            .map_err(|e| format!("GetActiveProvider RPC failed: {e}"))?
            .into_inner();

        if !provider_resp.configured {
            let failure = AgentMessage {
                payload: Some(agent_message::Payload::Failure(AgentFailure {
                    task_id: task.task_id,
                    error: "No provider configured.".into(),
                })),
            };
            let _ = msg_tx.send(failure).await;
            continue;
        }

        let tool_registry_resp = client
            .get_tool_registry(ToolRegistryQuery {})
            .await
            .map_err(|e| format!("GetToolRegistry RPC failed: {e}"))?
            .into_inner();

        let available_tools = tool_registry_resp.tools;
        let tool_names: Vec<&str> = available_tools.iter().map(|t| t.name.as_str()).collect();
        debug_log(
            &mut client,
            "debug",
            &format!("Tools available: {:?}", tool_names),
        )
        .await;

        let tool_definitions = tools_to_definitions(&available_tools);
        let system_prompt = build_system_prompt(&available_tools);

        let llm = match provider_resp.provider_type.as_str() {
            "gemini" => LlmClient::new_gemini(provider_resp.api_key.clone()),
            _ => LlmClient::new_openai(provider_resp.api_url.clone(), provider_resp.api_key.clone()),
        };

        debug_log(
            &mut client,
            "debug",
            &format!("Using provider type: {}", provider_resp.provider_type),
        )
        .await;

        let context_window = llm
            .get_context_window(&provider_resp.model)
            .await
            .unwrap_or(0);

        debug_log(
            &mut client,
            "debug",
            &format!("Context window for {}: {context_window}", provider_resp.model),
        )
        .await;

        history.push(ChatMessage {
            role: Role::User,
            content: task.prompt.clone(),
            tool_calls: None,
            tool_call_id: None,
        });

        let reasoning_effort = if provider_resp.reasoning_effort.is_empty() {
            None
        } else {
            Some(provider_resp.reasoning_effort.clone())
        };

        let mut blocks: Vec<AgentBlock> = Vec::new();

        let mut ctx = AgentContext {
            llm: &llm,
            client: &mut client,
            msg_tx: &msg_tx,
            task: &task,
            tool_definitions: &tool_definitions,
            system_prompt: &system_prompt,
            model: &provider_resp.model,
            reasoning_effort: &reasoning_effort,
            context_window,
        };

        if let Err(e) = run_tool_loop(&mut ctx, &mut history, &mut blocks).await {
            let failure = AgentMessage {
                payload: Some(agent_message::Payload::Failure(AgentFailure {
                    task_id: task.task_id.clone(),
                    error: e.to_string(),
                })),
            };
            let _ = msg_tx.send(failure).await;
        }
    }

    info!("Task stream closed, agent exiting");
    Ok(())
}

/// Runs the LLM ↔ tool-call loop until the model produces a final response.
///
/// Each iteration streams an LLM completion, executes any requested tool calls
/// through the Core, appends results to history, and loops back. The loop
/// terminates when the model finishes without requesting more tool calls.
async fn run_tool_loop(
    ctx: &mut AgentContext<'_>,
    history: &mut Vec<ChatMessage>,
    blocks: &mut Vec<AgentBlock>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let mut messages = vec![ChatMessage {
            role: Role::System,
            content: ctx.system_prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];
        messages.extend(history.clone());

        let tools = if ctx.tool_definitions.is_empty() {
            None
        } else {
            Some(ctx.tool_definitions.to_vec())
        };

        let request = ChatRequest {
            model: ctx.model.to_string(),
            messages,
            temperature: None,
            max_tokens: None,
            reasoning_effort: ctx.reasoning_effort.clone(),
            extra_body: None,
            tools,
        };

        debug_log(ctx.client, "debug", "Sending chat request to LLM").await;

        let mut stream = match ctx.llm.chat_stream(request).await {
            Ok(s) => s,
            Err(e) => {
                debug_log(
                    ctx.client,
                    "error",
                    &format!("LLM chat_stream failed: {e}"),
                )
                .await;
                return Err(e.into());
            }
        };
        let mut accumulated_tool_calls: Vec<(String, String, String, Option<String>)> = Vec::new();
        let mut finish_reason = String::new();
        let mut last_usage: Option<scarllet_llm::types::Usage> = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(ChatStreamEvent {
                    deltas,
                    finish_reason: fr,
                    tool_calls,
                    usage,
                }) => {
                    accumulate_stream_deltas(blocks, &deltas);

                    if !tool_calls.is_empty() {
                        accumulate_tool_calls(&mut accumulated_tool_calls, &tool_calls);
                    }

                    if let Some(reason) = fr {
                        finish_reason = reason;
                    }

                    if usage.is_some() {
                        last_usage = usage;
                    }

                    let progress = AgentMessage {
                        payload: Some(agent_message::Payload::Progress(AgentProgressMsg {
                            task_id: ctx.task.task_id.clone(),
                            blocks: blocks.clone(),
                        })),
                    };
                    let _ = ctx.msg_tx.send(progress).await;
                }
                Err(e) => {
                    debug_log(
                        ctx.client,
                        "error",
                        &format!("Stream chunk error: {e}"),
                    )
                    .await;
                    return Err(e.into());
                }
            }
        }

        if let Some(ref usage) = last_usage {
            let token_msg = AgentMessage {
                payload: Some(agent_message::Payload::TokenUsage(AgentTokenUsageMsg {
                    task_id: ctx.task.task_id.clone(),
                    total_tokens: usage.total_tokens,
                    context_window: ctx.context_window,
                })),
            };
            let _ = ctx.msg_tx.send(token_msg).await;
        }

        debug_log(
            ctx.client,
            "debug",
            &format!(
                "Stream ended: finish_reason=\"{finish_reason}\", tool_calls_accumulated={}",
                accumulated_tool_calls.len()
            ),
        )
        .await;

        if accumulated_tool_calls.is_empty() {
            history.push(ChatMessage {
                role: Role::Assistant,
                content: blocks_to_content(blocks),
                tool_calls: None,
                tool_call_id: None,
            });

            let result = AgentMessage {
                payload: Some(agent_message::Payload::Result(AgentResultMsg {
                    task_id: ctx.task.task_id.clone(),
                    blocks: blocks.clone(),
                })),
            };
            let _ = ctx.msg_tx.send(result).await;
            return Ok(());
        }

        let tool_calls = finalize_tool_calls(accumulated_tool_calls);

        debug_log(
            ctx.client,
            "debug",
            &format!(
                "Executing {} tool call(s): {:?}",
                tool_calls.len(),
                tool_calls.iter().map(|tc| &tc.function.name).collect::<Vec<_>>()
            ),
        )
        .await;

        history.push(ChatMessage {
            role: Role::Assistant,
            content: blocks_to_content(blocks),
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
        });

        for tc in &tool_calls {
            let preview = truncate_preview(&tc.function.arguments, 40);

            blocks.push(AgentBlock {
                block_type: "tool_call_ref".into(),
                content: tc.id.clone(),
            });

            let progress = AgentMessage {
                payload: Some(agent_message::Payload::Progress(AgentProgressMsg {
                    task_id: ctx.task.task_id.clone(),
                    blocks: blocks.clone(),
                })),
            };
            let _ = ctx.msg_tx.send(progress).await;

            let start_msg = AgentMessage {
                payload: Some(agent_message::Payload::ToolCall(AgentToolCallMsg {
                    task_id: ctx.task.task_id.clone(),
                    call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    arguments_preview: preview.clone(),
                    status: "running".into(),
                    duration_ms: 0,
                    result: String::new(),
                })),
            };
            let _ = ctx.msg_tx.send(start_msg).await;

            let invoke_start = std::time::Instant::now();

            let mut args: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::json!({}));

            if let serde_json::Value::Object(ref mut map) = args {
                if !map.contains_key("working_directory")
                    && !ctx.task.working_directory.is_empty()
                {
                    map.insert(
                        "working_directory".to_string(),
                        serde_json::Value::String(ctx.task.working_directory.clone()),
                    );
                }
            }

            let tool_result = ctx
                .client
                .invoke_tool(ToolInvocation {
                    tool_name: tc.function.name.clone(),
                    input_json: serde_json::to_string(&args).unwrap_or_default(),
                    snapshot_id: 0,
                })
                .await;

            let elapsed_ms = invoke_start.elapsed().as_millis() as u64;

            let (result_content, success) = match tool_result {
                Ok(resp) => {
                    let r = resp.into_inner();
                    if r.success {
                        (r.output_json, true)
                    } else {
                        (format!("Error: {}", r.error_message), false)
                    }
                }
                Err(e) => (format!("RPC error: {e}"), false),
            };

            let status = if success { "done" } else { "failed" };
            let done_msg = AgentMessage {
                payload: Some(agent_message::Payload::ToolCall(AgentToolCallMsg {
                    task_id: ctx.task.task_id.clone(),
                    call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    arguments_preview: preview,
                    status: status.into(),
                    duration_ms: elapsed_ms,
                    result: result_content.clone(),
                })),
            };
            let _ = ctx.msg_tx.send(done_msg).await;

            history.push(ChatMessage {
                role: Role::Tool,
                content: result_content,
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
            });
        }
    }
}
