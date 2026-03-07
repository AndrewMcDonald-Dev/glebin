use std::{
    collections::HashMap,
    error::Error,
    io::stdout,
    time::{Duration, Instant},
};

use clap::Parser;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use glebin_protocol::{
    to_line, ChatKind, ChatMessage, ClientMessage, CollectibleState, PlayerState, Point,
    ServerMessage, Snapshot, WorldConfig,
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::mpsc,
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;

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

#[derive(Debug)]
enum AppEvent {
    Server(ServerMessage),
    Disconnected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Navigate,
    Chat,
}

#[derive(Debug, Clone)]
struct VisualPlayer {
    state: PlayerState,
    from: (f32, f32),
    to: (f32, f32),
    started_at: Instant,
    duration: Duration,
}

impl VisualPlayer {
    fn new(state: PlayerState, now: Instant) -> Self {
        let x = f32::from(state.position.x);
        let y = f32::from(state.position.y);
        Self {
            state,
            from: (x, y),
            to: (x, y),
            started_at: now,
            duration: Duration::from_millis(1),
        }
    }

    fn update(&mut self, state: PlayerState, now: Instant, duration: Duration) {
        let current = self.current_position(now);
        self.from = current;
        self.to = (f32::from(state.position.x), f32::from(state.position.y));
        self.started_at = now;
        self.duration = duration.max(Duration::from_millis(1));
        self.state = state;
    }

    fn current_position(&self, now: Instant) -> (f32, f32) {
        let elapsed = now.saturating_duration_since(self.started_at);
        let duration = self.duration.max(Duration::from_millis(1));
        let progress = (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
        (
            self.from.0 + (self.to.0 - self.from.0) * progress,
            self.from.1 + (self.to.1 - self.from.1) * progress,
        )
    }
}

#[derive(Debug)]
struct App {
    server_addr: String,
    requested_name: String,
    player_id: Option<Uuid>,
    welcome_color: Option<u8>,
    tick_rate_hz: u16,
    world: WorldConfig,
    tick: u64,
    visuals: HashMap<Uuid, VisualPlayer>,
    collectibles: Vec<CollectibleState>,
    chat_log: Vec<ChatMessage>,
    input_mode: InputMode,
    chat_input: String,
    status: String,
    last_snapshot_at: Option<Instant>,
}

impl App {
    fn new(server_addr: String, requested_name: String) -> Self {
        Self {
            server_addr,
            requested_name,
            player_id: None,
            welcome_color: None,
            tick_rate_hz: 0,
            world: WorldConfig::default(),
            tick: 0,
            visuals: HashMap::new(),
            collectibles: Vec::new(),
            chat_log: Vec::new(),
            input_mode: InputMode::Navigate,
            chat_input: String::new(),
            status: "Connecting...".to_string(),
            last_snapshot_at: None,
        }
    }

    fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome {
                player_id,
                player_glyph,
                player_name,
                player_color,
                tick_rate_hz,
                world,
            } => {
                self.player_id = Some(player_id);
                self.welcome_color = Some(player_color);
                self.tick_rate_hz = tick_rate_hz;
                self.world = world;
                self.status = format!(
                    "Connected as {player_name} ({player_glyph}) to {}",
                    self.server_addr
                );
            }
            ServerMessage::Snapshot { snapshot } => self.apply_snapshot(snapshot),
            ServerMessage::Chat { message } => self.push_chat(message),
            ServerMessage::Error { message } => {
                self.status = format!("Server error: {message}");
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let now = Instant::now();
        let duration = self
            .last_snapshot_at
            .map(|previous| clamp_duration(now.saturating_duration_since(previous)))
            .unwrap_or_else(|| Duration::from_millis(45));
        self.last_snapshot_at = Some(now);
        self.tick = snapshot.tick;
        self.collectibles = snapshot.collectibles;

        let mut next_visuals = HashMap::new();
        for (player_id, player) in snapshot.players {
            if let Some(mut visual) = self.visuals.remove(&player_id) {
                visual.update(player, now, duration);
                next_visuals.insert(player_id, visual);
            } else {
                next_visuals.insert(player_id, VisualPlayer::new(player, now));
            }
        }
        self.visuals = next_visuals;
    }

    fn push_chat(&mut self, message: ChatMessage) {
        self.status = match message.kind {
            ChatKind::Player => format!("{}: {}", message.from, message.text),
            ChatKind::System => message.text.clone(),
            ChatKind::Whisper => format!(
                "{} -> {}: {}",
                message.from,
                message.to.as_deref().unwrap_or("?"),
                message.text
            ),
        };
        self.chat_log.push(message);
        if self.chat_log.len() > 120 {
            let drain = self.chat_log.len() - 120;
            self.chat_log.drain(0..drain);
        }
    }

    fn mark_disconnected(&mut self, reason: String) {
        self.status = reason;
    }

    fn local_player(&self) -> Option<&PlayerState> {
        self.player_id
            .and_then(|player_id| self.visuals.get(&player_id))
            .map(|visual| &visual.state)
    }

    fn focus_position(&self, now: Instant) -> (f32, f32) {
        self.player_id
            .and_then(|player_id| self.visuals.get(&player_id))
            .map(|visual| visual.current_position(now))
            .unwrap_or_else(|| {
                (
                    f32::from(self.world.width.saturating_sub(1)) / 2.0,
                    f32::from(self.world.height.saturating_sub(1)) / 2.0,
                )
            })
    }

    fn theme_color(&self) -> Color {
        self.player_id
            .and_then(|player_id| self.visuals.get(&player_id))
            .map(|visual| indexed_color(visual.state.ui_color))
            .or_else(|| self.welcome_color.map(indexed_color))
            .unwrap_or(Color::Cyan)
    }
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
    let stream = TcpStream::connect(&args.connect).await?;
    let (reader, mut writer) = stream.into_split();
    let (app_tx, mut app_rx) = mpsc::unbounded_channel::<AppEvent>();

    let requested_name = sanitize_client_text(&args.name);
    let name_payload = to_line(&ClientMessage::SetName {
        name: requested_name.clone(),
    })?;
    writer.write_all(name_payload.as_bytes()).await?;

    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match serde_json::from_str::<ServerMessage>(&line) {
                    Ok(message) => {
                        let _ = app_tx.send(AppEvent::Server(message));
                    }
                    Err(error) => {
                        let _ = app_tx.send(AppEvent::Disconnected(format!(
                            "Invalid server message: {error}"
                        )));
                        return;
                    }
                },
                Ok(None) => {
                    let _ = app_tx.send(AppEvent::Disconnected(
                        "Server closed the connection".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _ =
                        app_tx.send(AppEvent::Disconnected(format!("Connection error: {error}")));
                    return;
                }
            }
        }
    });

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut events = EventStream::new();
    let mut redraw = time::interval(Duration::from_millis(33));
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut app = App::new(args.connect, requested_name);

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        tokio::select! {
            Some(app_event) = app_rx.recv() => {
                match app_event {
                    AppEvent::Server(message) => app.apply(message),
                    AppEvent::Disconnected(reason) => {
                        app.mark_disconnected(reason);
                        terminal.draw(|frame| render(frame, &app))?;
                        break;
                    }
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        if !handle_key(key, &mut writer, &mut app).await? {
                            break;
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        app.mark_disconnected(format!("Terminal event error: {error}"));
                        terminal.draw(|frame| render(frame, &app))?;
                        break;
                    }
                    None => {
                        app.mark_disconnected("Terminal event stream closed".to_string());
                        terminal.draw(|frame| render(frame, &app))?;
                        break;
                    }
                }
            }
            _ = redraw.tick() => {}
        }
    }

    terminal.show_cursor()?;
    Ok(())
}

async fn handle_key(
    key: KeyEvent,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    app: &mut App,
) -> Result<bool, Box<dyn Error>> {
    if app.input_mode == InputMode::Chat {
        match key.code {
            KeyCode::Esc => {
                app.input_mode = InputMode::Navigate;
                app.status = "Chat cancelled".to_string();
            }
            KeyCode::Enter => {
                let text = sanitize_client_text(&app.chat_input);
                if !text.is_empty() {
                    let payload = to_line(&ClientMessage::SendChat { text: text.clone() })?;
                    writer.write_all(payload.as_bytes()).await?;
                    app.status = format!("You: {text}");
                }
                app.chat_input.clear();
                app.input_mode = InputMode::Navigate;
            }
            KeyCode::Backspace => {
                app.chat_input.pop();
            }
            KeyCode::Char(character) => {
                if app.chat_input.chars().count() < 120 {
                    app.chat_input.push(character);
                }
            }
            _ => {}
        }
        return Ok(true);
    }

    let movement = match key.code {
        KeyCode::Char('q') => return Ok(false),
        KeyCode::Enter | KeyCode::Char('/') | KeyCode::Char('c') => {
            app.input_mode = InputMode::Chat;
            app.status = "Chat mode".to_string();
            return Ok(true);
        }
        KeyCode::Up | KeyCode::Char('w') => Some((0, -1)),
        KeyCode::Down | KeyCode::Char('s') => Some((0, 1)),
        KeyCode::Left | KeyCode::Char('a') => Some((-1, 0)),
        KeyCode::Right | KeyCode::Char('d') => Some((1, 0)),
        _ => None,
    };

    if let Some((dx, dy)) = movement {
        let payload = to_line(&ClientMessage::Move { dx, dy })?;
        writer.write_all(payload.as_bytes()).await?;
        app.status = format!("Moved by ({dx}, {dy})");
    }

    Ok(true)
}

fn render(frame: &mut Frame, app: &App) {
    let now = Instant::now();
    let theme = app.theme_color();
    let base_style = panel_style();
    frame.render_widget(Block::default().style(base_style), frame.area());
    let outer = Layout::vertical([
        Constraint::Min(10),
        Constraint::Length(9),
        Constraint::Length(3),
    ])
    .split(frame.area());
    let main = Layout::horizontal([Constraint::Min(20), Constraint::Length(32)]).split(outer[0]);
    let sidebar = Layout::vertical([
        Constraint::Min(8),
        Constraint::Length(8),
        Constraint::Length(6),
    ])
    .split(main[1]);

    let world_block = Block::default()
        .title(format!(
            " World {}x{} | Tick {} ",
            app.world.width, app.world.height, app.tick
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme));
    let world = Paragraph::new(build_world_lines(app, now, world_block.inner(main[0])))
        .style(base_style)
        .block(world_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(world, main[0]);

    let players = Paragraph::new(build_player_lines(app))
        .style(base_style)
        .block(
            Block::default()
                .title(" Players ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(players, sidebar[0]);

    let legend = Paragraph::new(build_legend_lines(app))
        .style(base_style)
        .block(
            Block::default()
                .title(" World Notes ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(legend, sidebar[1]);

    let help = Paragraph::new(build_help_lines(app))
        .style(base_style)
        .block(
            Block::default()
                .title(" Controls ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(help, sidebar[2]);

    let chat = Paragraph::new(build_chat_lines(app))
        .style(base_style)
        .block(
            Block::default()
                .title(" Chat ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(chat, outer[1]);

    let prompt = match app.input_mode {
        InputMode::Navigate => "Press Enter, /, or c to chat",
        InputMode::Chat => "Send message (Enter to send, Esc to cancel)",
    };
    let input = Paragraph::new(Line::from(vec![
        Span::styled("> ", base_style.fg(theme).add_modifier(Modifier::BOLD)),
        Span::raw(app.chat_input.clone()),
    ]))
    .style(base_style)
    .block(
        Block::default()
            .title(format!(" {} | {} ", prompt, app.status))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme)),
    );
    frame.render_widget(input, outer[2]);
}

fn build_world_lines(app: &App, now: Instant, area: Rect) -> Vec<Line<'static>> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let width = usize::from(app.world.width.max(1));
    let height = usize::from(app.world.height.max(1));
    let mut cells = vec![vec![CellKind::Empty; width]; height];

    for feature in &app.world.features {
        let x = usize::from(feature.position.x.min(app.world.width.saturating_sub(1)));
        let y = usize::from(feature.position.y.min(app.world.height.saturating_sub(1)));
        if y < height && x < width {
            cells[y][x] = CellKind::Feature(feature.glyph, feature.solid);
        }
    }

    for collectible in &app.collectibles {
        let x = usize::from(
            collectible
                .position
                .x
                .min(app.world.width.saturating_sub(1)),
        );
        let y = usize::from(
            collectible
                .position
                .y
                .min(app.world.height.saturating_sub(1)),
        );
        if y < height && x < width {
            cells[y][x] = CellKind::Collectible(collectible.glyph);
        }
    }

    let mut players = app
        .visuals
        .iter()
        .map(|(player_id, visual)| {
            let (x, y) = visual.current_position(now);
            (
                *player_id,
                visual.state.clone(),
                Point::new(
                    x.round()
                        .clamp(0.0, f32::from(app.world.width.saturating_sub(1)))
                        as u16,
                    y.round()
                        .clamp(0.0, f32::from(app.world.height.saturating_sub(1)))
                        as u16,
                ),
            )
        })
        .collect::<Vec<_>>();
    players.sort_by_key(|(player_id, state, _)| {
        (
            Some(*player_id) == app.player_id,
            state.score,
            state.name.clone(),
        )
    });

    for (_player_id, state, position) in players {
        let x = usize::from(position.x);
        let y = usize::from(position.y);
        if y < height && x < width {
            cells[y][x] = CellKind::Player {
                glyph: state.glyph,
                ui_color: state.ui_color,
            };
        }
    }

    let viewport = WorldViewport::centered_on(app.focus_position(now), area.width, area.height);
    let world_width = i32::from(app.world.width);
    let world_height = i32::from(app.world.height);

    (0..area.height)
        .map(|row| {
            let world_y = viewport.top + i32::from(row);
            let spans = (0..area.width)
                .map(|column| {
                    let world_x = viewport.left + i32::from(column);
                    if world_x < 0
                        || world_y < 0
                        || world_x >= world_width
                        || world_y >= world_height
                    {
                        Span::styled(" ", Style::default().bg(interface_bg()))
                    } else {
                        cell_span(cells[world_y as usize][world_x as usize])
                    }
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

fn build_player_lines(app: &App) -> Vec<Line<'static>> {
    let mut players = app
        .visuals
        .iter()
        .map(|(player_id, visual)| (*player_id, visual.state.clone()))
        .collect::<Vec<_>>();
    players.sort_by(|left, right| {
        right
            .1
            .score
            .cmp(&left.1.score)
            .then_with(|| left.1.name.cmp(&right.1.name))
            .then_with(|| left.0.cmp(&right.0))
    });

    if players.is_empty() {
        return vec![Line::from("Waiting for players...")];
    }

    players
        .into_iter()
        .map(|(player_id, player)| {
            let marker = if Some(player_id) == app.player_id {
                ">"
            } else {
                " "
            };
            let player_style = Style::default()
                .fg(indexed_color(player.ui_color))
                .bg(interface_bg())
                .add_modifier(Modifier::BOLD);
            let meta_style = Style::default().fg(Color::Gray).bg(interface_bg());
            Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    if Some(player_id) == app.player_id {
                        player_style
                    } else {
                        Style::default().bg(interface_bg())
                    },
                ),
                Span::styled(format!("{} ", player.glyph), player_style),
                Span::styled(player.name, player_style),
                Span::styled(
                    format!(
                        " ({}, {}) [{}]",
                        player.position.x, player.position.y, player.score
                    ),
                    meta_style,
                ),
            ])
        })
        .collect()
}

fn build_legend_lines(app: &App) -> Vec<Line<'static>> {
    let local = app.local_player();
    vec![
        Line::from(format!(
            "You asked for: {}",
            if app.requested_name.is_empty() {
                "guest".to_string()
            } else {
                app.requested_name.clone()
            }
        )),
        Line::from(match local {
            Some(player) => format!("Identity: {} {}", player.glyph, player.name),
            None => "Identity: waiting for welcome".to_string(),
        }),
        Line::from(match local {
            Some(player) => format!("Score: {}", player.score),
            None => "Score: 0".to_string(),
        }),
        Line::from(format!("Collectibles: {} active", app.collectibles.len())),
        Line::from("# /^ solid terrain"),
        Line::from("~ / L scenic props"),
        Line::from("* star shard (+1)"),
    ]
}

fn build_help_lines(app: &App) -> Vec<Line<'static>> {
    match app.input_mode {
        InputMode::Navigate => vec![
            Line::from("Move: arrows or WASD"),
            Line::from("Chat: Enter, /, or c"),
            Line::from("Quit: q"),
            Line::from(format!("Server: {}", app.server_addr)),
        ],
        InputMode::Chat => vec![
            Line::from("Type your message"),
            Line::from("Send: Enter"),
            Line::from("Whisper: /w <name> <msg>"),
            Line::from("Reply: /r <msg>"),
            Line::from("Cancel: Esc"),
            Line::from("Movement paused while chatting"),
        ],
    }
}

fn build_chat_lines(app: &App) -> Vec<Line<'static>> {
    if app.chat_log.is_empty() {
        return vec![Line::from("No messages yet.")];
    }

    let available = 7usize;
    let start = app.chat_log.len().saturating_sub(available);
    app.chat_log[start..]
        .iter()
        .map(|message| {
            let (prefix, prefix_style, text_style) = match message.kind {
                ChatKind::Player => {
                    let color = message.ui_color.map(indexed_color).unwrap_or(Color::White);
                    (
                        format!("{} {}:", message.glyph.unwrap_or('?'), message.from),
                        Style::default()
                            .fg(color)
                            .bg(interface_bg())
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::White).bg(interface_bg()),
                    )
                }
                ChatKind::Whisper => {
                    let color = message.ui_color.map(indexed_color).unwrap_or(Color::White);
                    (
                        format!(
                            "{} {} -> {}:",
                            message.glyph.unwrap_or('?'),
                            message.from,
                            message.to.as_deref().unwrap_or("?")
                        ),
                        Style::default()
                            .fg(color)
                            .bg(interface_bg())
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::LightMagenta).bg(interface_bg()),
                    )
                }
                ChatKind::System => (
                    "system:".to_string(),
                    Style::default()
                        .fg(Color::Blue)
                        .bg(interface_bg())
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(Color::Blue).bg(interface_bg()),
                ),
            };
            Line::from(vec![
                Span::styled(format!("{} ", prefix), prefix_style),
                Span::styled(message.text.clone(), text_style),
            ])
        })
        .collect()
}

fn interface_bg() -> Color {
    Color::Black
}

fn panel_style() -> Style {
    Style::default().fg(Color::White).bg(interface_bg())
}

fn sanitize_client_text(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(120)
        .collect()
}

fn clamp_duration(duration: Duration) -> Duration {
    duration.clamp(Duration::from_millis(25), Duration::from_millis(90))
}

fn default_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "guest".to_string())
}

fn indexed_color(index: u8) -> Color {
    Color::Indexed(index)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorldViewport {
    left: i32,
    top: i32,
}

impl WorldViewport {
    fn centered_on(focus: (f32, f32), width: u16, height: u16) -> Self {
        let focus_x = focus.0.round() as i32;
        let focus_y = focus.1.round() as i32;
        Self {
            left: focus_x - i32::from(width) / 2,
            top: focus_y - i32::from(height) / 2,
        }
    }
}

fn cell_span(cell: CellKind) -> Span<'static> {
    let background = interface_bg();
    match cell {
        CellKind::Empty => Span::styled(
            ".".to_string(),
            Style::default().fg(Color::DarkGray).bg(background),
        ),
        CellKind::Feature(glyph, true) => Span::styled(
            glyph.to_string(),
            Style::default()
                .fg(Color::Green)
                .bg(background)
                .add_modifier(Modifier::BOLD),
        ),
        CellKind::Feature(glyph, false) => Span::styled(
            glyph.to_string(),
            Style::default().fg(Color::Blue).bg(background),
        ),
        CellKind::Collectible(glyph) => Span::styled(
            glyph.to_string(),
            Style::default()
                .fg(Color::Magenta)
                .bg(background)
                .add_modifier(Modifier::BOLD),
        ),
        CellKind::Player { glyph, ui_color } => Span::styled(
            glyph.to_string(),
            Style::default()
                .fg(indexed_color(ui_color))
                .bg(background)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

#[derive(Clone, Copy)]
enum CellKind {
    Empty,
    Feature(char, bool),
    Collectible(char),
    Player { glyph: char, ui_color: u8 },
}
