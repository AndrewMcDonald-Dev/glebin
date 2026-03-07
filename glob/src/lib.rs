use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

impl Point {
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldFeature {
    pub position: Point,
    pub glyph: char,
    pub solid: bool,
    pub label: String,
}

impl WorldFeature {
    pub fn new(position: Point, glyph: char, solid: bool, label: impl Into<String>) -> Self {
        Self {
            position,
            glyph,
            solid,
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldConfig {
    pub width: u16,
    pub height: u16,
    pub features: Vec<WorldFeature>,
}

impl WorldConfig {
    pub fn empty(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            features: Vec::new(),
        }
    }
}

impl Default for WorldConfig {
    fn default() -> Self {
        let features = vec![
            WorldFeature::new(Point::new(11, 4), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(12, 4), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(13, 4), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(11, 5), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(13, 5), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(11, 6), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(12, 6), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(13, 6), '#', true, "ruin wall"),
            WorldFeature::new(Point::new(32, 10), '^', true, "tree"),
            WorldFeature::new(Point::new(33, 10), '^', true, "tree"),
            WorldFeature::new(Point::new(31, 11), '^', true, "tree"),
            WorldFeature::new(Point::new(34, 11), '^', true, "tree"),
            WorldFeature::new(Point::new(32, 12), '^', true, "tree"),
            WorldFeature::new(Point::new(24, 8), '~', false, "pool"),
            WorldFeature::new(Point::new(25, 8), '~', false, "pool"),
            WorldFeature::new(Point::new(24, 9), '~', false, "pool"),
            WorldFeature::new(Point::new(25, 9), '~', false, "pool"),
            WorldFeature::new(Point::new(6, 13), 'L', false, "lantern"),
            WorldFeature::new(Point::new(40, 3), 'L', false, "lantern"),
        ];

        Self {
            width: 48,
            height: 18,
            features,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectibleState {
    pub id: u16,
    pub position: Point,
    pub glyph: char,
    pub label: String,
    pub points: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerState {
    pub position: Point,
    pub glyph: char,
    pub name: String,
    pub score: u32,
    pub ui_color: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub tick: u64,
    pub players: HashMap<Uuid, PlayerState>,
    pub collectibles: Vec<CollectibleState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatKind {
    Player,
    System,
    Whisper,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub from: String,
    pub text: String,
    pub kind: ChatKind,
    pub to: Option<String>,
    pub glyph: Option<char>,
    pub ui_color: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Move { dx: i16, dy: i16 },
    SetName { name: String },
    SendChat { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        player_id: Uuid,
        player_glyph: char,
        player_name: String,
        player_color: u8,
        tick_rate_hz: u16,
        world: WorldConfig,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Chat {
        message: ChatMessage,
    },
    Error {
        message: String,
    },
}

pub fn to_line<T: Serialize>(message: &T) -> serde_json::Result<String> {
    let mut encoded = serde_json::to_string(message)?;
    encoded.push('\n');
    Ok(encoded)
}
