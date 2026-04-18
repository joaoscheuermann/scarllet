//! `scarllet-tui` binary entry point.
//!
//! Boots the crossterm event loop, spawns the connection task that
//! dials core, and pumps `SessionDiff`s into the in-memory [`App`]
//! mirror. All per-session state lives in core; the TUI is a thin
//! projection of the diff stream.

mod app;
mod connection;
mod events;
mod git_info;
mod input;
mod render;
mod widgets;

use std::time::Duration;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use tokio::sync::mpsc;

use scarllet_proto::proto::SessionDiff;

use app::{App, CoreCommand, Route};

/// Scarllet TUI command-line arguments.
#[derive(Parser, Debug, Clone)]
#[command(name = "scarllet-tui", about = "Scarllet terminal interface")]
struct Args {
    /// Attach to an existing session by id instead of auto-creating a new
    /// one. When the supplied id does not exist on core, the TUI falls
    /// back to the default auto-create behaviour and surfaces a status
    /// message informing the user.
    #[arg(long)]
    session: Option<String>,
}

/// Entry point for the Scarllet TUI process.
///
/// Sets up the terminal in raw mode, spawns a background task that connects
/// to the Core orchestrator via gRPC, then runs the main event loop that
/// multiplexes session diffs and terminal key/paste events until the user
/// quits.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

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

    let (diff_tx, mut diff_rx) = mpsc::channel::<SessionDiff>(256);
    let (command_tx, command_rx) = mpsc::channel::<CoreCommand>(64);

    let requested_session = args.session.clone();
    tokio::spawn(async move {
        connection::connect_and_stream(diff_tx, command_rx, requested_session).await;
    });

    let cwd = std::env::current_dir().unwrap_or_default();
    let debug_enabled = std::env::var("SCARLLET_DEBUG")
        .map(|v| v == "true")
        .unwrap_or(false);
    let mut app = App::new(command_tx, cwd, debug_enabled);

    loop {
        loop {
            match diff_rx.try_recv() {
                Ok(diff) => events::handle_session_diff(&mut app, diff),
                Err(mpsc::error::TryRecvError::Disconnected) if !app.stream_closed => {
                    app.stream_closed = true;
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
                                let _ = app.command_tx.try_send(CoreCommand::DestroyAndRecreate);
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
                        Event::Mouse(_) => continue,
                        _ => {}
                    }
                }
            }

            if should_exit {
                break;
            }
        }

        // Force chat route once a session is bound, even if Attached arrived
        // out of band (e.g. on Ctrl-N during the connecting splash).
        if app.session_id.is_some() && matches!(app.route, Route::Connecting { .. }) {
            app.route = Route::Chat;
        }

        app.advance_tick();
    }

    crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste).ok();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags
    )
    .ok();
    ratatui::restore();
    Ok(())
}
