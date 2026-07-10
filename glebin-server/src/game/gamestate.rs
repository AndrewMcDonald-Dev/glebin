use std::collections::{HashMap, HashSet, VecDeque};

use glebin_protocol::{
    ChatKind, ChatMessage, CollectibleState, PlayerState, Point, ServerMessage, Snapshot,
    WorldConfig, MAX_CHAT_LEN, MAX_NAME_LEN,
};
use uuid::Uuid;

use crate::server::{ServerCommand, ServerEvent};

#[derive(Debug)]
pub struct GameState {
    world: WorldConfig,
    players: HashMap<Uuid, PlayerState>,
    collectibles: HashMap<u16, CollectibleState>,
    pending_collectibles: Vec<CollectibleState>,
    collectible_spawns: Vec<Point>,
    solid_tiles: HashSet<Point>,
    tick: u64,
    next_collectible_spawn: usize,
    chat_history: VecDeque<ChatMessage>,
    last_whisper_from: HashMap<Uuid, Uuid>,
}

impl GameState {
    pub fn new(world: WorldConfig) -> Result<Self, String> {
        let collectible_spawns = default_collectible_spawns()
            .into_iter()
            .filter(|position| position.x < world.width && position.y < world.height)
            .collect();
        Self::with_collectible_spawns(world, collectible_spawns)
    }

    fn with_collectible_spawns(
        world: WorldConfig,
        collectible_spawns: Vec<Point>,
    ) -> Result<Self, String> {
        world.validate()?;
        if let Some(position) = collectible_spawns
            .iter()
            .find(|position| position.x >= world.width || position.y >= world.height)
        {
            return Err(format!(
                "collectible spawn ({}, {}) is outside the {}x{} world",
                position.x, position.y, world.width, world.height
            ));
        }

        let solid_tiles = world
            .features
            .iter()
            .filter(|feature| feature.solid)
            .map(|feature| feature.position)
            .collect::<HashSet<_>>();

        let mut state = Self {
            world,
            players: HashMap::new(),
            collectibles: HashMap::new(),
            pending_collectibles: Vec::new(),
            collectible_spawns,
            solid_tiles,
            tick: 0,
            next_collectible_spawn: 0,
            chat_history: VecDeque::with_capacity(CHAT_HISTORY_LIMIT),
            last_whisper_from: HashMap::new(),
        };

        state.seed_collectibles(3);
        Ok(state)
    }

    pub fn apply(&mut self, command: ServerCommand) -> Vec<ServerEvent> {
        match command {
            ServerCommand::Connect {
                player_id,
                glyph,
                ui_color,
                name,
            } => self.connect_player(player_id, glyph, ui_color, name),
            ServerCommand::Disconnect { player_id } => self.disconnect_player(player_id),
            ServerCommand::Move { player_id, dx, dy } => self.move_player(player_id, dx, dy),
            ServerCommand::SetName { player_id, name } => self.set_player_name(player_id, name),
            ServerCommand::SendChat { player_id, text } => self.send_chat(player_id, text),
        }
    }

    pub fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.respawn_pending_collectibles();
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut collectibles = self.collectibles.values().cloned().collect::<Vec<_>>();
        collectibles.sort_by_key(|collectible| collectible.id);
        Snapshot {
            tick: self.tick,
            players: self.players.clone(),
            collectibles,
        }
    }

    pub fn chat_history(&self) -> Vec<ChatMessage> {
        self.chat_history.iter().cloned().collect()
    }

    fn connect_player(
        &mut self,
        player_id: Uuid,
        glyph: char,
        ui_color: u8,
        name: String,
    ) -> Vec<ServerEvent> {
        if self.players.contains_key(&player_id) {
            return Vec::new();
        }

        let fallback_name = format!("Pilot-{glyph}");
        let sanitized_name = self.unique_name(&name, &fallback_name, None);
        let player = self.spawn_player(glyph, ui_color, sanitized_name.clone());
        self.players.insert(player_id, player);

        self.broadcast_system_chat(format!("{sanitized_name} entered the shard field"))
    }

    fn disconnect_player(&mut self, player_id: Uuid) -> Vec<ServerEvent> {
        self.last_whisper_from.remove(&player_id);
        self.last_whisper_from
            .retain(|_, last_sender_id| *last_sender_id != player_id);

        match self.players.remove(&player_id) {
            Some(player) => self.broadcast_system_chat(format!("{} disconnected", player.name)),
            None => Vec::new(),
        }
    }

    fn move_player(&mut self, player_id: Uuid, dx: i16, dy: i16) -> Vec<ServerEvent> {
        let Some(current_position) = self.players.get(&player_id).map(|player| player.position)
        else {
            return Vec::new();
        };

        if !matches!((dx, dy), (-1, 0) | (1, 0) | (0, -1) | (0, 1)) {
            return self.direct_error(
                player_id,
                "movement must be exactly one cardinal tile".to_string(),
            );
        }

        let next_position = Point::new(
            clamp_axis(current_position.x, dx, self.world.width),
            clamp_axis(current_position.y, dy, self.world.height),
        );

        if self.solid_tiles.contains(&next_position) {
            return Vec::new();
        }

        if self
            .players
            .iter()
            .any(|(other_id, player)| *other_id != player_id && player.position == next_position)
        {
            return Vec::new();
        }

        if let Some(player) = self.players.get_mut(&player_id) {
            player.position = next_position;
        }

        self.collect_if_present(player_id, next_position)
    }

    fn set_player_name(&mut self, player_id: Uuid, name: String) -> Vec<ServerEvent> {
        let Some(previous_name) = self
            .players
            .get(&player_id)
            .map(|player| player.name.clone())
        else {
            return Vec::new();
        };

        let sanitized_name = self.unique_name(&name, &previous_name, Some(player_id));
        if sanitized_name == previous_name {
            return Vec::new();
        }

        if let Some(player) = self.players.get_mut(&player_id) {
            player.name = sanitized_name.clone();
        }
        self.broadcast_system_chat(format!("{previous_name} now goes by {sanitized_name}"))
    }

    fn send_chat(&mut self, player_id: Uuid, text: String) -> Vec<ServerEvent> {
        let Some(player) = self.players.get(&player_id).cloned() else {
            return Vec::new();
        };

        let Some(text) = sanitize_chat(&text) else {
            return Vec::new();
        };

        if let Some(rest) = text.strip_prefix("/w ") {
            return self.send_whisper(player_id, &player, rest);
        }

        if text == "/r" || text.starts_with("/r ") {
            let reply = text.strip_prefix("/r").unwrap_or_default().trim_start();
            return self.reply_whisper(player_id, &player, reply);
        }

        self.broadcast_player_chat(player, text)
    }

    fn collect_if_present(&mut self, player_id: Uuid, position: Point) -> Vec<ServerEvent> {
        let Some(collectible_id) = self
            .collectibles
            .iter()
            .find_map(|(id, collectible)| (collectible.position == position).then_some(*id))
        else {
            return Vec::new();
        };

        let Some(mut collectible) = self.collectibles.remove(&collectible_id) else {
            return Vec::new();
        };

        let player_name;
        if let Some(player) = self.players.get_mut(&player_id) {
            player.score = player.score.saturating_add(collectible.points);
            player_name = player.name.clone();
        } else {
            return Vec::new();
        }

        if let Some(next_position) = self.next_available_collectible_position() {
            collectible.position = next_position;
            self.collectibles
                .insert(collectible_id, collectible.clone());
        } else {
            self.pending_collectibles.push(collectible.clone());
        }

        self.broadcast_system_chat(format!(
            "{player_name} picked up a {} (+{})",
            collectible.label, collectible.points
        ))
    }

    fn spawn_player(&self, glyph: char, ui_color: u8, name: String) -> PlayerState {
        let width = usize::from(self.world.width.max(1));
        let height = usize::from(self.world.height.max(1));

        for index in 0..(width * height).max(1) {
            let candidate = Point::new((index % width) as u16, ((index / width) % height) as u16);
            if self.solid_tiles.contains(&candidate) || self.collectible_at(candidate) {
                continue;
            }
            if self
                .players
                .values()
                .any(|player| player.position == candidate)
            {
                continue;
            }

            return PlayerState {
                position: candidate,
                glyph,
                name,
                score: 0,
                ui_color,
            };
        }

        PlayerState {
            position: Point::new(0, 0),
            glyph,
            name,
            score: 0,
            ui_color,
        }
    }

    fn unique_name(
        &self,
        requested: &str,
        fallback: &str,
        exclude_player_id: Option<Uuid>,
    ) -> String {
        let sanitized = sanitize_name(requested, fallback);
        if !self.name_in_use(&sanitized, exclude_player_id) {
            return sanitized;
        }

        for suffix in 2u32.. {
            let candidate = suffix_name(&sanitized, suffix);
            if !self.name_in_use(&candidate, exclude_player_id) {
                return candidate;
            }
        }

        sanitized
    }

    fn name_in_use(&self, candidate: &str, exclude_player_id: Option<Uuid>) -> bool {
        let candidate = name_key(candidate);
        self.players.iter().any(|(player_id, player)| {
            Some(*player_id) != exclude_player_id && name_key(&player.name) == candidate
        })
    }

    fn broadcast_player_chat(&mut self, player: PlayerState, text: String) -> Vec<ServerEvent> {
        let message = ChatMessage {
            from: player.name,
            text,
            kind: ChatKind::Player,
            to: None,
            glyph: Some(player.glyph),
            ui_color: Some(player.ui_color),
        };
        self.push_history(message.clone());
        vec![
            ServerEvent::Audit(audit_line(&message)),
            ServerEvent::Broadcast(ServerMessage::Chat { message }),
        ]
    }

    fn broadcast_system_chat(&mut self, text: String) -> Vec<ServerEvent> {
        let message = ChatMessage {
            from: "system".to_string(),
            text,
            kind: ChatKind::System,
            to: None,
            glyph: None,
            ui_color: None,
        };
        self.push_history(message.clone());
        vec![
            ServerEvent::Audit(audit_line(&message)),
            ServerEvent::Broadcast(ServerMessage::Chat { message }),
        ]
    }

    fn send_whisper(
        &mut self,
        sender_id: Uuid,
        sender: &PlayerState,
        whisper_payload: &str,
    ) -> Vec<ServerEvent> {
        let Some((target_name, whisper_text)) = parse_whisper_command(whisper_payload) else {
            return self.direct_system_chat(sender_id, "Usage: /w <name> <message>".to_string());
        };

        let Some((target_id, target)) = self.find_player_by_name(target_name) else {
            return self.direct_system_chat(
                sender_id,
                format!("No player named {target_name} is connected"),
            );
        };

        self.send_whisper_to_target(
            sender_id,
            sender,
            target_id,
            &target,
            whisper_text.to_string(),
        )
    }

    fn reply_whisper(
        &mut self,
        sender_id: Uuid,
        sender: &PlayerState,
        whisper_text: &str,
    ) -> Vec<ServerEvent> {
        let whisper_text = whisper_text.trim();
        if whisper_text.is_empty() {
            return self.direct_system_chat(sender_id, "Usage: /r <message>".to_string());
        }

        let Some(target_id) = self.last_whisper_from.get(&sender_id).copied() else {
            return self.direct_system_chat(sender_id, "No whisper to reply to yet".to_string());
        };

        let Some(target) = self.players.get(&target_id).cloned() else {
            self.last_whisper_from.remove(&sender_id);
            return self
                .direct_system_chat(sender_id, "That player is no longer connected".to_string());
        };

        self.send_whisper_to_target(
            sender_id,
            sender,
            target_id,
            &target,
            whisper_text.to_string(),
        )
    }

    fn send_whisper_to_target(
        &mut self,
        sender_id: Uuid,
        sender: &PlayerState,
        target_id: Uuid,
        target: &PlayerState,
        whisper_text: String,
    ) -> Vec<ServerEvent> {
        self.last_whisper_from.insert(target_id, sender_id);

        let sender_message = ChatMessage {
            from: sender.name.clone(),
            text: whisper_text.clone(),
            kind: ChatKind::Whisper,
            to: Some(target.name.clone()),
            glyph: Some(sender.glyph),
            ui_color: Some(sender.ui_color),
        };
        let recipient_message = sender_message.clone();

        vec![
            ServerEvent::Audit(audit_line(&sender_message)),
            ServerEvent::Direct {
                player_id: sender_id,
                message: ServerMessage::Chat {
                    message: sender_message,
                },
            },
            ServerEvent::Direct {
                player_id: target_id,
                message: ServerMessage::Chat {
                    message: recipient_message,
                },
            },
        ]
    }

    fn direct_system_chat(&self, player_id: Uuid, text: String) -> Vec<ServerEvent> {
        vec![ServerEvent::Direct {
            player_id,
            message: ServerMessage::Chat {
                message: ChatMessage {
                    from: "system".to_string(),
                    text,
                    kind: ChatKind::System,
                    to: None,
                    glyph: None,
                    ui_color: None,
                },
            },
        }]
    }

    fn direct_error(&self, player_id: Uuid, message: String) -> Vec<ServerEvent> {
        vec![ServerEvent::Direct {
            player_id,
            message: ServerMessage::Error { message },
        }]
    }

    fn find_player_by_name(&self, name: &str) -> Option<(Uuid, PlayerState)> {
        let name = name_key(name);
        self.players.iter().find_map(|(player_id, player)| {
            (name_key(&player.name) == name).then_some((*player_id, player.clone()))
        })
    }

    fn push_history(&mut self, message: ChatMessage) {
        if self.chat_history.len() == CHAT_HISTORY_LIMIT {
            self.chat_history.pop_front();
        }
        self.chat_history.push_back(message);
    }

    fn collectible_at(&self, position: Point) -> bool {
        self.collectibles
            .values()
            .any(|collectible| collectible.position == position)
    }

    fn next_available_collectible_position(&mut self) -> Option<Point> {
        if self.collectible_spawns.is_empty() {
            return None;
        }

        for _ in 0..self.collectible_spawns.len() {
            let position = self.collectible_spawns[self.next_collectible_spawn];
            self.next_collectible_spawn =
                (self.next_collectible_spawn + 1) % self.collectible_spawns.len();

            if self.solid_tiles.contains(&position) || self.collectible_at(position) {
                continue;
            }
            if self
                .players
                .values()
                .any(|player| player.position == position)
            {
                continue;
            }

            return Some(position);
        }

        None
    }

    fn seed_collectibles(&mut self, desired: usize) {
        for id in 0..desired.min(u16::MAX as usize) {
            let Some(position) = self.next_available_collectible_position() else {
                break;
            };
            self.collectibles.insert(
                id as u16,
                CollectibleState {
                    id: id as u16,
                    position,
                    glyph: '*',
                    label: "star shard".to_string(),
                    points: 1,
                },
            );
        }
    }

    fn respawn_pending_collectibles(&mut self) {
        let pending = std::mem::take(&mut self.pending_collectibles);
        for mut collectible in pending {
            if let Some(position) = self.next_available_collectible_position() {
                collectible.position = position;
                self.collectibles.insert(collectible.id, collectible);
            } else {
                self.pending_collectibles.push(collectible);
            }
        }
    }
}

fn clamp_axis(current: u16, delta: i16, max: u16) -> u16 {
    let upper = i32::from(max.saturating_sub(1));
    (i32::from(current) + i32::from(delta)).clamp(0, upper) as u16
}

fn sanitize_name(name: &str, fallback: &str) -> String {
    let sanitized = name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() && !ch.is_whitespace())
        .take(MAX_NAME_LEN)
        .collect::<String>();

    if sanitized.is_empty() {
        fallback.to_string()
    } else {
        sanitized
    }
}

fn name_key(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

fn suffix_name(base: &str, suffix: u32) -> String {
    let suffix = suffix.to_string();
    if suffix.len() >= MAX_NAME_LEN {
        return suffix
            .chars()
            .skip(suffix.len() - MAX_NAME_LEN)
            .collect::<String>();
    }

    let available = MAX_NAME_LEN - suffix.len();
    let prefix = base.chars().take(available).collect::<String>();
    format!("{prefix}{suffix}")
}

fn sanitize_chat(text: &str) -> Option<String> {
    let sanitized = text
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() || *ch == ' ')
        .take(MAX_CHAT_LEN)
        .collect::<String>();

    (!sanitized.is_empty()).then_some(sanitized)
}

fn parse_whisper_command(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    let (target_name, whisper_text) = trimmed.split_once(' ')?;
    let whisper_text = whisper_text.trim();
    (!target_name.is_empty() && !whisper_text.is_empty()).then_some((target_name, whisper_text))
}

fn audit_line(message: &ChatMessage) -> String {
    match message.kind {
        ChatKind::Player => format!("[chat] {}: {}", message.from, message.text),
        ChatKind::System => format!("[system] {}", message.text),
        ChatKind::Whisper => format!(
            "[whisper] {} -> {}: {}",
            message.from,
            message.to.as_deref().unwrap_or("?"),
            message.text
        ),
    }
}

const CHAT_HISTORY_LIMIT: usize = 20;

fn default_collectible_spawns() -> Vec<Point> {
    vec![
        Point::new(4, 3),
        Point::new(20, 4),
        Point::new(38, 5),
        Point::new(7, 10),
        Point::new(20, 13),
        Point::new(42, 14),
        Point::new(27, 2),
        Point::new(15, 15),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glebin_protocol::WorldFeature;

    fn test_world() -> WorldConfig {
        WorldConfig {
            width: 6,
            height: 4,
            features: vec![WorldFeature::new(Point::new(1, 0), '#', true, "wall")],
        }
    }

    #[test]
    fn ignores_moves_for_unknown_players() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let player_id = Uuid::new_v4();

        let events = state.apply(ServerCommand::Move {
            player_id,
            dx: 1,
            dy: 1,
        });

        let snapshot = state.snapshot();
        assert!(events.is_empty());
        assert!(snapshot.players.is_empty());
    }

    #[test]
    fn tracks_connect_move_and_disconnect() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'A',
            ui_color: 33,
            name: "Pilot-A".to_string(),
        });
        state.apply(ServerCommand::Move {
            player_id,
            dx: 0,
            dy: 1,
        });
        state.apply(ServerCommand::Move {
            player_id,
            dx: 1,
            dy: 0,
        });
        state.apply(ServerCommand::Move {
            player_id,
            dx: 1,
            dy: 0,
        });

        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.players.get(&player_id),
            Some(&PlayerState {
                position: Point::new(2, 1),
                glyph: 'A',
                name: "Pilot-A".to_string(),
                score: 0,
                ui_color: 33,
            })
        );

        state.apply(ServerCommand::Disconnect { player_id });
        let snapshot = state.snapshot();
        assert!(!snapshot.players.contains_key(&player_id));
    }

    #[test]
    fn blocks_solid_tiles() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'B',
            ui_color: 44,
            name: "Pilot-B".to_string(),
        });
        state.apply(ServerCommand::Move {
            player_id,
            dx: 1,
            dy: 0,
        });

        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.players.get(&player_id).unwrap().position,
            Point::new(0, 0)
        );
    }

    #[test]
    fn collects_pickups_and_awards_score() {
        let mut state = GameState::with_collectible_spawns(
            test_world(),
            vec![
                Point::new(0, 1),
                Point::new(5, 3),
                Point::new(4, 0),
                Point::new(5, 2),
            ],
        )
        .unwrap();
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'C',
            ui_color: 55,
            name: "Collector".to_string(),
        });
        let events = state.apply(ServerCommand::Move {
            player_id,
            dx: 0,
            dy: 1,
        });

        let snapshot = state.snapshot();
        assert_eq!(snapshot.players.get(&player_id).unwrap().score, 1);
        assert_eq!(snapshot.collectibles[0].position, Point::new(5, 2));
        assert!(events
            .iter()
            .any(|event| matches!(event, ServerEvent::Broadcast(ServerMessage::Chat { .. }))));
    }

    #[test]
    fn renames_players_and_emits_system_chat() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'D',
            ui_color: 66,
            name: "Pilot-D".to_string(),
        });
        let events = state.apply(ServerCommand::SetName {
            player_id,
            name: "Nova".to_string(),
        });

        assert_eq!(state.players.get(&player_id).unwrap().name, "Nova");
        assert!(events
            .iter()
            .any(|event| matches!(event, ServerEvent::Broadcast(ServerMessage::Chat { .. }))));
    }

    #[test]
    fn rejects_teleports_and_diagonal_movement() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let player_id = Uuid::new_v4();
        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'E',
            ui_color: 77,
            name: "Mover".to_string(),
        });

        for (dx, dy) in [(2, 0), (1, 1), (0, 0)] {
            let events = state.apply(ServerCommand::Move { player_id, dx, dy });
            assert!(events.iter().any(|event| matches!(
                event,
                ServerEvent::Direct {
                    message: ServerMessage::Error { .. },
                    ..
                }
            )));
        }
        assert_eq!(state.players[&player_id].position, Point::new(0, 0));
    }

    #[test]
    fn blocks_movement_into_other_players() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        for (player_id, glyph) in [(first, 'F'), (second, 'G')] {
            state.apply(ServerCommand::Connect {
                player_id,
                glyph,
                ui_color: 88,
                name: glyph.to_string(),
            });
        }
        assert_eq!(state.players[&first].position, Point::new(0, 0));
        assert_eq!(state.players[&second].position, Point::new(2, 0));

        state.apply(ServerCommand::Move {
            player_id: first,
            dx: 0,
            dy: 1,
        });
        state.apply(ServerCommand::Move {
            player_id: first,
            dx: 1,
            dy: 0,
        });
        state.apply(ServerCommand::Move {
            player_id: second,
            dx: 0,
            dy: 1,
        });
        state.apply(ServerCommand::Move {
            player_id: second,
            dx: -1,
            dy: 0,
        });
        assert_eq!(state.players[&first].position, Point::new(1, 1));
        assert_eq!(state.players[&second].position, Point::new(2, 1));
    }

    #[test]
    fn names_are_unique_case_insensitively_and_have_no_whitespace() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new()).unwrap();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        state.apply(ServerCommand::Connect {
            player_id: first,
            glyph: 'H',
            ui_color: 99,
            name: "Alice Smith".to_string(),
        });
        state.apply(ServerCommand::Connect {
            player_id: second,
            glyph: 'I',
            ui_color: 100,
            name: "alicesmith".to_string(),
        });

        assert_eq!(state.players[&first].name, "AliceSmith");
        assert_eq!(state.players[&second].name, "alicesmith2");
    }

    #[test]
    fn collected_items_wait_for_a_free_respawn_without_scoring_twice() {
        let mut state =
            GameState::with_collectible_spawns(test_world(), vec![Point::new(0, 1)]).unwrap();
        let player_id = Uuid::new_v4();
        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'J',
            ui_color: 101,
            name: "Collector".to_string(),
        });
        state.apply(ServerCommand::Move {
            player_id,
            dx: 0,
            dy: 1,
        });
        state.advance_tick();
        state.apply(ServerCommand::Move {
            player_id,
            dx: 0,
            dy: 0,
        });

        assert_eq!(state.players[&player_id].score, 1);
        assert!(state.collectibles.is_empty());
        assert_eq!(state.pending_collectibles.len(), 1);
    }

    #[test]
    fn rejects_out_of_bounds_world_content() {
        let world = WorldConfig {
            width: 1,
            height: 1,
            features: vec![WorldFeature::new(Point::new(1, 0), '#', true, "bad wall")],
        };
        assert!(GameState::with_collectible_spawns(world, Vec::new()).is_err());
    }

    #[test]
    fn supports_valid_worlds_smaller_than_default_collectible_map() {
        let state = GameState::new(WorldConfig::empty(2, 2)).unwrap();
        assert!(state.collectibles.is_empty());
    }
}
