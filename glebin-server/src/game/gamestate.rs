use std::collections::{HashMap, HashSet};

use glebin_protocol::{
    ChatKind, ChatMessage, CollectibleState, PlayerState, Point, ServerMessage, Snapshot,
    WorldConfig,
};
use uuid::Uuid;

use crate::server::ServerCommand;

#[derive(Debug)]
pub struct GameState {
    world: WorldConfig,
    players: HashMap<Uuid, PlayerState>,
    collectibles: HashMap<u16, CollectibleState>,
    collectible_spawns: Vec<Point>,
    solid_tiles: HashSet<Point>,
    tick: u64,
    next_collectible_spawn: usize,
}

impl GameState {
    pub fn new(world: WorldConfig) -> Self {
        Self::with_collectible_spawns(world, default_collectible_spawns())
    }

    fn with_collectible_spawns(world: WorldConfig, collectible_spawns: Vec<Point>) -> Self {
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
            collectible_spawns,
            solid_tiles,
            tick: 0,
            next_collectible_spawn: 0,
        };

        state.seed_collectibles(3);
        state
    }

    pub fn apply(&mut self, command: ServerCommand) -> Vec<ServerMessage> {
        match command {
            ServerCommand::Connect {
                player_id,
                glyph,
                name,
            } => self.connect_player(player_id, glyph, name),
            ServerCommand::Disconnect { player_id } => self.disconnect_player(player_id),
            ServerCommand::Move { player_id, dx, dy } => self.move_player(player_id, dx, dy),
            ServerCommand::SetName { player_id, name } => self.set_player_name(player_id, name),
            ServerCommand::SendChat { player_id, text } => self.send_chat(player_id, text),
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.tick = self.tick.wrapping_add(1);
        let mut collectibles = self.collectibles.values().cloned().collect::<Vec<_>>();
        collectibles.sort_by_key(|collectible| collectible.id);
        Snapshot {
            tick: self.tick,
            players: self.players.clone(),
            collectibles,
        }
    }

    fn connect_player(&mut self, player_id: Uuid, glyph: char, name: String) -> Vec<ServerMessage> {
        if self.players.contains_key(&player_id) {
            return Vec::new();
        }

        let sanitized_name = sanitize_name(&name, &format!("Pilot-{glyph}"));
        let player = self.spawn_player(glyph, sanitized_name.clone());
        self.players.insert(player_id, player);

        vec![system_chat(format!(
            "{sanitized_name} entered the shard field"
        ))]
    }

    fn disconnect_player(&mut self, player_id: Uuid) -> Vec<ServerMessage> {
        match self.players.remove(&player_id) {
            Some(player) => vec![system_chat(format!("{} disconnected", player.name))],
            None => Vec::new(),
        }
    }

    fn move_player(&mut self, player_id: Uuid, dx: i16, dy: i16) -> Vec<ServerMessage> {
        let Some(current_position) = self.players.get(&player_id).map(|player| player.position)
        else {
            return Vec::new();
        };

        let next_position = Point::new(
            clamp_axis(current_position.x, dx, self.world.width),
            clamp_axis(current_position.y, dy, self.world.height),
        );

        if self.solid_tiles.contains(&next_position) {
            return Vec::new();
        }

        if let Some(player) = self.players.get_mut(&player_id) {
            player.position = next_position;
        }

        self.collect_if_present(player_id, next_position)
    }

    fn set_player_name(&mut self, player_id: Uuid, name: String) -> Vec<ServerMessage> {
        let Some(player) = self.players.get_mut(&player_id) else {
            return Vec::new();
        };

        let previous_name = player.name.clone();
        let sanitized_name = sanitize_name(&name, &previous_name);
        if sanitized_name == previous_name {
            return Vec::new();
        }

        player.name = sanitized_name.clone();
        vec![system_chat(format!(
            "{previous_name} now goes by {sanitized_name}"
        ))]
    }

    fn send_chat(&mut self, player_id: Uuid, text: String) -> Vec<ServerMessage> {
        let Some(player) = self.players.get(&player_id) else {
            return Vec::new();
        };

        let Some(text) = sanitize_chat(&text) else {
            return Vec::new();
        };

        vec![ServerMessage::Chat {
            message: ChatMessage {
                from: player.name.clone(),
                text,
                kind: ChatKind::Player,
            },
        }]
    }

    fn collect_if_present(&mut self, player_id: Uuid, position: Point) -> Vec<ServerMessage> {
        let Some(collectible_id) = self
            .collectibles
            .iter()
            .find_map(|(id, collectible)| (collectible.position == position).then_some(*id))
        else {
            return Vec::new();
        };

        let Some(collectible) = self.collectibles.get(&collectible_id).cloned() else {
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
            if let Some(entry) = self.collectibles.get_mut(&collectible_id) {
                entry.position = next_position;
            }
        }

        vec![system_chat(format!(
            "{player_name} picked up a {} (+{})",
            collectible.label, collectible.points
        ))]
    }

    fn spawn_player(&self, glyph: char, name: String) -> PlayerState {
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
            };
        }

        PlayerState {
            position: Point::new(0, 0),
            glyph,
            name,
            score: 0,
        }
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
}

fn clamp_axis(current: u16, delta: i16, max: u16) -> u16 {
    let upper = i32::from(max.saturating_sub(1));
    (i32::from(current) + i32::from(delta)).clamp(0, upper) as u16
}

fn sanitize_name(name: &str, fallback: &str) -> String {
    let sanitized = name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(16)
        .collect::<String>();

    if sanitized.is_empty() {
        fallback.to_string()
    } else {
        sanitized
    }
}

fn sanitize_chat(text: &str) -> Option<String> {
    let sanitized = text
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() || *ch == ' ')
        .take(160)
        .collect::<String>();

    (!sanitized.is_empty()).then_some(sanitized)
}

fn system_chat(text: impl Into<String>) -> ServerMessage {
    ServerMessage::Chat {
        message: ChatMessage {
            from: "system".to_string(),
            text: text.into(),
            kind: ChatKind::System,
        },
    }
}

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
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new());
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
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new());
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'A',
            name: "Pilot-A".to_string(),
        });
        state.apply(ServerCommand::Move {
            player_id,
            dx: 2,
            dy: 1,
        });

        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.players.get(&player_id),
            Some(&PlayerState {
                position: Point::new(2, 1),
                glyph: 'A',
                name: "Pilot-A".to_string(),
                score: 0,
            })
        );

        state.apply(ServerCommand::Disconnect { player_id });
        let snapshot = state.snapshot();
        assert!(!snapshot.players.contains_key(&player_id));
    }

    #[test]
    fn blocks_solid_tiles() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new());
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'B',
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
        );
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'C',
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
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ServerMessage::Chat { .. }));
    }

    #[test]
    fn renames_players_and_emits_system_chat() {
        let mut state = GameState::with_collectible_spawns(test_world(), Vec::new());
        let player_id = Uuid::new_v4();

        state.apply(ServerCommand::Connect {
            player_id,
            glyph: 'D',
            name: "Pilot-D".to_string(),
        });
        let events = state.apply(ServerCommand::SetName {
            player_id,
            name: "Nova".to_string(),
        });

        assert_eq!(state.players.get(&player_id).unwrap().name, "Nova");
        assert_eq!(events.len(), 1);
    }
}
