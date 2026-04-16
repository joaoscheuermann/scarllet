mod app;
mod connection;
mod events;
mod git_info;
mod input;
mod render;
mod widgets;

use std::time::Duration;

use crossterm::event::{self, Event};
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
        connection::connect_and_stream(event_tx, message_rx).await;
    });

    let cwd = std::env::current_dir().unwrap_or_default();
    let debug_enabled = std::env::var("SCARLLET_DEBUG")
        .map(|v| v == "true")
        .unwrap_or(false);
    let mut app = App::new(message_tx, cwd, debug_enabled);

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
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != crossterm::event::KeyEventKind::Press {
                        continue;
                    }
                    if events::handle_input(&mut app, key) {
                        break;
                    }
                }
                Event::Paste(text) => {
                    events::handle_paste(&mut app, &text);
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
