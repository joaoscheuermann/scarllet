mod git_info;
mod input;
mod widgets;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;
use scarllet_proto::proto::core_event;
use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::tui_message;
use scarllet_proto::proto::*;
use scarllet_sdk::lockfile;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const TYPEWRITER_CHARS_PER_TICK: usize = 30;

#[derive(Clone, PartialEq)]
pub(crate) enum Focus {
    Input,
    History,
}

#[derive(Clone, PartialEq)]
pub(crate) enum ToolCallStatus {
    Running,
    Done,
    Failed,
}

pub(crate) struct ToolCallData {
    #[allow(dead_code)]
    pub(crate) call_id: String,
    pub(crate) tool_name: String,
    pub(crate) arguments_preview: String,
    pub(crate) status: ToolCallStatus,
    pub(crate) started_at: Instant,
    pub(crate) duration_ms: u64,
    pub(crate) result: String,
}

pub(crate) enum DisplayBlock {
    Thought(String),
    Text(String),
    ToolCallRef(String),
}

pub(crate) enum ChatEntry {
    User {
        text: String,
    },
    Agent {
        name: String,
        task_id: String,
        blocks: Vec<DisplayBlock>,
        visible_chars: usize,
        done: bool,
    },
    Debug {
        source: String,
        level: String,
        message: String,
        timestamp: String,
    },
    System {
        text: String,
    },
}

enum Route {
    Connecting { tick: u64 },
    Chat,
}

const ENV_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

struct App {
    route: Route,
    messages: Vec<ChatEntry>,
    tool_calls: HashMap<String, ToolCallData>,
    input_state: input::InputState,
    input_locked: bool,
    focus: Focus,
    wrap_width: u16,
    scroll_view_state: widgets::ScrollViewState,
    focused_message_idx: Option<usize>,
    tick: u64,
    stream_closed: bool,
    message_tx: mpsc::Sender<TuiMessage>,
    provider_name: String,
    model: String,
    reasoning_effort: String,
    cwd: PathBuf,
    cwd_display: String,
    git_info: Option<git_info::GitInfo>,
    last_env_refresh: Instant,
    debug_enabled: bool,
    total_tokens: u32,
    context_window: u32,
}

fn total_block_chars(blocks: &[DisplayBlock]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            DisplayBlock::Thought(t) | DisplayBlock::Text(t) => t.chars().count(),
            DisplayBlock::ToolCallRef(_) => 0,
        })
        .sum()
}

impl App {
    fn new(message_tx: mpsc::Sender<TuiMessage>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let cwd_display = git_info::abbreviate_home(&cwd);
        let git = git_info::read_git_info(&cwd);
        Self {
            route: Route::Connecting { tick: 0 },
            messages: Vec::new(),
            tool_calls: HashMap::new(),
            input_state: input::InputState::new(),
            input_locked: false,
            focus: Focus::Input,
            wrap_width: 80,
            scroll_view_state: widgets::ScrollViewState::new(),
            focused_message_idx: None,
            tick: 0,
            stream_closed: false,
            message_tx,
            provider_name: String::new(),
            model: String::new(),
            reasoning_effort: String::new(),
            cwd,
            cwd_display,
            git_info: git,
            last_env_refresh: Instant::now(),
            debug_enabled: std::env::var("SCARLLET_DEBUG")
                .map(|v| v == "true")
                .unwrap_or(false),
            total_tokens: 0,
            context_window: 0,
        }
    }

    fn refresh_env(&mut self) {
        if self.last_env_refresh.elapsed() < ENV_REFRESH_INTERVAL {
            return;
        }
        self.last_env_refresh = Instant::now();
        self.cwd = std::env::current_dir().unwrap_or_default();
        self.cwd_display = git_info::abbreviate_home(&self.cwd);
        self.git_info = git_info::read_git_info(&self.cwd);
    }

    fn advance_tick(&mut self) {
        self.tick += 1;
        self.refresh_env();
        if let Route::Connecting { ref mut tick } = self.route {
            *tick += 1;
        }
        for entry in &mut self.messages {
            if let ChatEntry::Agent {
                blocks,
                visible_chars,
                done,
                ..
            } = entry
            {
                let total = total_block_chars(blocks);
                if *visible_chars < total {
                    *visible_chars = (*visible_chars + TYPEWRITER_CHARS_PER_TICK).min(total);
                }
                if *done {
                    *visible_chars = total;
                }
            }
        }
    }

    fn is_streaming(&self) -> bool {
        self.messages
            .iter()
            .any(|e| matches!(e, ChatEntry::Agent { done: false, .. }))
            || self
                .tool_calls
                .values()
                .any(|tc| tc.status == ToolCallStatus::Running)
    }

    fn push_message(&mut self, entry: ChatEntry) {
        self.messages.push(entry);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original_hook(info);
    }));

    let mut terminal = ratatui::init();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste,
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    )
    .ok();

    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(256);
    let (message_tx, message_rx) = mpsc::channel::<TuiMessage>(256);

    tokio::spawn(async move {
        connect_and_stream(event_tx, message_rx).await;
    });

    let mut app = App::new(message_tx);

    loop {
        loop {
            match event_rx.try_recv() {
                Ok(event) => handle_core_event(&mut app, event),
                Err(mpsc::error::TryRecvError::Disconnected) if !app.stream_closed => {
                    app.stream_closed = true;
                    if matches!(app.route, Route::Chat) {
                        app.push_message(ChatEntry::System {
                            text: "Disconnected from Core.".into(),
                        });
                        app.input_locked = false;
                    }
                }
                _ => break,
            }
        }

        terminal.draw(|f| routes(f, &mut app))?;

        let poll_ms = if app.is_streaming() { 50 } else { 200 };
        if event::poll(Duration::from_millis(poll_ms))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != crossterm::event::KeyEventKind::Press {
                        continue;
                    }
                    if handle_input(&mut app, key) {
                        break;
                    }
                }
                Event::Paste(text) => {
                    handle_paste(&mut app, &text);
                }
                _ => {}
            }
        }

        app.advance_tick();
    }

    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::PopKeyboardEnhancementFlags
    )
    .ok();
    ratatui::restore();
    Ok(())
}

fn insert_text_at_cursor(app: &mut App, text: &str) {
    app.input_state.insert_str(text);
}

fn handle_paste(app: &mut App, text: &str) {
    if app.focus != Focus::Input || app.input_locked {
        return;
    }
    if !matches!(app.route, Route::Chat) {
        return;
    }
    let cleaned = text.replace("\r\n", "\n").replace('\r', "\n");
    if !cleaned.is_empty() {
        insert_text_at_cursor(app, &cleaned);
    }
}

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

fn return_to_input(app: &mut App) {
    app.focus = Focus::Input;
    app.focused_message_idx = None;
}

fn enter_history(app: &mut App) {
    if !app.messages.is_empty() {
        app.focus = Focus::History;
        app.focused_message_idx = Some(app.messages.len() - 1);
    }
}

fn handle_input(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if !matches!(app.route, Route::Chat) {
        return false;
    }

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
            KeyCode::Esc => {
                return_to_input(app);
            }
            KeyCode::Char(c) => {
                return_to_input(app);
                app.input_state.insert_char(c);
            }
            KeyCode::PageUp => {
                app.scroll_view_state.offset_y = app.scroll_view_state.offset_y.saturating_sub(1);
            }
            KeyCode::PageDown => {
                app.scroll_view_state.offset_y += 1;
            }
            _ => {}
        }
        return false;
    }

    if key.code == KeyCode::Esc && app.is_streaming() {
        if let Some(task_id) = find_running_task_id(&app.messages) {
            let msg = TuiMessage {
                payload: Some(tui_message::Payload::Cancel(CancelPrompt { task_id })),
            };
            let _ = app.message_tx.try_send(msg);
        }
        return false;
    }

    let input_editable = app.focus == Focus::Input && !app.input_locked;
    let has_shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if input_editable {
        if key.code == KeyCode::Enter && !has_shift && !has_ctrl {
            let trimmed = app.input_state.text().trim().to_string();
            if trimmed.eq_ignore_ascii_case("exit") {
                return true;
            }
            if !trimmed.is_empty() {
                app.scroll_view_state.scroll_to_bottom();
                app.push_message(ChatEntry::User {
                    text: trimmed.clone(),
                });
                app.input_state.set_text(String::new());

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
            return false;
        } else if key.code == KeyCode::Enter && (has_shift || has_ctrl) {
            app.input_state.insert_char('\n');
            return false;
        } else if key.code == KeyCode::Tab {
            app.input_state.insert_str("  ");
            return false;
        } else if key.code == KeyCode::PageUp {
            app.scroll_view_state.offset_y = app.scroll_view_state.offset_y.saturating_sub(1);
            return false;
        } else if key.code == KeyCode::PageDown {
            app.scroll_view_state.offset_y += 1;
            return false;
        } else if key.code == KeyCode::Up
            && !has_shift
            && app.input_state.is_at_top(app.wrap_width)
            && !app.messages.is_empty()
        {
            enter_history(app);
            return false;
        }

        app.input_state.handle_key_event(key, app.wrap_width);
    } else {
        match key.code {
            KeyCode::PageUp | KeyCode::Up => {
                app.scroll_view_state.offset_y = app.scroll_view_state.offset_y.saturating_sub(1);
            }
            KeyCode::PageDown | KeyCode::Down => {
                app.scroll_view_state.offset_y += 1;
            }
            _ => {}
        }
    }

    false
}

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

fn handle_core_event(app: &mut App, event: CoreEvent) {
    let Some(payload) = event.payload else {
        return;
    };
    match payload {
        core_event::Payload::Connected(_) => {
            app.route = Route::Chat;
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

fn find_agent_entry<'a>(
    messages: &'a mut [ChatEntry],
    target_id: &str,
) -> Option<&'a mut ChatEntry> {
    messages
        .iter_mut()
        .rev()
        .find(|entry| matches!(entry, ChatEntry::Agent { task_id, .. } if task_id == target_id))
}

fn routes(frame: &mut Frame, app: &mut App) {
    match &app.route {
        Route::Connecting { tick } => draw_connecting(frame, *tick),
        Route::Chat => draw_chat(frame, app),
    }
}

fn draw_connecting(frame: &mut Frame, tick: u64) {
    let dots = match (tick / 3) % 4 {
        1 => ".",
        2 => "..",
        3 => "...",
        _ => "",
    };
    let text = format!("Connecting to agent core{dots}");
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center);

    let [_, center, _] = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Length(1),
        Constraint::Percentage(45),
    ])
    .areas(frame.area());

    frame.render_widget(paragraph, center);
}

const INPUT_PREFIX_WIDTH: u16 = 2;

fn compute_input_height(app: &input::InputState, available_width: u16, max_height: u16) -> u16 {
    if available_width == 0 {
        return 1;
    }
    let input_col_width = available_width.saturating_sub(INPUT_PREFIX_WIDTH).max(1);
    let line_count = app.visual_lines(input_col_width).len() as u16;
    line_count.max(1).min(max_height)
}

fn draw_chat(frame: &mut Frame, app: &mut App) {
    let total_height = frame.area().height;
    let max_input_lines = (total_height as u32 * 30 / 100).max(1) as u16; // 30% of screen as per spec
    let input_inner_width = frame.area().width.saturating_sub(2 + 1);
    let input_lines = compute_input_height(&app.input_state, input_inner_width, max_input_lines);

    let [history_area, _, input_area, _, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(input_lines),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_history(frame, app, history_area);
    draw_input(frame, app, input_area);
    draw_status_bar(frame, app, status_area);
}

fn draw_history(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default();

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.messages.is_empty() {
        let welcome = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "Welcome to Scarllet. Type a message to begin.",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(welcome, inner);
        return;
    }

    use widgets::ScrollItem;
    let msg_widgets: Vec<widgets::ChatMessageWidget> = app
        .messages
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            widgets::ChatMessageWidget::new(
                entry,
                app.focused_message_idx == Some(i),
                &app.tool_calls,
                app.tick,
                inner.width,
            )
        })
        .collect();

    if let Some(idx) = app.focused_message_idx {
        let gap = 1u16;
        let mut y = 0u16;
        for (i, w) in msg_widgets.iter().enumerate() {
            if i > 0 {
                y = y.saturating_add(gap);
            }
            if i == idx {
                app.scroll_view_state
                    .ensure_visible(y, w.height(), inner.height);
                break;
            }
            y = y.saturating_add(w.height());
        }
    }

    let items: Vec<&dyn widgets::ScrollItem> = msg_widgets
        .iter()
        .map(|w| w as &dyn widgets::ScrollItem)
        .collect();

    widgets::ScrollView::render(
        inner,
        frame.buffer_mut(),
        &mut app.scroll_view_state,
        &items,
        1,
    );

    if app.scroll_view_state.content_height > inner.height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("┃")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(Color::DarkGray))
            .track_style(Style::default().fg(Color::Rgb(40, 40, 40)));

        let max_offset = app
            .scroll_view_state
            .content_height
            .saturating_sub(inner.height) as usize;
        let position = app.scroll_view_state.offset_y as usize;
        let mut scrollbar_state = ScrollbarState::new(max_offset).position(position);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn draw_input(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default().padding(Padding::left(1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [prefix_area, input_col] =
        Layout::horizontal([Constraint::Length(INPUT_PREFIX_WIDTH), Constraint::Min(0)])
            .areas(inner);

    if app.input_locked {
        let paragraph = Paragraph::new(Line::styled(
            "Waiting for agent...",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(paragraph, input_col);
        return;
    }

    let prefix = Paragraph::new(Line::styled("> ", Style::default().fg(Color::White)));
    frame.render_widget(prefix, prefix_area);

    let w = input_col.width.max(1);
    app.wrap_width = w;

    let visual_lines = app.input_state.visual_lines(w);
    let visible_rows = input_col.height;

    // Adjust scrolling
    let (_, cursor_row) = app.input_state.cursor_visual_position(w);
    if cursor_row < app.input_state.vertical_scroll {
        app.input_state.vertical_scroll = cursor_row;
    }
    if cursor_row >= app.input_state.vertical_scroll + visible_rows {
        app.input_state.vertical_scroll = cursor_row - visible_rows + 1;
    }

    let start = app.input_state.vertical_scroll as usize;
    let end = (start + visible_rows as usize).min(visual_lines.len());

    let mut display_lines: Vec<Line> = Vec::new();
    let sel_range = app.input_state.selection_range();

    for (_, vl) in visual_lines[start..end].iter().enumerate() {
        let line_text = &app.input_state.text()[vl.byte_start..vl.byte_end];

        if let Some((sel_start, sel_end)) = sel_range {
            let mut spans = Vec::new();

            for (g_idx, g) in
                unicode_segmentation::UnicodeSegmentation::grapheme_indices(line_text, true)
            {
                let absolute_byte = vl.byte_start + g_idx;
                let is_selected = absolute_byte >= sel_start && absolute_byte < sel_end;
                let style = if is_selected {
                    Style::default().bg(Color::Rgb(60, 60, 80))
                } else {
                    Style::default()
                };
                spans.push(Span::styled(g.to_string(), style));
            }
            display_lines.push(Line::from(spans));
        } else {
            display_lines.push(Line::raw(line_text.to_string()));
        }
    }

    let paragraph = Paragraph::new(display_lines);
    frame.render_widget(paragraph, input_col);

    if app.focus == Focus::Input {
        let (cursor_col, cursor_row) = app.input_state.cursor_visual_position(w);
        if cursor_row >= app.input_state.vertical_scroll
            && cursor_row < app.input_state.vertical_scroll + visible_rows
        {
            let screen_row = cursor_row - app.input_state.vertical_scroll;
            frame.set_cursor_position((input_col.x + cursor_col, input_col.y + screen_row));
        }
    }

    // Render scrollbar if needed
    if visual_lines.len() as u16 > visible_rows {
        let scrollbar = ratatui::widgets::Scrollbar::default()
            .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        let mut state = ratatui::widgets::ScrollbarState::default()
            .content_length(visual_lines.len().saturating_sub(visible_rows as usize))
            .position(app.input_state.vertical_scroll as usize);
        frame.render_stateful_widget(scrollbar, input_col, &mut state);
    }
}

fn format_git_segment(info: &git_info::GitInfo) -> String {
    if info.detached {
        return format!("detached - {}", info.short_sha);
    }
    if let Some(ref branch) = info.branch {
        return format!("{branch} ({}/HEAD)", info.short_sha);
    }
    info.short_sha.clone()
}

fn format_compact(n: u32) -> String {
    if n < 1_000 {
        return format!("{n}");
    }
    let k = n as f64 / 1_000.0;
    let decimal = (k.fract() * 10.0).round() as u32;
    if decimal == 0 {
        format!("{}k", k as u32)
    } else {
        format!("{:.1}k", k)
    }
}

fn format_token_budget(total: u32, window: u32) -> String {
    if window == 0 {
        return String::new();
    }
    let pct = (total as f64 / window as f64 * 100.0).round() as u32;
    format!(
        "{} / {} tokens ({}%)",
        format_compact(total),
        format_compact(window),
        pct
    )
}

fn token_budget_style(total: u32, window: u32) -> Style {
    if window == 0 {
        return Style::default().fg(Color::DarkGray);
    }
    let pct = (total as f64 / window as f64 * 100.0).round() as u32;
    if pct >= 95 {
        Style::default().fg(Color::LightRed)
    } else if pct >= 75 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let style = Style::default().fg(Color::DarkGray);
    let sep = "  │  ";
    let width = area.width as usize;

    let left_cwd = format!(" {}", app.cwd_display);
    let left_git = app.git_info.as_ref().map(format_git_segment);

    let right_provider: Option<String> = if app.provider_name.is_empty() {
        None
    } else {
        Some(app.provider_name.clone())
    };
    let right_model: Option<String> = if app.model.is_empty() {
        None
    } else if app.reasoning_effort.is_empty() {
        Some(app.model.clone())
    } else {
        Some(format!("{} · {}", app.model, app.reasoning_effort))
    };

    let mut right_parts: Vec<String> = Vec::new();
    if let Some(ref prov) = right_provider {
        right_parts.push(prov.clone());
        if let Some(ref model) = right_model {
            right_parts.push(sep.to_string());
            right_parts.push(model.clone());
        }
    }

    let right_str: String = right_parts.concat();
    let left_git_str = left_git
        .as_ref()
        .map(|g| format!("{sep}{g}"))
        .unwrap_or_default();
    let left_full = format!("{left_cwd}{left_git_str}");

    let center_str = format_token_budget(app.total_tokens, app.context_window);
    let center_style = token_budget_style(app.total_tokens, app.context_window);

    let gap = 2usize;

    let (left_out, right_out) =
        if !right_str.is_empty() && left_full.len() + gap + right_str.len() <= width {
            (left_full, right_str)
        } else if right_provider.is_some()
            && left_full.len() + gap + right_provider.as_ref().unwrap().len() <= width
        {
            (left_full, right_provider.unwrap())
        } else if left_full.len() <= width {
            (left_full, String::new())
        } else if left_cwd.len() <= width {
            (left_cwd.clone(), String::new())
        } else {
            let max = width.saturating_sub(1);
            let truncated: String = left_cwd.chars().take(max).collect();
            (format!("{truncated}…"), String::new())
        };

    let left_len = left_out.chars().count();
    let right_len = right_out.chars().count();
    let center_len = center_str.chars().count();

    let mut spans: Vec<Span> = vec![Span::styled(left_out, style)];

    let fits_all_three =
        !center_str.is_empty() && left_len + gap + center_len + gap + right_len <= width;

    if fits_all_three && !right_out.is_empty() {
        let total_content = left_len + center_len + right_len;
        let remaining = width.saturating_sub(total_content);
        let left_pad = remaining / 2;
        let right_pad = remaining - left_pad;

        spans.push(Span::raw(" ".repeat(left_pad)));
        spans.push(Span::styled(center_str, center_style));
        spans.push(Span::raw(" ".repeat(right_pad)));
        spans.push(Span::styled(right_out, style));
    } else if fits_all_three {
        let remaining = width.saturating_sub(left_len + center_len);
        let left_pad = remaining / 2;

        spans.push(Span::raw(" ".repeat(left_pad)));
        spans.push(Span::styled(center_str, center_style));
    } else if !center_str.is_empty() && left_len + gap + center_len <= width && right_out.is_empty()
    {
        let remaining = width.saturating_sub(left_len + center_len);
        let left_pad = remaining / 2;

        spans.push(Span::raw(" ".repeat(left_pad)));
        spans.push(Span::styled(center_str, center_style));
    } else if !right_out.is_empty() && left_len + right_len < width {
        let padding = width.saturating_sub(left_len + right_len);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(right_out, style));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

async fn connect_and_stream(
    event_tx: mpsc::Sender<CoreEvent>,
    message_rx: mpsc::Receiver<TuiMessage>,
) {
    let address = find_core_address().await;

    let Some(channel) = connect_to_core(&address).await else {
        return;
    };

    let mut client = OrchestratorClient::new(channel);
    let outgoing = ReceiverStream::new(message_rx);

    let Ok(response) = client.attach_tui(outgoing).await else {
        return;
    };

    let mut incoming = response.into_inner();
    while let Ok(Some(event)) = incoming.message().await {
        if event_tx.send(event).await.is_err() {
            return;
        }
    }
}

async fn connect_to_core(address: &str) -> Option<tonic::transport::Channel> {
    for _ in 0..10 {
        let endpoint = format!("http://{address}");
        if let Ok(ep) = tonic::transport::Endpoint::from_shared(endpoint) {
            if let Ok(Ok(channel)) =
                tokio::time::timeout(Duration::from_secs(3), ep.connect()).await
            {
                return Some(channel);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

async fn find_core_address() -> String {
    loop {
        if let Ok(Some(lock)) = lockfile::read() {
            if lockfile::is_pid_alive(lock.pid) {
                return lock.address;
            }
            lockfile::remove();
        }

        let _ = spawn_core();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(Some(lock)) = lockfile::read() {
                if lockfile::is_pid_alive(lock.pid) {
                    return lock.address;
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn spawn_core() -> io::Result<()> {
    let self_path = std::env::current_exe()?;
    let dir = self_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine binary dir"))?;
    let mut core_path = dir.join("core");
    if cfg!(windows) {
        core_path.set_extension("exe");
    }
    if !core_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Core binary not found at {}", core_path.display()),
        ));
    }
    std::process::Command::new(&core_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
