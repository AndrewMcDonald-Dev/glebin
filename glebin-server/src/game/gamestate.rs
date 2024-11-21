use std::collections::{HashMap, VecDeque};

use serde::Serialize;
use uuid::Uuid;

use crate::server::Message;

#[derive(Serialize)]
pub enum Player {
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
pub struct GameState {
    players: HashMap<Uuid, Player>,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            players: HashMap::new(),
        }
    }

    pub fn add_player(&mut self, id: Uuid) {
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

    pub fn get_state(&self) -> String {
        serde_json::to_string(&self.players).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn process_messages(&mut self, messages: VecDeque<Message>) -> Result<(), String> {
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

impl Default for GameState {
    fn default() -> Self {
        Self::new()
    }
}
