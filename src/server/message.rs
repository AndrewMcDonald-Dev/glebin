use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone, Deserialize)]
pub enum Message {
    AddPlayer { id: Uuid },
    RemovePlayer { id: Uuid },
    UpdatePlayerPosition { id: Uuid, x: f32, y: f32 },
}
