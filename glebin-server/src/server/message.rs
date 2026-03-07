use glebin_protocol::ServerMessage;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerCommand {
    Connect {
        player_id: Uuid,
        glyph: char,
        ui_color: u8,
        name: String,
    },
    Disconnect {
        player_id: Uuid,
    },
    Move {
        player_id: Uuid,
        dx: i16,
        dy: i16,
    },
    SetName {
        player_id: Uuid,
        name: String,
    },
    SendChat {
        player_id: Uuid,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Broadcast(ServerMessage),
    Direct {
        player_id: Uuid,
        message: ServerMessage,
    },
    Audit(String),
}
