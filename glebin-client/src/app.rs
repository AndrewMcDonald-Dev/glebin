use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use glebin_protocol::{
    ChatKind, ChatMessage, CollectibleState, PlayerState, ServerMessage, Snapshot, WorldConfig,
    PROTOCOL_VERSION,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Navigate,
    Chat,
}

#[derive(Debug, Clone)]
pub struct VisualPlayer {
    pub state: PlayerState,
    from: (f32, f32),
    to: (f32, f32),
    started_at: Instant,
    duration: Duration,
}

impl VisualPlayer {
    fn new(state: PlayerState, now: Instant) -> Self {
        let x = f32::from(state.position.x);
        let y = f32::from(state.position.y);
        Self {
            state,
            from: (x, y),
            to: (x, y),
            started_at: now,
            duration: Duration::from_millis(1),
        }
    }

    fn update(&mut self, state: PlayerState, now: Instant, duration: Duration) {
        self.from = self.current_position(now);
        self.to = (f32::from(state.position.x), f32::from(state.position.y));
        self.started_at = now;
        self.duration = duration.max(Duration::from_millis(1));
        self.state = state;
    }

    pub fn current_position(&self, now: Instant) -> (f32, f32) {
        let elapsed = now.saturating_duration_since(self.started_at);
        let duration = self.duration.max(Duration::from_millis(1));
        let progress = (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
        (
            self.from.0 + (self.to.0 - self.from.0) * progress,
            self.from.1 + (self.to.1 - self.from.1) * progress,
        )
    }
}

#[derive(Debug)]
pub struct App {
    pub server_addr: String,
    pub requested_name: String,
    pub player_id: Option<Uuid>,
    pub welcome_color: Option<u8>,
    pub tick_rate_hz: u16,
    pub world: WorldConfig,
    pub tick: u64,
    pub visuals: HashMap<Uuid, VisualPlayer>,
    pub collectibles: Vec<CollectibleState>,
    pub chat_log: Vec<ChatMessage>,
    pub input_mode: InputMode,
    pub chat_input: String,
    pub status: String,
    last_snapshot_at: Option<Instant>,
}

impl App {
    pub fn new(server_addr: String, requested_name: String) -> Self {
        Self {
            server_addr,
            requested_name,
            player_id: None,
            welcome_color: None,
            tick_rate_hz: 0,
            world: WorldConfig::default(),
            tick: 0,
            visuals: HashMap::new(),
            collectibles: Vec::new(),
            chat_log: Vec::new(),
            input_mode: InputMode::Navigate,
            chat_input: String::new(),
            status: "Connecting...".to_string(),
            last_snapshot_at: None,
        }
    }

    pub fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome {
                protocol_version,
                player_id,
                player_glyph,
                player_name,
                player_color,
                tick_rate_hz,
                world,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    self.status = format!(
                        "Protocol mismatch: server {protocol_version}, client {PROTOCOL_VERSION}"
                    );
                    return;
                }
                self.player_id = Some(player_id);
                self.welcome_color = Some(player_color);
                self.tick_rate_hz = tick_rate_hz;
                self.world = world;
                self.status = format!(
                    "Connected as {player_name} ({player_glyph}) to {}",
                    self.server_addr
                );
            }
            ServerMessage::Snapshot { snapshot } => self.apply_snapshot(snapshot),
            ServerMessage::Chat { message } => self.push_chat(message),
            ServerMessage::Error { message } => {
                self.status = format!("Server error: {message}");
            }
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        if snapshot.tick < self.tick {
            return;
        }
        let now = Instant::now();
        let duration = self
            .last_snapshot_at
            .map(|previous| clamp_duration(now.saturating_duration_since(previous)))
            .unwrap_or_else(|| Duration::from_millis(45));
        self.last_snapshot_at = Some(now);
        self.tick = snapshot.tick;
        self.collectibles = snapshot.collectibles;

        let mut next_visuals = HashMap::new();
        for (player_id, player) in snapshot.players {
            if let Some(mut visual) = self.visuals.remove(&player_id) {
                visual.update(player, now, duration);
                next_visuals.insert(player_id, visual);
            } else {
                next_visuals.insert(player_id, VisualPlayer::new(player, now));
            }
        }
        self.visuals = next_visuals;
    }

    pub fn push_chat(&mut self, message: ChatMessage) {
        self.status = match message.kind {
            ChatKind::Player => format!("{}: {}", message.from, message.text),
            ChatKind::System => message.text.clone(),
            ChatKind::Whisper => format!(
                "{} -> {}: {}",
                message.from,
                message.to.as_deref().unwrap_or("?"),
                message.text
            ),
        };
        self.chat_log.push(message);
        if self.chat_log.len() > 120 {
            self.chat_log.drain(0..self.chat_log.len() - 120);
        }
    }

    pub fn mark_disconnected(&mut self, reason: String) {
        self.status = reason;
    }

    pub fn local_player(&self) -> Option<&PlayerState> {
        self.player_id
            .and_then(|player_id| self.visuals.get(&player_id))
            .map(|visual| &visual.state)
    }

    pub fn focus_position(&self, now: Instant) -> (f32, f32) {
        self.player_id
            .and_then(|player_id| self.visuals.get(&player_id))
            .map(|visual| visual.current_position(now))
            .unwrap_or_else(|| {
                (
                    f32::from(self.world.width.saturating_sub(1)) / 2.0,
                    f32::from(self.world.height.saturating_sub(1)) / 2.0,
                )
            })
    }
}

fn clamp_duration(duration: Duration) -> Duration {
    duration.clamp(Duration::from_millis(25), Duration::from_millis(90))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_older_snapshots() {
        let mut app = App::new("test".to_string(), "test".to_string());
        app.apply_snapshot(Snapshot {
            tick: 10,
            players: HashMap::new(),
            collectibles: Vec::new(),
        });
        app.apply_snapshot(Snapshot {
            tick: 9,
            players: HashMap::new(),
            collectibles: Vec::new(),
        });
        assert_eq!(app.tick, 10);
    }
}
