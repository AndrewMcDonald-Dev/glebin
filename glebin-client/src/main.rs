mod app;
mod input;
mod network;
mod ui;

use std::{error::Error, io::stdout, time::Duration};

use app::App;
use clap::Parser;
use crossterm::{
    event::{Event, EventStream, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use glebin_protocol::{to_line, MAX_NAME_LEN};
use input::{handle_key, InputAction};
use network::AppEvent;
use ratatui::prelude::*;
use tokio::{
    io::AsyncWriteExt,
    time::{self, MissedTickBehavior},
};

#[derive(Debug, Parser)]
#[command(
    name = "glebin-client",
    about = "Terminal UI client for the Glebin tick server"
)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9132")]
    connect: String,
    #[arg(short, long, default_value_t = default_name())]
    name: String,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self, Box<dyn Error>> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let requested_name = sanitize_requested_name(&args.name);
    let (mut writer, mut app_rx, mut snapshot_rx) =
        network::connect(&args.connect, requested_name.clone()).await?;

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut events = EventStream::new();
    let mut redraw = time::interval(Duration::from_millis(33));
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut app = App::new(args.connect, requested_name);

    loop {
        tokio::select! {
            app_event = app_rx.recv() => {
                match app_event {
                    Some(AppEvent::Server(message)) => app.apply(message),
                    Some(AppEvent::Disconnected(reason)) => {
                        app.mark_disconnected(reason);
                        terminal.draw(|frame| ui::render(frame, &app))?;
                        break;
                    }
                    None => break,
                }
            }
            snapshot_result = snapshot_rx.changed() => {
                if snapshot_result.is_err() {
                    app.mark_disconnected("Snapshot stream closed".to_string());
                    break;
                }
                let snapshot = snapshot_rx.borrow_and_update().clone();
                if let Some(snapshot) = snapshot {
                    app.apply_snapshot(snapshot);
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        match handle_key(key, &mut app) {
                            InputAction::Continue => {}
                            InputAction::Quit => break,
                            InputAction::Send(message) => {
                                let payload = to_line(&message)?;
                                writer.write_all(payload.as_bytes()).await?;
                            }
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) | Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        app.mark_disconnected(format!("Terminal event error: {error}"));
                        terminal.draw(|frame| ui::render(frame, &app))?;
                        break;
                    }
                    None => {
                        app.mark_disconnected("Terminal event stream closed".to_string());
                        terminal.draw(|frame| ui::render(frame, &app))?;
                        break;
                    }
                }
            }
            _ = redraw.tick() => {
                terminal.draw(|frame| ui::render(frame, &app))?;
            }
        }
    }

    terminal.show_cursor()?;
    Ok(())
}

fn sanitize_requested_name(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() && !ch.is_whitespace())
        .take(MAX_NAME_LEN)
        .collect()
}

fn default_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "guest".to_string())
}
