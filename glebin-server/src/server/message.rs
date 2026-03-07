use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerCommand {
    Connect {
        player_id: Uuid,
        glyph: char,
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
