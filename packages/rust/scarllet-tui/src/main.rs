mod git_info;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
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
enum Focus {
    Input,
    History,
}

#[derive(Clone, PartialEq)]
enum ToolCallStatus {
    Running,
    Done,
    Failed,
}

struct ToolCallData {
    #[allow(dead_code)]
    call_id: String,
    tool_name: String,
    arguments_preview: String,
    status: ToolCallStatus,
    started_at: Instant,
    duration_ms: u64,
    result: String,
}

enum DisplayBlock {
    Thought(String),
    Text(String),
    ToolCallRef(String),
}

enum ChatEntry {
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
    input: String,
    cursor_pos: usize,
    input_locked: bool,
    focus: Focus,
    scroll_offset: u16,
    max_scroll: u16,
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
            input: String::new(),
            cursor_pos: 0,
            input_locked: false,
            focus: Focus::Input,
            scroll_offset: 0,
            max_scroll: 0,
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
        }
    }

    fn char_count(&self) -> usize {
        self.input.chars().count()
    }

    fn byte_offset_at(&self, char_pos: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_pos)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
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
    let byte_pos = app.byte_offset_at(app.cursor_pos);
    app.input.insert_str(byte_pos, text);
    app.cursor_pos += text.chars().count();
}

fn handle_paste(app: &mut App, text: &str) {
    if app.focus != Focus::Input || app.input_locked {
        return;
    }
    if !matches!(app.route, Route::Chat) {
        return;
    }
    let cleaned = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = cleaned.trim_end();
    if !trimmed.is_empty() {
        insert_text_at_cursor(app, trimmed);
    }
}

fn handle_input(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if !matches!(app.route, Route::Chat) {
        return false;
    }

    let input_editable = app.focus == Focus::Input && !app.input_locked;
    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Input => Focus::History,
                Focus::History => Focus::Input,
            };
        }
        KeyCode::Enter
            if input_editable && (key.modifiers.contains(KeyModifiers::SHIFT) || has_ctrl) =>
        {
            insert_text_at_cursor(app, "\n");
        }
        KeyCode::Enter if input_editable => {
            let trimmed = app.input.trim().to_string();
            if trimmed.eq_ignore_ascii_case("exit") {
                return true;
            }
            if !trimmed.is_empty() {
                app.scroll_offset = 0;
                app.push_message(ChatEntry::User {
                    text: trimmed.clone(),
                });
                app.input.clear();
                app.cursor_pos = 0;

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
        KeyCode::Char(c) if input_editable && !has_ctrl => {
            let byte_pos = app.byte_offset_at(app.cursor_pos);
            app.input.insert(byte_pos, c);
            app.cursor_pos += 1;
        }
        KeyCode::Backspace if input_editable => {
            if app.cursor_pos > 0 {
                app.cursor_pos -= 1;
                let byte_pos = app.byte_offset_at(app.cursor_pos);
                app.input.remove(byte_pos);
            }
        }
        KeyCode::Delete if input_editable => {
            if app.cursor_pos < app.char_count() {
                let byte_pos = app.byte_offset_at(app.cursor_pos);
                app.input.remove(byte_pos);
            }
        }
        KeyCode::Left if input_editable => {
            app.cursor_pos = app.cursor_pos.saturating_sub(1);
        }
        KeyCode::Right if input_editable => {
            let max = app.char_count();
            if app.cursor_pos < max {
                app.cursor_pos += 1;
            }
        }
        KeyCode::Home if input_editable => {
            app.cursor_pos = 0;
        }
        KeyCode::End if input_editable => {
            app.cursor_pos = app.char_count();
        }
        KeyCode::Up if app.focus == Focus::History => {
            app.scroll_offset = app.scroll_offset.saturating_add(1);
        }
        KeyCode::Down if app.focus == Focus::History => {
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
        }
        _ => {}
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
            let naive =
                chrono::DateTime::from_timestamp(secs as i64, (millis * 1_000_000) as u32)
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

fn compute_input_height(input: &str, available_width: u16, max_height: u16) -> u16 {
    if available_width == 0 {
        return 1;
    }

    let input_col_width = available_width.saturating_sub(INPUT_PREFIX_WIDTH).max(1);

    let mut line_count: u16 = 0;

    for line in input.split('\n') {
        let line_len = line.chars().count() as u16;
        let wrapped = line_len.div_ceil(input_col_width).max(1);
        line_count += wrapped;
    }

    line_count = line_count.max(1);

    line_count.min(max_height) - 2
}

fn focused_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Rgb(80, 100, 180))
    } else {
        Style::default().fg(Color::Rgb(30, 30, 30))
    }
}

fn draw_chat(frame: &mut Frame, app: &mut App) {
    let total_height = frame.area().height;
    let max_input_lines = (total_height as u32 * 35 / 100).max(1) as u16;
    let border_h = 2u16;
    let input_inner_width = frame.area().width.saturating_sub(2 + 1);
    let input_lines = compute_input_height(&app.input, input_inner_width, max_input_lines);

    let [history_area, _, input_area, _, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(input_lines + border_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_history(frame, app, history_area);
    draw_input(frame, app, input_area);
    draw_status_bar(frame, app, status_area);
}

fn thinking_dots(tick: u64) -> &'static str {
    match (tick / 3) % 4 {
        1 => "·",
        2 => "··",
        3 => "···",
        _ => "",
    }
}

fn render_tool_call_lines(tc: &ToolCallData, lines: &mut Vec<Line>, width: u16) {
    let ToolCallData {
        tool_name,
        arguments_preview,
        status,
        started_at,
        duration_ms,
        result,
        ..
    } = tc;

    let border = Span::styled("│ ", Style::default().fg(Color::Rgb(100, 60, 140)));
    let content_w = width.saturating_sub(2) as usize;

    let title = format!("{tool_name} - {arguments_preview}");
    let title_style = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);
    let title_lines = vec![Line::from(Span::styled(title, title_style))];
    lines.extend(prepend_border(title_lines, border.clone(), content_w));

    if !result.is_empty() {
        let first_line = result.lines().next().unwrap_or("");
        let result_style = Style::default().fg(Color::DarkGray);
        let result_lines = vec![Line::from(Span::styled(
            first_line.to_string(),
            result_style,
        ))];
        lines.extend(prepend_border(result_lines, border.clone(), content_w));
    }

    let (status_text, status_style) = match status {
        ToolCallStatus::Running => {
            let elapsed_secs = started_at.elapsed().as_secs();
            (
                format!("running for {elapsed_secs}s..."),
                Style::default().fg(Color::Blue),
            )
        }
        ToolCallStatus::Done => {
            let secs = *duration_ms / 1000;
            (
                format!("ran for {secs}s."),
                Style::default().fg(Color::Rgb(0, 160, 0)),
            )
        }
        ToolCallStatus::Failed => {
            let secs = *duration_ms / 1000;
            (
                format!("failed after {secs}s."),
                Style::default().fg(Color::LightRed),
            )
        }
    };
    let status_lines = vec![Line::from(Span::styled(status_text, status_style))];
    lines.extend(prepend_border(status_lines, border, content_w));
}

/// Takes lines produced by markdown rendering and prepends a left-border span to each,
/// re-wrapping content so continuation lines also get the border.
fn prepend_border<'a>(
    src_lines: Vec<Line<'a>>,
    border: Span<'a>,
    content_width: usize,
) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    for line in src_lines {
        let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let plain_chars: Vec<char> = plain.chars().collect();

        if plain_chars.is_empty() {
            out.push(Line::from(vec![border.clone()]));
            continue;
        }

        let style = if line.spans.len() == 1 {
            line.spans[0].style
        } else {
            Style::default()
        };

        if plain_chars.len() <= content_width {
            let mut spans = vec![border.clone()];
            spans.extend(line.spans);
            out.push(Line::from(spans));
        } else {
            for chunk in plain_chars.chunks(content_width) {
                let text: String = chunk.iter().collect();
                out.push(Line::from(vec![
                    border.clone(),
                    Span::styled(text, style),
                ]));
            }
        }
    }
    out
}

fn byte_offset_for_chars(s: &str, max_chars: usize) -> usize {
    s.char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn draw_history(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(focused_border_style(app.focus == Focus::History))
        .padding(Padding::left(1));

    let inner = block.inner(area);

    if app.messages.is_empty() {
        let welcome = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "Welcome to Scarllet. Type a message to begin.",
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .block(block);
        frame.render_widget(welcome, area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, entry) in app.messages.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        match entry {
            ChatEntry::User { text } => {
                lines.push(Line::from(Span::styled(
                    "You: ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                let md = tui_markdown::from_str(text);
                for line in md.lines {
                    lines.push(line);
                }
            }
            ChatEntry::Agent {
                name,
                task_id,
                blocks,
                visible_chars,
                done,
            } => {
                let id_short = &task_id[..8.min(task_id.len())];
                let label = format!("{name} ({id_short}): ");
                lines.push(Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )));

                lines.push(Line::styled("", Style::default().fg(Color::DarkGray)));

                let mut chars_budget = *visible_chars;
                for (bi, blk) in blocks.iter().enumerate() {
                    if bi > 0 {
                        lines.push(Line::raw(""));
                    }
                    match blk {
                        DisplayBlock::Thought(text) => {
                            if chars_budget == 0 {
                                break;
                            }
                            let char_count = text.chars().count();
                            let take = chars_budget.min(char_count);
                            let visible_end = byte_offset_for_chars(text, take);
                            let visible = &text[..visible_end];
                            chars_budget -= take;
                            let border = Span::styled(
                                "│ ",
                                Style::default().fg(Color::DarkGray),
                            );
                            let content_w = inner.width.saturating_sub(2) as usize;
                            let md = tui_markdown::from_str(visible);
                            let dimmed: Vec<Line> = md
                                .lines
                                .into_iter()
                                .map(|l| {
                                    let spans: Vec<Span> =
                                        l.spans.into_iter().map(|s| s.dark_gray()).collect();
                                    Line::from(spans)
                                })
                                .collect();
                            lines.extend(prepend_border(dimmed, border, content_w));
                        }
                        DisplayBlock::Text(text) => {
                            if chars_budget == 0 {
                                break;
                            }
                            let char_count = text.chars().count();
                            let take = chars_budget.min(char_count);
                            let visible_end = byte_offset_for_chars(text, take);
                            let visible = &text[..visible_end];
                            chars_budget -= take;
                            let md = tui_markdown::from_str(visible);
                            for line in md.lines {
                                lines.push(line);
                            }
                        }
                        DisplayBlock::ToolCallRef(call_id) => {
                            if let Some(tc) = app.tool_calls.get(call_id) {
                                render_tool_call_lines(tc, &mut lines, inner.width);
                            }
                        }
                    }
                }

                if !done {
                    let dots = thinking_dots(app.tick);
                    lines.push(Line::styled(
                        format!("Working{dots}"),
                        Style::default().fg(Color::Yellow),
                    ));
                }
            }
            ChatEntry::Debug {
                source,
                level,
                message,
                timestamp,
            } => {
                let level_style = match level.as_str() {
                    "error" => Style::default().fg(Color::LightRed),
                    "warn" => Style::default().fg(Color::Yellow),
                    _ => Style::default().fg(Color::DarkGray),
                };
                let label = format!("{level} - {timestamp} [{source}]: ");
                lines.push(Line::from(vec![
                    Span::styled(label, level_style),
                    Span::styled(message.to_string(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            ChatEntry::System { text } => {
                lines.push(Line::styled(
                    format!("System: {text}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total_lines = paragraph.line_count(inner.width) as u16;
    let viewport_height = inner.height;

    let max_scroll = total_lines.saturating_sub(viewport_height);
    app.max_scroll = max_scroll;
    app.scroll_offset = app.scroll_offset.min(max_scroll);
    let scroll_y = max_scroll - app.scroll_offset;

    let paragraph = paragraph.block(block).scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);

    if total_lines > viewport_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("┃")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(Color::DarkGray))
            .track_style(Style::default().fg(Color::Rgb(40, 40, 40)));

        let position = scroll_y as usize;
        let content_len = max_scroll as usize;
        let mut scrollbar_state = ScrollbarState::new(content_len).position(position);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(focused_border_style(app.focus == Focus::Input))
        .padding(Padding::left(1));

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

    let prefix = Paragraph::new(Line::styled("$ ", Style::default().fg(Color::White)));
    frame.render_widget(prefix, prefix_area);

    let input_lines: Vec<&str> = app.input.split('\n').collect();
    let wrap_width = input_col.width.max(1) as usize;
    let mut display_lines: Vec<Line> = Vec::new();
    for line in &input_lines {
        if line.is_empty() {
            display_lines.push(Line::raw(""));
        } else {
            let chars: Vec<char> = line.chars().collect();
            for chunk in chars.chunks(wrap_width) {
                display_lines.push(Line::raw(chunk.iter().collect::<String>()));
            }
        }
    }
    let paragraph = Paragraph::new(display_lines);
    frame.render_widget(paragraph, input_col);

    if app.focus == Focus::Input {
        let wrap_width = input_col.width.max(1);
        let mut chars_remaining = app.cursor_pos;
        let mut cursor_row: u16 = 0;
        let mut found = false;

        for (i, line) in input_lines.iter().enumerate() {
            let line_chars = line.chars().count();
            let is_last = i == input_lines.len() - 1;

            if chars_remaining <= line_chars && (chars_remaining < line_chars || is_last) {
                let col = chars_remaining as u16;
                let visual_row = col / wrap_width;
                let visual_col = col % wrap_width;
                cursor_row += visual_row;
                frame.set_cursor_position((input_col.x + visual_col, input_col.y + cursor_row));
                found = true;
                break;
            }

            let line_wrapped_rows = (line_chars as u16).div_ceil(wrap_width).max(1);
            cursor_row += line_wrapped_rows;
            chars_remaining = chars_remaining.saturating_sub(line_chars + 1);
        }

        if !found {
            frame.set_cursor_position((input_col.x, input_col.y));
        }
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

    let mut spans: Vec<Span> = vec![Span::styled(left_out, style)];

    if !right_out.is_empty() && left_len + right_len < width {
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
