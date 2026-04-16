use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers};

use scarllet_proto::proto::core_event;
use scarllet_proto::proto::tui_message;
use scarllet_proto::proto::*;

use crate::app::{
    total_block_chars, App, ChatEntry, DisplayBlock, Focus, Route, ToolCallData, ToolCallStatus,
};

/// Scrolls the view by one page in the given direction.
pub(crate) fn scroll_page(state: &mut crate::widgets::ScrollViewState, up: bool, page_height: u16) {
    if up {
        state.offset_y = state.offset_y.saturating_sub(page_height);
    } else {
        state.offset_y = state.offset_y.saturating_add(page_height);
    }
}

/// Inserts a string at the current cursor position in the input buffer.
fn insert_text_at_cursor(app: &mut App, text: &str) {
    app.input_state.insert_str(text);
}

/// Handles a bracketed-paste event by inserting cleaned text into the input.
pub(crate) fn handle_paste(app: &mut App, text: &str) {
    if app.focus != Focus::Input || app.input_locked {
        return;
    }
    if !matches!(app.route, Route::Chat) {
        return;
    }

    let cleaned: String = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    if !cleaned.is_empty() {
        insert_text_at_cursor(app, &cleaned);
    }
}

/// Finds the task ID of the most recent agent entry that is still streaming.
fn find_running_task_id(messages: &[ChatEntry]) -> Option<String> {
    messages.iter().rev().find_map(|e| match e {
        ChatEntry::Agent {
            task_id,
            done: false,
            ..
        } => Some(task_id.clone()),
        _ => None,
    })
}

/// Moves focus back to the input pane and clears the history selection.
fn return_to_input(app: &mut App) {
    app.focus = Focus::Input;
    app.focused_message_idx = None;
}

/// Switches focus to the history pane, selecting the latest message.
fn enter_history(app: &mut App) {
    if !app.messages.is_empty() {
        app.focus = Focus::History;
        app.focused_message_idx = Some(app.messages.len() - 1);
    }
}

/// Calculates the page-scroll increment as 1/4 of the viewport height.
fn scroll_page_increment(viewport_height: u16) -> u16 {
    (viewport_height / 4).max(1)
}

/// Processes a key event and returns `true` if the application should exit.
///
/// Routes keys to history navigation, cancel-streaming, input editing,
/// or scroll depending on current focus and application state.
pub(crate) fn handle_input(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    // Guard clause: Ctrl+C always exits
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    // Guard clause: Esc cancels streaming regardless of focus or route
    if key.code == KeyCode::Esc && app.is_streaming() {
        if let Some(task_id) = find_running_task_id(&app.messages) {
            let msg = TuiMessage {
                payload: Some(tui_message::Payload::Cancel(CancelPrompt { task_id })),
            };
            let _ = app.message_tx.try_send(msg);
        }
        return false;
    }

    // Guard clause: Most keys require Chat route
    if !matches!(app.route, Route::Chat) {
        return false;
    }

    // Handle History focus
    if app.focus == Focus::History {
        match key.code {
            KeyCode::Up => {
                if let Some(idx) = app.focused_message_idx {
                    if idx > 0 {
                        app.focused_message_idx = Some(idx - 1);
                    }
                }
            }
            KeyCode::Down => {
                if let Some(idx) = app.focused_message_idx {
                    if idx + 1 < app.messages.len() {
                        app.focused_message_idx = Some(idx + 1);
                    } else {
                        return_to_input(app);
                    }
                }
            }
            KeyCode::Esc => return_to_input(app),
            KeyCode::PageUp => scroll_page(&mut app.scroll_view_state, true, scroll_page_increment(app.history_viewport_height)),
            KeyCode::PageDown => scroll_page(&mut app.scroll_view_state, false, scroll_page_increment(app.history_viewport_height)),
            _ => {}
        }
        return false;
    }

    // Handle Input focus
    let input_editable = app.focus == Focus::Input && !app.input_locked;
    let has_shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let page = scroll_page_increment(app.history_viewport_height);

    if input_editable {
        match key.code {
            KeyCode::Enter if !has_shift && !has_ctrl => {
                let trimmed = app.input_state.text().trim().to_string();
                if trimmed.eq_ignore_ascii_case("exit") {
                    return true;
                }
                if !trimmed.is_empty() {
                    app.scroll_view_state.scroll_to_bottom();
                    app.push_message(ChatEntry::User { text: trimmed.clone() });
                    app.input_state.set_text(String::new());
                    app.save_session();
                    let msg = TuiMessage {
                        payload: Some(tui_message::Payload::Prompt(PromptMessage {
                            text: trimmed,
                            working_directory: std::env::current_dir()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_default(),
                        })),
                    };
                    if app.message_tx.try_send(msg).is_err() {
                        app.push_message(ChatEntry::System {
                            text: "Connection lost. Please restart the TUI.".into(),
                        });
                    }
                }
            }
            KeyCode::Enter if has_shift || has_ctrl => app.input_state.insert_char('\n'),
            KeyCode::Tab => app.input_state.insert_str("  "),
            KeyCode::PageUp => scroll_page(&mut app.scroll_view_state, true, page),
            KeyCode::PageDown => scroll_page(&mut app.scroll_view_state, false, page),
            KeyCode::Up if !has_shift && app.input_state.is_at_top(app.wrap_width) && !app.messages.is_empty() => {
                enter_history(app);
            }
            _ => app.input_state.handle_key_event(key, app.wrap_width),
        }
        return false;
    }

    // Non-editable input (agent thinking): allow scrolling, ignore text input
    match key.code {
        KeyCode::PageUp | KeyCode::PageDown | KeyCode::Up | KeyCode::Down => {
            let up = matches!(key.code, KeyCode::PageUp | KeyCode::Up);
            scroll_page(&mut app.scroll_view_state, up, page);
        }
        _ => {}
    }

    false
}

/// Converts protobuf `AgentBlock` messages into TUI `DisplayBlock` values.
fn proto_blocks_to_display(proto_blocks: &[AgentBlock]) -> Vec<DisplayBlock> {
    proto_blocks
        .iter()
        .filter(|b| !b.content.is_empty() || b.block_type == "tool_call_ref")
        .map(|b| match b.block_type.as_str() {
            "thought" => DisplayBlock::Thought(b.content.clone()),
            "tool_call_ref" => DisplayBlock::ToolCallRef(b.content.clone()),
            _ => DisplayBlock::Text(b.content.clone()),
        })
        .collect()
}

/// Dispatches a Core event into application state mutations.
///
/// Handles connection, agent lifecycle, tool calls, debug logs,
/// system messages, provider info, and token usage updates.
pub(crate) fn handle_core_event(app: &mut App, event: CoreEvent) {
    let Some(payload) = event.payload else {
        return;
    };
    match payload {
        core_event::Payload::Connected(_) => {
            app.route = Route::Chat;

            let entries: Vec<HistoryEntry> = app
                .messages
                .iter()
                .filter_map(|e| match e {
                    ChatEntry::User { text } => Some(HistoryEntry {
                        role: "user".into(),
                        content: text.clone(),
                    }),
                    ChatEntry::Agent { blocks, done, .. } if *done => {
                        let content = blocks
                            .iter()
                            .filter_map(|b| match b {
                                DisplayBlock::Text(t) => Some(t.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        if content.is_empty() {
                            return None;
                        }
                        Some(HistoryEntry {
                            role: "assistant".into(),
                            content,
                        })
                    }
                    _ => None,
                })
                .collect();

            if !entries.is_empty() {
                let msg = TuiMessage {
                    payload: Some(tui_message::Payload::HistorySync(HistorySync {
                        messages: entries,
                    })),
                };
                let _ = app.message_tx.try_send(msg);
            }
        }
        core_event::Payload::AgentStarted(e) => {
            app.input_locked = true;
            app.push_message(ChatEntry::Agent {
                name: e.agent_name,
                task_id: e.task_id,
                blocks: Vec::new(),
                visible_chars: 0,
                done: false,
            });
        }
        core_event::Payload::AgentThinking(e) => {
            if let Some(entry) = find_agent_entry(&mut app.messages, &e.task_id) {
                if let ChatEntry::Agent { blocks, .. } = entry {
                    *blocks = proto_blocks_to_display(&e.blocks);
                }
            }
        }
        core_event::Payload::AgentResponse(e) => {
            if let Some(entry) = find_agent_entry(&mut app.messages, &e.task_id) {
                if let ChatEntry::Agent {
                    blocks,
                    visible_chars,
                    done,
                    ..
                } = entry
                {
                    *blocks = proto_blocks_to_display(&e.blocks);
                    *visible_chars = total_block_chars(blocks);
                    *done = true;
                }
            }
            app.save_session();
            app.input_locked = false;
            app.focus = Focus::Input;
            app.focused_message_idx = None;
        }
        core_event::Payload::AgentError(e) => {
            if let Some(entry) = find_agent_entry(&mut app.messages, &e.task_id) {
                if let ChatEntry::Agent { done, .. } = entry {
                    *done = true;
                }
            }
            app.push_message(ChatEntry::System {
                text: format!(
                    "Error ({}): {}",
                    &e.task_id[..8.min(e.task_id.len())],
                    e.error
                ),
            });
            app.input_locked = false;
            app.focus = Focus::Input;
            app.focused_message_idx = None;
        }
        core_event::Payload::AgentToolCall(e) => {
            let status = match e.status.as_str() {
                "done" => ToolCallStatus::Done,
                "failed" => ToolCallStatus::Failed,
                _ => ToolCallStatus::Running,
            };

            if let Some(tc) = app.tool_calls.get_mut(&e.call_id) {
                tc.status = status;
                tc.duration_ms = e.duration_ms;
                tc.result = e.result;
            } else {
                app.tool_calls.insert(
                    e.call_id.clone(),
                    ToolCallData {
                        call_id: e.call_id,
                        tool_name: e.tool_name,
                        arguments_preview: e.arguments_preview,
                        status,
                        started_at: Instant::now(),
                        duration_ms: e.duration_ms,
                        result: e.result,
                    },
                );
            }
        }
        core_event::Payload::DebugLog(e) => {
            if !app.debug_enabled {
                return;
            }
            let secs = e.timestamp_ms / 1000;
            let millis = e.timestamp_ms % 1000;
            let naive = chrono::DateTime::from_timestamp(secs as i64, (millis * 1_000_000) as u32)
                .map(|dt| dt.format("%H:%M:%S%.3f").to_string())
                .unwrap_or_else(|| format!("{}ms", e.timestamp_ms));

            app.push_message(ChatEntry::Debug {
                source: e.source,
                level: e.level,
                message: e.message,
                timestamp: naive,
            });
        }
        core_event::Payload::System(e) => {
            app.push_message(ChatEntry::System { text: e.message });
        }
        core_event::Payload::ProviderInfo(e) => {
            app.provider_name = e.provider_name;
            app.model = e.model;
            app.reasoning_effort = e.reasoning_effort;
        }
        core_event::Payload::TokenUsage(e) => {
            app.total_tokens = e.total_tokens;
            app.context_window = e.context_window;
        }
    }
}

/// Finds the most recent agent entry matching the given task ID.
fn find_agent_entry<'a>(
    messages: &'a mut [ChatEntry],
    target_id: &str,
) -> Option<&'a mut ChatEntry> {
    messages
        .iter_mut()
        .rev()
        .find(|entry| matches!(entry, ChatEntry::Agent { task_id, .. } if task_id == target_id))
}
