use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, Mutex},
};
use uuid::Uuid;

#[derive(Serialize)]
enum Player {
    Active(f32, f32),
}

impl Player {
    fn move_player(player: &mut Player, x: f32, y: f32) -> Result<(), String> {
        match player {
            Player::Active(_, _) => {
                *player = Player::Active(x, y);
                Ok(())
            }
        }
    }
}

#[derive(Serialize)]
struct GameState {
    players: HashMap<Uuid, Player>,
}

impl GameState {
    fn new() -> Self {
        Self {
            players: HashMap::new(),
        }
    }

    fn add_player(&mut self, id: Uuid) {
        self.players.insert(id, Player::Active(0.0, 0.0));
    }

    fn remove_players(&mut self, id: Uuid) {
        self.players.remove(&id);
    }

    fn update_player_position(&mut self, id: Uuid, x: f32, y: f32) -> Result<(), String> {
        let player = self.players.get_mut(&id);

        match player {
            Some(player) => Player::move_player(player, x, y),
            None => Err("Could not move player as they do not exist.".to_string()),
        }
    }

    fn get_state(&self) -> String {
        serde_json::to_string(&self.players).unwrap_or_else(|_| "{}".to_string())
    }

    fn process_messages(&mut self, messages: VecDeque<Message>) -> Result<(), String> {
        for message in messages {
            match message {
                Message::AddPlayer { id } => self.add_player(id),
                Message::RemovePlayer { id } => self.remove_players(id),
                Message::UpdatePlayerPosition { id, x, y } => {
                    self.update_player_position(id, x, y)?
                }
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let listener = TcpListener::bind("127.0.0.1:9132").await.unwrap();
    let mut state = GameState::new();
    let msg_q = Arc::new(Mutex::new(VecDeque::<Message>::new()));
    let (tx, _rx) = broadcast::channel(10);
    info!("Server started on 127.0.0.1:8080");

    {
        let msg_q = msg_q.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let msg_q = msg_q.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_client(socket, msg_q, tx).await;
                });
            }
        });
    }

    let mut time = Instant::now();
    loop {
        // wait until x time passes
        if time.elapsed() > Duration::from_millis(3000) {
            let msg_q_copy;
            {
                // grab control over message queue
                let mut msg_q = msg_q.lock().await;

                // copy message queue
                msg_q_copy = msg_q.clone();

                // clear original queue
                msg_q.clear();

                // let go of control over message queue
            }

            // process state with message queue
            state.process_messages(msg_q_copy).unwrap();

            // Send new state to all subscribers.
            let state_string = state.get_state();
            let num_received = tx
                .send(state_string.clone())
                .expect("Could not send message.");
            debug!(
                "{} clients received message: {}",
                num_received, state_string
            );

            // reset timer
            time = Instant::now();
        }
    }
}

#[derive(Clone, Deserialize)]
enum Message {
    AddPlayer { id: Uuid },
    RemovePlayer { id: Uuid },
    UpdatePlayerPosition { id: Uuid, x: f32, y: f32 },
}

async fn handle_client(
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
            // TODO: IDK what this does exactly
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
