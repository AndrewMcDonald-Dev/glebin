use std::{io, time::Duration};

use glebin_protocol::{to_line, ServerMessage, WorldConfig};
use log::{debug, info};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{game::GameState, server::handle_client};

use super::ServerCommand;

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
                let player_name = default_player_name(player_glyph);
                debug!(
                    "Accepted connection from {} as {} ({}, {})",
                    peer_addr,
                    player_id,
                    player_name,
                    player_glyph
                );
                tokio::spawn(handle_client(
                    socket,
                    command_tx.clone(),
                    message_tx.subscribe(),
                    config.tick_rate_hz,
                    config.world.clone(),
                    player_id,
                    player_glyph,
                    player_name,
                ));
            }
            _ = ticker.tick() => {
                while let Ok(command) = command_rx.try_recv() {
                    for message in state.apply(command) {
                        broadcast_message(&message_tx, &message)?;
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

fn broadcast_message(
    message_tx: &broadcast::Sender<String>,
    message: &ServerMessage,
) -> io::Result<()> {
    let payload =
        to_line(message).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let _ = message_tx.send(payload);
    Ok(())
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
