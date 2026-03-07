use std::io;

use glebin_protocol::{to_line, ChatMessage, ClientMessage, ServerMessage, WorldConfig};
use log::{debug, info, warn};
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use uuid::Uuid;

use super::ServerCommand;

pub async fn handle_client(
    socket: TcpStream,
    command_tx: mpsc::UnboundedSender<ServerCommand>,
    mut message_rx: broadcast::Receiver<String>,
    mut direct_rx: mpsc::UnboundedReceiver<String>,
    chat_history: Vec<ChatMessage>,
    tick_rate_hz: u16,
    world: WorldConfig,
    player_id: Uuid,
    player_glyph: char,
    player_color: u8,
    player_name: String,
) {
    if let Err(error) = run_client_loop(
        socket,
        &command_tx,
        &mut message_rx,
        &mut direct_rx,
        chat_history,
        tick_rate_hz,
        world,
        player_id,
        player_glyph,
        player_color,
        &player_name,
    )
    .await
    {
        warn!("Client loop exited with error: {error}");
    }
}

async fn run_client_loop(
    socket: TcpStream,
    command_tx: &mpsc::UnboundedSender<ServerCommand>,
    message_rx: &mut broadcast::Receiver<String>,
    direct_rx: &mut mpsc::UnboundedReceiver<String>,
    chat_history: Vec<ChatMessage>,
    tick_rate_hz: u16,
    world: WorldConfig,
    player_id: Uuid,
    player_glyph: char,
    player_color: u8,
    player_name: &str,
) -> io::Result<()> {
    let (reader, mut writer) = socket.into_split();
    let mut lines = BufReader::new(reader).lines();

    enqueue(
        command_tx,
        ServerCommand::Connect {
            player_id,
            glyph: player_glyph,
            ui_color: player_color,
            name: player_name.to_string(),
        },
    )?;
    write_message(
        &mut writer,
        &ServerMessage::Welcome {
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
    info!("Player {player_id} connected as {player_name} ({player_glyph}, color {player_color})");

    let result = loop {
        tokio::select! {
            line_result = lines.next_line() => {
                match line_result? {
                    Some(line) => {
                        debug!("Received from {player_id}: {line}");
                        match serde_json::from_str::<ClientMessage>(&line) {
                            Ok(ClientMessage::Move { dx, dy }) => {
                                enqueue(command_tx, ServerCommand::Move { player_id, dx, dy })?;
                            }
                            Ok(ClientMessage::SetName { name }) => {
                                enqueue(command_tx, ServerCommand::SetName { player_id, name })?;
                            }
                            Ok(ClientMessage::SendChat { text }) => {
                                enqueue(command_tx, ServerCommand::SendChat { player_id, text })?;
                            }
                            Err(error) => {
                                warn!("Invalid client message from {player_id}: {error}");
                                write_message(
                                    &mut writer,
                                    &ServerMessage::Error {
                                        message: format!("invalid client message: {error}"),
                                    },
                                ).await?;
                            }
                        }
                    }
                    None => break Ok(()),
                }
            }
            message_result = message_rx.recv() => {
                match message_result {
                    Ok(message) => writer.write_all(message.as_bytes()).await?,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Player {player_id} lagged behind by {skipped} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break Ok(()),
                }
            }
            direct_message = direct_rx.recv() => {
                match direct_message {
                    Some(message) => writer.write_all(message.as_bytes()).await?,
                    None => break Ok(()),
                }
            }
        }
    };

    enqueue(command_tx, ServerCommand::Disconnect { player_id })?;
    info!("Player {player_id} disconnected");
    result
}

fn enqueue(
    command_tx: &mpsc::UnboundedSender<ServerCommand>,
    command: ServerCommand,
) -> io::Result<()> {
    command_tx
        .send(command)
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
