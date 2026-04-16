mod app;
mod connection;
mod events;
mod git_info;
mod input;
mod render;
mod session;
mod widgets;

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use tokio::sync::mpsc;

use scarllet_proto::proto::*;

use app::{App, ChatEntry, Route};

/// Entry point for the Scarllet TUI process.
///
/// Sets up the terminal in raw mode, spawns a background task that connects
/// to the Core orchestrator via gRPC, then runs the main event loop that
/// multiplexes Core events and terminal key/paste events until the user quits.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original_hook(info);
    }));

    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste).ok();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    )
    .ok();

    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(256);
    let (message_tx, message_rx) = mpsc::channel::<TuiMessage>(256);

    tokio::spawn(async move {
        connection::connect_and_stream(event_tx, message_rx).await;
    });

    let cwd = std::env::current_dir().unwrap_or_default();
    let debug_enabled = std::env::var("SCARLLET_DEBUG")
        .map(|v| v == "true")
        .unwrap_or(false);
    let session_repo: std::sync::Arc<dyn session::SessionRepository> =
        match session::FileSessionRepository::new() {
            Ok(repo) => std::sync::Arc::new(repo),
            Err(_) => std::sync::Arc::new(session::NullSessionRepository),
        };
    let mut app = App::new(message_tx, cwd, debug_enabled, session_repo);

    if let Ok(Some(s)) = app.session_repo.load() {
        app.load_from_session(s);
    }

    loop {
        loop {
            match event_rx.try_recv() {
                Ok(event) => events::handle_core_event(&mut app, event),
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

        terminal.draw(|f| render::routes(f, &mut app))?;

        let poll_ms = if app.is_streaming() { 50 } else { 200 };
        if event::poll(Duration::from_millis(poll_ms))? {
            let first = event::read()?;
            let mut batch = vec![first];
            while event::poll(Duration::from_millis(1))? {
                batch.push(event::read()?);
            }

            let all_keys = batch.iter().all(|e| matches!(e, Event::Key(_)));
            let is_paste_batch = batch.len() > 1 && all_keys && app.is_input_editable();

            let mut should_exit = false;

            if is_paste_batch {
                let mut paste_buf = String::new();
                for ev in &batch {
                    if let Event::Key(key) = ev {
                        if key.kind != crossterm::event::KeyEventKind::Press {
                            continue;
                        }
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            should_exit = true;
                            break;
                        }
                        match key.code {
                            KeyCode::Enter => paste_buf.push('\n'),
                            KeyCode::Char(c) => paste_buf.push(c),
                            KeyCode::Tab => paste_buf.push('\t'),
                            _ => {}
                        }
                    }
                }
                if !should_exit && !paste_buf.is_empty() {
                    events::handle_paste(&mut app, &paste_buf);
                }
            } else {
                for ev in batch {
                    match ev {
                        Event::Key(key) => {
                            if key.kind != crossterm::event::KeyEventKind::Press {
                                continue;
                            }
                            if key.code == KeyCode::Char('n')
                                && key.modifiers.contains(KeyModifiers::CONTROL)
                            {
                                app.save_session();
                                app.new_session();
                                continue;
                            }
                            if events::handle_input(&mut app, key) {
                                should_exit = true;
                                break;
                            }
                        }
                        Event::Paste(text) => {
                            events::handle_paste(&mut app, &text);
                        }
                        _ => {}
                    }
                }
            }

            if should_exit {
                app.save_session();
                break;
            }
        }

        app.advance_tick();
    }

    crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste).ok();
    crossterm::execute!(std::io::stdout(), crossterm::event::PopKeyboardEnhancementFlags).ok();
    ratatui::restore();
    Ok(())
}
