use std::time::Instant;

use glebin_protocol::{ChatKind, Point};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{App, InputMode};

fn theme_color(app: &App) -> Color {
    app.player_id
        .and_then(|player_id| app.visuals.get(&player_id))
        .map(|visual| indexed_color(visual.state.ui_color))
        .or_else(|| app.welcome_color.map(indexed_color))
        .unwrap_or(Color::Cyan)
}

pub fn render(frame: &mut Frame, app: &App) {
    let now = Instant::now();
    let theme = theme_color(app);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn viewport_centers_on_focus() {
        assert_eq!(
            WorldViewport::centered_on((10.0, 6.0), 8, 4),
            WorldViewport { left: 6, top: 4 }
        );
    }

    #[test]
    fn renders_on_a_small_terminal_without_panicking() {
        let backend = TestBackend::new(30, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::new("test".to_string(), "guest".to_string());
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }
}
