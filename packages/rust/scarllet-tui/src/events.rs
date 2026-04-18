//! TUI input + diff-apply handlers.
//!
//! Translates `crossterm` key events into outbound [`CoreCommand`]s
//! (`SendPrompt`, `StopSession`, `DestroyAndRecreate`) and folds
//! incoming [`SessionDiff`]s into the local [`App`] mirror.

use crossterm::event::{KeyCode, KeyModifiers};

use scarllet_proto::proto::*;

use crate::app::{AgentSummary, App, CoreCommand, Focus, ProviderInfo, Route, SessionStatus};

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

/// Moves focus back to the input pane and clears the history selection.
fn return_to_input(app: &mut App) {
    app.focus = Focus::Input;
    app.focused_message_idx = None;
}

/// Switches focus to the history pane, selecting the latest top-level node.
fn enter_history(app: &mut App) {
    let count = app.top_level_nodes().count();
    if count > 0 {
        app.focus = Focus::History;
        app.focused_message_idx = Some(count - 1);
    }
}

/// Calculates the page-scroll increment as 1/4 of the viewport height.
fn scroll_page_increment(viewport_height: u16) -> u16 {
    (viewport_height / 4).max(1)
}

/// Processes a key event and returns `true` if the application should exit.
pub(crate) fn handle_input(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if key.code == KeyCode::Esc && matches!(app.route, Route::Chat) {
        let should_stop = match app.session_status {
            SessionStatus::Paused => true,
            SessionStatus::Running => app.is_streaming(),
        };
        if should_stop {
            let _ = app.command_tx.try_send(CoreCommand::StopSession);
            return false;
        }
    }

    if !matches!(app.route, Route::Chat) {
        return false;
    }

    if app.focus == Focus::History {
        let count = app.top_level_nodes().count();
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
                    if idx + 1 < count {
                        app.focused_message_idx = Some(idx + 1);
                    } else {
                        return_to_input(app);
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(idx) = app.focused_message_idx {
                    app.toggle_spawn_sub_agent_expand(idx);
                }
            }
            KeyCode::Esc => return_to_input(app),
            KeyCode::PageUp => scroll_page(
                &mut app.scroll_view_state,
                true,
                scroll_page_increment(app.history_viewport_height),
            ),
            KeyCode::PageDown => scroll_page(
                &mut app.scroll_view_state,
                false,
                scroll_page_increment(app.history_viewport_height),
            ),
            _ => {}
        }
        return false;
    }

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
                    app.input_state.set_text(String::new());
                    let cwd = std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let _ = app.command_tx.try_send(CoreCommand::SendPrompt {
                        text: trimmed,
                        cwd,
                    });
                }
            }
            KeyCode::Enter if has_shift || has_ctrl => app.input_state.insert_char('\n'),
            KeyCode::Tab => app.input_state.insert_str("  "),
            KeyCode::PageUp => scroll_page(&mut app.scroll_view_state, true, page),
            KeyCode::PageDown => scroll_page(&mut app.scroll_view_state, false, page),
            KeyCode::Up
                if !has_shift
                    && app.input_state.is_at_top(app.wrap_width)
                    && app.top_level_nodes().count() > 0 =>
            {
                enter_history(app);
            }
            _ => app.input_state.handle_key_event(key, app.wrap_width),
        }
        return false;
    }

    match key.code {
        KeyCode::PageUp | KeyCode::PageDown | KeyCode::Up | KeyCode::Down => {
            let up = matches!(key.code, KeyCode::PageUp | KeyCode::Up);
            scroll_page(&mut app.scroll_view_state, up, page);
        }
        _ => {}
    }

    false
}

/// Applies one [`SessionDiff`] received from core to the local mirror.
pub(crate) fn handle_session_diff(app: &mut App, diff: SessionDiff) {
    let Some(payload) = diff.payload else {
        return;
    };
    match payload {
        session_diff::Payload::Attached(att) => {
            app.route = Route::Chat;
            let Some(state) = att.state else {
                return;
            };
            let queue_len = state.queue.len();
            let agents = state.agents.into_iter().map(agent_to_summary).collect();
            let provider_info = state.provider.and_then(ProviderInfo::from_wire);
            app.reset_with(
                state.session_id,
                SessionStatus::from_wire(&state.status),
                state.nodes,
                queue_len,
                agents,
                provider_info,
            );
        }
        session_diff::Payload::NodeCreated(nc) => {
            if let Some(node) = nc.node {
                app.insert_node(node);
            }
        }
        session_diff::Payload::NodeUpdated(nu) => {
            let Some(patch) = nu.patch else {
                return;
            };
            app.apply_node_patch(&nu.node_id, patch, nu.updated_at);
        }
        session_diff::Payload::QueueChanged(qc) => {
            app.queue_len = qc.queued.len();
        }
        session_diff::Payload::AgentRegistered(reg) => {
            app.connected_agents.insert(
                reg.agent_id.clone(),
                AgentSummary {
                    agent_id: reg.agent_id,
                },
            );
        }
        session_diff::Payload::AgentUnregistered(unreg) => {
            app.connected_agents.remove(&unreg.agent_id);
        }
        session_diff::Payload::StatusChanged(sc) => {
            app.session_status = SessionStatus::from_wire(&sc.status);
        }
        session_diff::Payload::Destroyed(_) => {
            app.session_id = None;
            app.nodes.clear();
            app.node_order.clear();
            app.queue_len = 0;
            app.connected_agents.clear();
            app.provider_info = None;
            app.scroll_view_state = crate::widgets::ScrollViewState::new();
            app.focused_message_idx = None;
            app.input_locked = false;
            app.reveal.clear();
        }
    }
}

/// Translates a proto `AgentSummary` into the TUI snapshot type.
fn agent_to_summary(a: scarllet_proto::proto::AgentSummary) -> AgentSummary {
    AgentSummary {
        agent_id: a.agent_id,
    }
}
