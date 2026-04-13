mod content_parser;

use std::io;
use std::time::Duration;

use content_parser::{parse_blocks, ContentBlock};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
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

enum ChatEntry {
    User {
        text: String,
    },
    Agent {
        name: String,
        task_id: String,
        content: String,
        visible_len: usize,
        done: bool,
    },
    System {
        text: String,
    },
}

enum Screen {
    Connecting { tick: u64 },
    Chat,
}

struct App {
    screen: Screen,
    messages: Vec<ChatEntry>,
    input: String,
    input_locked: bool,
    focus: Focus,
    scroll_offset: u16,
    tick: u64,
    stream_closed: bool,
    message_tx: mpsc::Sender<TuiMessage>,
}

impl App {
    fn new(message_tx: mpsc::Sender<TuiMessage>) -> Self {
        Self {
            screen: Screen::Connecting { tick: 0 },
            messages: Vec::new(),
            input: String::new(),
            input_locked: false,
            focus: Focus::Input,
            scroll_offset: 0,
            tick: 0,
            stream_closed: false,
            message_tx,
        }
    }

    fn advance_tick(&mut self) {
        self.tick += 1;
        if let Screen::Connecting { ref mut tick } = self.screen {
            *tick += 1;
        }
        for entry in &mut self.messages {
            if let ChatEntry::Agent {
                content,
                visible_len,
                done,
                ..
            } = entry
            {
                if *visible_len < content.len() {
                    let remaining = &content[*visible_len..];
                    let advance: usize = remaining
                        .chars()
                        .take(TYPEWRITER_CHARS_PER_TICK)
                        .map(|c| c.len_utf8())
                        .sum();
                    *visible_len += advance;
                }
                if *done {
                    *visible_len = content.len();
                }
            }
        }
    }

    fn is_streaming(&self) -> bool {
        self.messages.iter().any(
            |e| matches!(e, ChatEntry::Agent { done: false, .. }),
        )
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
                    if matches!(app.screen, Screen::Chat) {
                        app.push_message(ChatEntry::System {
                            text: "Disconnected from Core.".into(),
                        });
                        app.input_locked = false;
                    }
                }
                _ => break,
            }
        }

        terminal.draw(|f| draw(f, &app))?;

        let poll_ms = if app.is_streaming() { 50 } else { 200 };
        if event::poll(Duration::from_millis(poll_ms))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }
                if handle_input(&mut app, key) {
                    break;
                }
            }
        }

        app.advance_tick();
    }

    ratatui::restore();
    Ok(())
}

fn handle_input(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }

    if !matches!(app.screen, Screen::Chat) {
        return false;
    }

    match key.code {
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Input => Focus::History,
                Focus::History => Focus::Input,
            };
        }
        KeyCode::Enter if app.focus == Focus::Input && !app.input_locked => {
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
        KeyCode::Char(c) if app.focus == Focus::Input && !app.input_locked => {
            app.input.push(c);
        }
        KeyCode::Backspace if app.focus == Focus::Input && !app.input_locked => {
            app.input.pop();
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

fn handle_core_event(app: &mut App, event: CoreEvent) {
    let Some(payload) = event.payload else {
        return;
    };
    match payload {
        core_event::Payload::Connected(_) => {
            app.screen = Screen::Chat;
        }
        core_event::Payload::AgentStarted(e) => {
            app.input_locked = true;
            app.push_message(ChatEntry::Agent {
                name: e.agent_name,
                task_id: e.task_id,
                content: String::new(),
                visible_len: 0,
                done: false,
            });
        }
        core_event::Payload::AgentThinking(e) => {
            if let Some(entry) = find_agent_entry(&mut app.messages, &e.task_id) {
                if let ChatEntry::Agent { content, .. } = entry {
                    *content = e.content;
                }
            }
        }
        core_event::Payload::AgentResponse(e) => {
            if let Some(entry) = find_agent_entry(&mut app.messages, &e.task_id) {
                if let ChatEntry::Agent {
                    content,
                    visible_len,
                    done,
                    ..
                } = entry
                {
                    *content = e.content;
                    *visible_len = content.len();
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
        core_event::Payload::System(e) => {
            app.push_message(ChatEntry::System { text: e.message });
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

fn draw(frame: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Connecting { tick } => draw_connecting(frame, *tick),
        Screen::Chat => draw_chat(frame, app),
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

fn draw_chat(frame: &mut Frame, app: &App) {
    let [history_area, input_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(frame.area());

    draw_history(frame, app, history_area);
    draw_input(frame, app, input_area);
}

fn thinking_dots(tick: u64) -> &'static str {
    match (tick / 3) % 4 {
        1 => "·",
        2 => "··",
        3 => "···",
        _ => "",
    }
}

fn draw_history(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let border_color = if app.focus == Focus::History {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::left(1));

    if app.messages.is_empty() {
        let welcome = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "  Welcome to Scarllet. Type a message to begin.",
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
                lines.push(Line::from(vec![
                    Span::styled(
                        "You: ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text.as_str(), Style::default().fg(Color::White)),
                ]));
            }
            ChatEntry::Agent {
                name,
                task_id,
                content,
                visible_len,
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

                let visible = &content[..*visible_len];
                let blocks = parse_blocks(visible);
                for (bi, blk) in blocks.iter().enumerate() {
                    if bi > 0 {
                        lines.push(Line::raw(""));
                    }
                    match blk {
                        ContentBlock::Thought(text) => {
                            let md = tui_markdown::from_str(text);
                            for line in md.lines {
                                let dimmed_spans: Vec<Span> = line
                                    .spans
                                    .into_iter()
                                    .map(|s| s.dark_gray())
                                    .collect();
                                lines.push(Line::from(dimmed_spans));
                            }
                        }
                        ContentBlock::Response(text) => {
                            let md = tui_markdown::from_str(text);
                            for line in md.lines {
                                lines.push(line);
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
            ChatEntry::System { text } => {
                lines.push(Line::styled(
                    format!("System: {text}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }

    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total_lines = paragraph.line_count(inner.width) as u16;
    let viewport_height = inner.height;

    let scroll_y = if total_lines > viewport_height {
        (total_lines - viewport_height).saturating_sub(app.scroll_offset)
    } else {
        0
    };

    let paragraph = paragraph.block(block).scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let border_color = if app.focus == Focus::Input {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::left(1));

    if app.input_locked {
        let paragraph = Paragraph::new(Line::styled(
            "Waiting for agent...",
            Style::default().fg(Color::DarkGray),
        ))
        .block(block);

        frame.render_widget(paragraph, area);

        return;
    }

    let text = format!("> {}", app.input);
    let paragraph = Paragraph::new(Line::raw(text)).block(block);

    frame.render_widget(paragraph, area);

    if app.focus == Focus::Input {
        let cursor_x = area.x + 4 + app.input.len() as u16;
        let cursor_y = area.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
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
