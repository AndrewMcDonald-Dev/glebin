use std::{collections::VecDeque, sync::Arc};

use log::{info, warn};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    sync::{broadcast, Mutex},
};
use uuid::Uuid;

use super::Message;

pub async fn handle_client(
    mut socket: TcpStream,
    msg_q: Arc<Mutex<VecDeque<Message>>>,
    tx: broadcast::Sender<String>,
) {
    // Generate new player_id
    let player_id = Uuid::new_v4();

    // Create AddPlayer message and add it to msg_q
    {
        let mut msg_q = msg_q.lock().await;
        msg_q.push_back(Message::AddPlayer { id: player_id });
    }
    info!("Player {} connected", player_id);

    let mut rx = tx.subscribe();
    loop {
        tokio::select! {
            result = socket.readable() => {
                if result.is_err() {
                    warn!("Socker readable error for player {}: {:?}", player_id, result);
                    break;
                }

                let mut buffer = vec![0; 128];
                match socket.try_read(&mut buffer) {
                    Ok(0) => {
                        info!("Player {} disconnected (client closed connection.)", player_id);
                        break;
                    }
                    Ok(n) => {
                        let message = String::from_utf8_lossy(&buffer[..n]);
                        info!("Received message from player {}: {}", player_id, message);
                        match serde_json::from_str::<(f32, f32)>(&message) {
                            Ok(pos) => {
                                let (x, y) = pos;
                                // Create UpdatePlayerPosition messages and add it to msg_q
                                {
                                    let mut msg_q = msg_q.lock().await;
                                    msg_q.push_back(Message::UpdatePlayerPosition { id: player_id, x, y });
                                }
                                info!("Updated position for player {}: ({}, {})", player_id, x, y);
                            },
                            Err(e) => {
                                warn!("Failed to parse message from player {}: {} - Error: {:?}", player_id, message, e);
                                continue;
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(e) => {
                        warn!("Socker read error for player {}: {:?}", player_id, e);
                        break;
                    }
                }

            },
            // This runs whenever tx is used to send a message to receivers.
            Ok(update) = rx.recv() => {
                if let Err(e) = socket.write_all(update.as_bytes()).await {
                    warn!("Failed to send update to player {}: {:?}", player_id, e);
                    break;
                }
            }
        }
    }
    {
        // Create RemovePlayer message and add it to msg_q
        let mut msg_q = msg_q.lock().await;
        msg_q.push_back(Message::RemovePlayer { id: player_id });
    }
    info!("Player {} removed from game state", player_id);
}
