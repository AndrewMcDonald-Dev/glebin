use std::{
    collections::HashMap, fs::OpenOptions, future::Future, io, path::PathBuf, sync::Arc,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use glebin_protocol::{to_line, ServerMessage, WorldConfig};
use log::{debug, info, warn};
use tokio::{
    io::{AsyncWriteExt, BufWriter},
    net::TcpListener,
    sync::{broadcast, mpsc, watch},
    task::{JoinHandle, JoinSet},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{game::GameState, server::handle_client};

use super::{ServerCommand, ServerEvent};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub tick_rate_hz: u16,
    pub snapshot_rate_hz: u16,
    pub command_channel_capacity: usize,
    pub direct_message_channel_capacity: usize,
    pub broadcast_channel_capacity: usize,
    pub audit_channel_capacity: usize,
    pub max_commands_per_tick: usize,
    pub max_client_messages_per_second: u16,
    pub world: WorldConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: 128,
            snapshot_rate_hz: 20,
            command_channel_capacity: 1024,
            direct_message_channel_capacity: 64,
            broadcast_channel_capacity: 256,
            audit_channel_capacity: 512,
            max_commands_per_tick: 256,
            max_client_messages_per_second: 64,
            world: WorldConfig::default(),
        }
    }
}

impl ServerConfig {
    fn validate(&self) -> io::Result<()> {
        self.world
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        if self.tick_rate_hz == 0
            || self.snapshot_rate_hz == 0
            || self.max_client_messages_per_second == 0
            || self.command_channel_capacity == 0
            || self.direct_message_channel_capacity == 0
            || self.broadcast_channel_capacity == 0
            || self.audit_channel_capacity == 0
            || self.max_commands_per_tick == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "server rates, channel capacities, and command budget must be greater than zero",
            ));
        }
        Ok(())
    }
}

pub async fn run(listener: TcpListener) -> io::Result<()> {
    run_with_config(listener, ServerConfig::default()).await
}

pub async fn run_with_config(listener: TcpListener, config: ServerConfig) -> io::Result<()> {
    run_with_config_until(listener, config, std::future::pending()).await
}

pub async fn run_with_config_until<F>(
    listener: TcpListener,
    config: ServerConfig,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()>,
{
    config.validate()?;
    let local_addr = listener.local_addr()?;
    let (command_tx, mut command_rx) =
        mpsc::channel::<ServerCommand>(config.command_channel_capacity);
    let (message_tx, _) = broadcast::channel::<Arc<str>>(config.broadcast_channel_capacity);
    let mut state = GameState::new(config.world.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let initial_snapshot = encode_message(&ServerMessage::Snapshot {
        snapshot: state.snapshot(),
    })?;
    let (snapshot_tx, _) = watch::channel(initial_snapshot);
    let mut direct_message_txs = HashMap::<Uuid, mpsc::Sender<Arc<str>>>::new();
    let mut audit_log = AuditLog::new(config.audit_channel_capacity)?;
    let mut simulation_ticker = time::interval(tick_duration(config.tick_rate_hz));
    let mut snapshot_ticker = time::interval(tick_duration(config.snapshot_rate_hz));
    let mut player_sequence: u64 = 0;
    let mut client_tasks = JoinSet::new();
    tokio::pin!(shutdown);
    simulation_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    snapshot_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    info!(
        "Server listening on {} at {} ticks/sec and {} snapshots/sec (world: {}x{})",
        local_addr,
        config.tick_rate_hz,
        config.snapshot_rate_hz,
        config.world.width,
        config.world.height
    );

    let result = 'server: loop {
        tokio::select! {
            _ = &mut shutdown => break 'server Ok(()),
            accept_result = listener.accept() => {
                let (socket, peer_addr) = match accept_result {
                    Ok(connection) => connection,
                    Err(error) => break 'server Err(error),
                };
                player_sequence = player_sequence.wrapping_add(1);
                let player_id = Uuid::new_v4();
                let player_glyph = player_glyph(player_sequence - 1);
                let player_color = player_color(player_sequence - 1);
                let player_name = default_player_name(player_glyph);
                let chat_history = state.chat_history();
                let (direct_tx, direct_rx) =
                    mpsc::channel(config.direct_message_channel_capacity);
                direct_message_txs.insert(player_id, direct_tx);
                debug!(
                    "Accepted connection from {} as {} ({}, {}, color {})",
                    peer_addr,
                    player_id,
                    player_name,
                    player_glyph,
                    player_color
                );
                client_tasks.spawn(handle_client(
                    socket,
                    command_tx.clone(),
                    message_tx.subscribe(),
                    snapshot_tx.subscribe(),
                    direct_rx,
                    chat_history,
                    config.tick_rate_hz,
                    config.world.clone(),
                    config.max_client_messages_per_second,
                    player_id,
                    player_glyph,
                    player_color,
                    player_name,
                ));
            }
            _ = simulation_ticker.tick() => {
                for _ in 0..config.max_commands_per_tick {
                    let Ok(command) = command_rx.try_recv() else {
                        break;
                    };
                    if let ServerCommand::Disconnect { player_id } = &command {
                        direct_message_txs.remove(player_id);
                    }

                    for event in state.apply(command) {
                        if let Err(error) = handle_event(
                            &message_tx,
                            &direct_message_txs,
                            &audit_log,
                            event,
                        ).await {
                            break 'server Err(error);
                        }
                    }
                }
                state.advance_tick();
            }
            _ = snapshot_ticker.tick() => {
                if snapshot_tx.receiver_count() > 0 {
                    let snapshot = state.snapshot();
                    match encode_message(&ServerMessage::Snapshot { snapshot }) {
                        Ok(payload) => {
                            snapshot_tx.send_replace(payload);
                        }
                        Err(error) => break 'server Err(error),
                    }
                }
            }
            Some(task_result) = client_tasks.join_next(), if !client_tasks.is_empty() => {
                if let Err(error) = task_result {
                    warn!("Client task failed: {error}");
                }
            }
        }
    };

    client_tasks.abort_all();
    while client_tasks.join_next().await.is_some() {}
    drop(command_tx);
    audit_log.close().await?;
    result
}

async fn handle_event(
    message_tx: &broadcast::Sender<Arc<str>>,
    direct_message_txs: &HashMap<Uuid, mpsc::Sender<Arc<str>>>,
    audit_log: &AuditLog,
    event: ServerEvent,
) -> io::Result<()> {
    match event {
        ServerEvent::Broadcast(message) => broadcast_message(message_tx, &message),
        ServerEvent::Direct { player_id, message } => {
            let payload = encode_message(&message)?;
            if let Some(direct_tx) = direct_message_txs.get(&player_id) {
                if direct_tx.try_send(payload).is_err() {
                    warn!("Dropping direct message for slow player {player_id}");
                }
            }
            Ok(())
        }
        ServerEvent::Audit(line) => audit_log.write_line(line).await,
    }
}

fn broadcast_message(
    message_tx: &broadcast::Sender<Arc<str>>,
    message: &ServerMessage,
) -> io::Result<()> {
    let payload = encode_message(message)?;
    let _ = message_tx.send(payload);
    Ok(())
}

fn encode_message(message: &ServerMessage) -> io::Result<Arc<str>> {
    to_line(message)
        .map(Arc::<str>::from)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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
    let index = index % 125;
    let r = ((index / 25) % 5) + 1;
    let g = ((index / 5) % 5) + 1;
    let b = (index % 5) + 1;
    (16 + (36 * r) + (6 * g) + b) as u8
}

struct AuditLog {
    sender: Option<mpsc::Sender<String>>,
    task: JoinHandle<io::Result<()>>,
}

impl AuditLog {
    fn new(capacity: usize) -> io::Result<Self> {
        let path = audit_log_path();
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(&path)?;
        info!("Chat audit log: {}", path.display());

        let (sender, mut receiver) = mpsc::channel::<String>(capacity);
        let task = tokio::spawn(async move {
            let mut writer = BufWriter::new(tokio::fs::File::from_std(file));
            let mut flush_timer = time::interval(Duration::from_secs(1));
            flush_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    line = receiver.recv() => match line {
                        Some(line) => {
                            writer.write_all(line.as_bytes()).await?;
                            writer.write_all(b"\n").await?;
                        }
                        None => {
                            writer.flush().await?;
                            return Ok(());
                        }
                    },
                    _ = flush_timer.tick() => writer.flush().await?,
                }
            }
        });

        Ok(Self {
            sender: Some(sender),
            task,
        })
    }

    async fn write_line(&self, line: String) -> io::Result<()> {
        self.sender
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "audit log is closed"))?
            .send(line)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "audit log task stopped"))
    }

    async fn close(&mut self) -> io::Result<()> {
        self.sender.take();
        (&mut self.task)
            .await
            .map_err(|error| io::Error::other(format!("audit log task failed: {error}")))?
    }
}

fn audit_log_path() -> PathBuf {
    std::env::temp_dir().join(format!("glebin-chat-{}.log", Uuid::new_v4()))
}
