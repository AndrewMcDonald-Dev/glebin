use std::{io, sync::Arc, time::Duration};

use futures_util::StreamExt;
use glebin_protocol::{
    to_line, ChatMessage, ClientMessage, ServerMessage, WorldConfig, MAX_CLIENT_LINE_LEN,
    PROTOCOL_VERSION,
};
use log::{debug, info, warn};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{broadcast, mpsc, watch},
    time::{self, MissedTickBehavior},
};
use tokio_util::codec::{FramedRead, LinesCodec};
use uuid::Uuid;

use super::ServerCommand;

#[allow(clippy::too_many_arguments)]
pub async fn handle_client(
    socket: TcpStream,
    command_tx: mpsc::Sender<ServerCommand>,
    mut message_rx: broadcast::Receiver<Arc<str>>,
    mut snapshot_rx: watch::Receiver<Arc<str>>,
    mut direct_rx: mpsc::Receiver<Arc<str>>,
    chat_history: Vec<ChatMessage>,
    tick_rate_hz: u16,
    world: WorldConfig,
    max_messages_per_second: u16,
    player_id: Uuid,
    player_glyph: char,
    player_color: u8,
    player_name: String,
) {
    let result = run_client_loop(
        socket,
        &command_tx,
        &mut message_rx,
        &mut snapshot_rx,
        &mut direct_rx,
        chat_history,
        tick_rate_hz,
        world,
        max_messages_per_second,
        player_id,
        player_glyph,
        player_color,
        &player_name,
    )
    .await;

    // This cleanup intentionally lives outside the fallible client loop so socket read/write
    // errors cannot leave a ghost player behind.
    let _ = command_tx
        .send(ServerCommand::Disconnect { player_id })
        .await;
    info!("Player {player_id} disconnected");
    if let Err(error) = result {
        warn!("Client loop for {player_id} exited with error: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_client_loop(
    socket: TcpStream,
    command_tx: &mpsc::Sender<ServerCommand>,
    message_rx: &mut broadcast::Receiver<Arc<str>>,
    snapshot_rx: &mut watch::Receiver<Arc<str>>,
    direct_rx: &mut mpsc::Receiver<Arc<str>>,
    chat_history: Vec<ChatMessage>,
    tick_rate_hz: u16,
    world: WorldConfig,
    max_messages_per_second: u16,
    player_id: Uuid,
    player_glyph: char,
    player_color: u8,
    player_name: &str,
) -> io::Result<()> {
    let (reader, mut writer) = socket.into_split();
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_CLIENT_LINE_LEN));
    let mut rate_limit_reset = time::interval(Duration::from_secs(1));
    rate_limit_reset.set_missed_tick_behavior(MissedTickBehavior::Skip);
    rate_limit_reset.tick().await;
    let mut messages_this_second = 0u16;

    enqueue(
        command_tx,
        ServerCommand::Connect {
            player_id,
            glyph: player_glyph,
            ui_color: player_color,
            name: player_name.to_string(),
        },
    )
    .await?;
    write_message(
        &mut writer,
        &ServerMessage::Welcome {
            protocol_version: PROTOCOL_VERSION,
            player_id,
            player_glyph,
            player_name: player_name.to_string(),
            player_color,
            tick_rate_hz,
            world,
        },
    )
    .await?;
    for message in chat_history {
        write_message(&mut writer, &ServerMessage::Chat { message }).await?;
    }
    let initial_snapshot = snapshot_rx.borrow_and_update().clone();
    writer.write_all(initial_snapshot.as_bytes()).await?;
    info!("Player {player_id} connected as {player_name} ({player_glyph}, color {player_color})");

    loop {
        tokio::select! {
            _ = rate_limit_reset.tick() => messages_this_second = 0,
            line_result = lines.next() => {
                let Some(line_result) = line_result else {
                    return Ok(());
                };
                let line = line_result.map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("invalid client frame: {error}"))
                })?;
                messages_this_second = messages_this_second.saturating_add(1);
                if messages_this_second > max_messages_per_second {
                    write_message(
                        &mut writer,
                        &ServerMessage::Error {
                            message: "client message rate limit exceeded".to_string(),
                        },
                    ).await?;
                    continue;
                }

                debug!("Received from {player_id}: {line}");
                match serde_json::from_str::<ClientMessage>(&line) {
                    Ok(ClientMessage::Move { dx, dy }) => {
                        enqueue(command_tx, ServerCommand::Move { player_id, dx, dy }).await?;
                    }
                    Ok(ClientMessage::SetName { name }) => {
                        enqueue(command_tx, ServerCommand::SetName { player_id, name }).await?;
                    }
                    Ok(ClientMessage::SendChat { text }) => {
                        enqueue(command_tx, ServerCommand::SendChat { player_id, text }).await?;
                    }
                    Err(error) => {
                        warn!("Invalid client message from {player_id}: {error}");
                        write_message(
                            &mut writer,
                            &ServerMessage::Error {
                                message: "invalid client message".to_string(),
                            },
                        ).await?;
                    }
                }
            }
            message_result = message_rx.recv() => {
                match message_result {
                    Ok(message) => writer.write_all(message.as_bytes()).await?,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Player {player_id} lagged behind by {skipped} event messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            snapshot_result = snapshot_rx.changed() => {
                if snapshot_result.is_err() {
                    return Ok(());
                }
                let snapshot = snapshot_rx.borrow_and_update().clone();
                writer.write_all(snapshot.as_bytes()).await?;
            }
            direct_message = direct_rx.recv() => {
                match direct_message {
                    Some(message) => writer.write_all(message.as_bytes()).await?,
                    None => return Ok(()),
                }
            }
        }
    }
}

async fn enqueue(
    command_tx: &mpsc::Sender<ServerCommand>,
    command: ServerCommand,
) -> io::Result<()> {
    command_tx
        .send(command)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "server command channel closed"))
}

async fn write_message<W>(writer: &mut W, message: &ServerMessage) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded =
        to_line(message).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(encoded.as_bytes()).await
}
