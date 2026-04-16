use scarllet_proto::proto::{
    agent_instruction, core_event, AgentBlock, AgentErrorEvent, AgentHistorySync, AgentInstruction,
    AgentResponseEvent, AgentStartedEvent, AgentTask, AgentThinkingEvent, AgentToolCallEvent,
    ConnectedEvent, CoreEvent, DebugLogEvent, HistoryEntry, ProviderInfoEvent, SystemEvent,
    TokenUsageEvent,
};
use scarllet_sdk::config::ScarlletConfig;

/// Builds a `CoreEvent::ProviderInfo` from the current configuration.
///
/// Returns an event with empty fields when no active provider is configured.
pub(crate) fn provider_info(cfg: &ScarlletConfig) -> CoreEvent {
    let (provider_name, model, reasoning_effort) = match cfg.active_provider() {
        Some(p) => (
            p.name.clone(),
            p.model.clone(),
            p.reasoning_effort().unwrap_or_default().to_string(),
        ),
        None => (String::new(), String::new(), String::new()),
    };

    wrap_core(core_event::Payload::ProviderInfo(ProviderInfoEvent {
        provider_name,
        model,
        reasoning_effort,
    }))
}

/// Wraps a `core_event::Payload` into the outer `CoreEvent` envelope.
fn wrap_core(payload: core_event::Payload) -> CoreEvent {
    CoreEvent {
        payload: Some(payload),
    }
}

/// Connected handshake event carrying the daemon uptime.
pub(crate) fn connected(uptime_secs: u64) -> CoreEvent {
    wrap_core(core_event::Payload::Connected(ConnectedEvent {
        uptime_secs,
    }))
}

/// Signals that the core has accepted a task and bound it to an agent.
pub(crate) fn agent_started(task_id: String, agent_name: String) -> CoreEvent {
    wrap_core(core_event::Payload::AgentStarted(AgentStartedEvent {
        task_id,
        agent_name,
    }))
}

/// Streams an in-progress reasoning/content update for an agent task.
pub(crate) fn agent_thinking(
    task_id: String,
    agent_name: String,
    blocks: Vec<AgentBlock>,
) -> CoreEvent {
    wrap_core(core_event::Payload::AgentThinking(AgentThinkingEvent {
        task_id,
        agent_name,
        blocks,
    }))
}

/// Streams the final (or completed) response blocks for an agent task.
pub(crate) fn agent_response(
    task_id: String,
    agent_name: String,
    blocks: Vec<AgentBlock>,
) -> CoreEvent {
    wrap_core(core_event::Payload::AgentResponse(AgentResponseEvent {
        task_id,
        agent_name,
        blocks,
    }))
}

/// Signals that an agent task has failed with the given error message.
pub(crate) fn agent_error(task_id: String, agent_name: String, error: String) -> CoreEvent {
    wrap_core(core_event::Payload::AgentError(AgentErrorEvent {
        task_id,
        agent_name,
        error,
    }))
}

/// Tool-call lifecycle update (running / done / failed) for a given invocation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_tool_call(
    task_id: String,
    agent_name: String,
    call_id: String,
    tool_name: String,
    arguments_preview: String,
    status: String,
    duration_ms: u64,
    result: String,
) -> CoreEvent {
    wrap_core(core_event::Payload::AgentToolCall(AgentToolCallEvent {
        task_id,
        agent_name,
        call_id,
        tool_name,
        arguments_preview,
        status,
        duration_ms,
        result,
    }))
}

/// Forwards a structured debug log entry to TUI subscribers.
pub(crate) fn debug_log(
    source: String,
    level: String,
    message: String,
    timestamp_ms: u64,
) -> CoreEvent {
    wrap_core(core_event::Payload::DebugLog(DebugLogEvent {
        source,
        level,
        message,
        timestamp_ms,
    }))
}

/// System-level notification (connection status, provider errors, etc).
pub(crate) fn system(message: String) -> CoreEvent {
    wrap_core(core_event::Payload::System(SystemEvent { message }))
}

/// Token usage update reported by the active agent.
pub(crate) fn token_usage(total_tokens: u32, context_window: u32) -> CoreEvent {
    wrap_core(core_event::Payload::TokenUsage(TokenUsageEvent {
        total_tokens,
        context_window,
    }))
}

/// Dispatches a task to a connected agent via `AgentStream`.
pub(crate) fn task_instruction(
    task_id: String,
    prompt: String,
    working_directory: String,
) -> AgentInstruction {
    AgentInstruction {
        payload: Some(agent_instruction::Payload::Task(AgentTask {
            task_id,
            prompt,
            working_directory,
        })),
    }
}

/// Seeds a newly registered agent with the current conversation transcript.
pub(crate) fn history_instruction(messages: Vec<HistoryEntry>) -> AgentInstruction {
    AgentInstruction {
        payload: Some(agent_instruction::Payload::HistorySync(AgentHistorySync {
            messages,
        })),
    }
}
