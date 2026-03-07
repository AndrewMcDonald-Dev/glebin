use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use glebin_protocol::{to_line, ServerMessage, WorldConfig};
use log::{debug, info};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{game::GameState, server::handle_client};

use super::{ServerCommand, ServerEvent};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub tick_rate_hz: u16,
    pub broadcast_channel_capacity: usize,
    pub world: WorldConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: 128,
            broadcast_channel_capacity: 256,
            world: WorldConfig::default(),
        }
    }
}

pub async fn run(listener: TcpListener) -> io::Result<()> {
    run_with_config(listener, ServerConfig::default()).await
}

async fn run_with_config(listener: TcpListener, config: ServerConfig) -> io::Result<()> {
    let local_addr = listener.local_addr()?;
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<ServerCommand>();
    let (message_tx, _) = broadcast::channel::<String>(config.broadcast_channel_capacity);
    let mut state = GameState::new(config.world.clone());
    let mut direct_message_txs = HashMap::<Uuid, mpsc::UnboundedSender<String>>::new();
    let mut audit_log = AuditLog::new()?;
    let mut ticker = time::interval(tick_duration(config.tick_rate_hz));
    let mut player_sequence: u64 = 0;
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    info!(
        "Server listening on {} at {} ticks/sec (world: {}x{})",
        local_addr, config.tick_rate_hz, config.world.width, config.world.height
    );

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (socket, peer_addr) = accept_result?;
                player_sequence = player_sequence.wrapping_add(1);
                let player_id = Uuid::new_v4();
                let player_glyph = player_glyph(player_sequence - 1);
                let player_color = player_color(player_sequence - 1);
                let player_name = default_player_name(player_glyph);
                let chat_history = state.chat_history();
                let (direct_tx, direct_rx) = mpsc::unbounded_channel();
                direct_message_txs.insert(player_id, direct_tx);
                debug!(
                    "Accepted connection from {} as {} ({}, {}, color {})",
                    peer_addr,
                    player_id,
                    player_name,
                    player_glyph,
                    player_color
                );
                tokio::spawn(handle_client(
                    socket,
                    command_tx.clone(),
                    message_tx.subscribe(),
                    direct_rx,
                    chat_history,
                    config.tick_rate_hz,
                    config.world.clone(),
                    player_id,
                    player_glyph,
                    player_color,
                    player_name,
                ));
            }
            _ = ticker.tick() => {
                while let Ok(command) = command_rx.try_recv() {
                    if matches!(command, ServerCommand::Disconnect { player_id: _ }) {
                        if let ServerCommand::Disconnect { player_id } = command.clone() {
                            direct_message_txs.remove(&player_id);
                        }
                    }

                    for event in state.apply(command) {
                        handle_event(
                            &message_tx,
                            &direct_message_txs,
                            &mut audit_log,
                            event,
                        )?;
                    }
                }

                if message_tx.receiver_count() == 0 {
                    continue;
                }

                let snapshot = state.snapshot();
                broadcast_message(&message_tx, &ServerMessage::Snapshot { snapshot })?;
            }
        }
    }
}

fn handle_event(
    message_tx: &broadcast::Sender<String>,
    direct_message_txs: &HashMap<Uuid, mpsc::UnboundedSender<String>>,
    audit_log: &mut AuditLog,
    event: ServerEvent,
) -> io::Result<()> {
    match event {
        ServerEvent::Broadcast(message) => broadcast_message(message_tx, &message),
        ServerEvent::Direct { player_id, message } => {
            let payload = encode_message(&message)?;
            if let Some(direct_tx) = direct_message_txs.get(&player_id) {
                let _ = direct_tx.send(payload);
            }
            Ok(())
        }
        ServerEvent::Audit(line) => audit_log.write_line(&line),
    }
}

fn broadcast_message(
    message_tx: &broadcast::Sender<String>,
    message: &ServerMessage,
) -> io::Result<()> {
    let payload = encode_message(message)?;
    let _ = message_tx.send(payload);
    Ok(())
}

fn encode_message(message: &ServerMessage) -> io::Result<String> {
    to_line(message).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn tick_duration(tick_rate_hz: u16) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(tick_rate_hz.max(1)))
}

fn player_glyph(index: u64) -> char {
    const GLYPHS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz";
    GLYPHS
        .get(index as usize % GLYPHS.len())
        .copied()
        .map(char::from)
        .unwrap_or('@')
}

fn default_player_name(glyph: char) -> String {
    format!("Pilot-{glyph}")
}

fn player_color(index: u64) -> u8 {
    // Walk the brighter portion of the 256-color cube to keep names and borders readable.
    let index = index % 125;
    let r = ((index / 25) % 5) + 1;
    let g = ((index / 5) % 5) + 1;
    let b = (index % 5) + 1;
    (16 + (36 * r) + (6 * g) + b) as u8
}

struct AuditLog {
    file: File,
    path: PathBuf,
}

impl AuditLog {
    fn new() -> io::Result<Self> {
        let path = audit_log_path();
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)?;
        info!("Chat audit log: {}", path.display());
        Ok(Self { file, path })
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.file, "{line}")?;
        self.file.flush()
    }
}

impl Drop for AuditLog {
    fn drop(&mut self) {
        let _ = self.file.flush();
        debug!("Closed chat audit log {}", self.path.display());
    }
}

fn audit_log_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    std::env::temp_dir().join(format!(
        "glebin-chat-{}-{timestamp}.log",
        std::process::id()
    ))
}
