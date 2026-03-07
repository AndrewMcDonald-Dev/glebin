use glebin_protocol::{ChatKind, ClientMessage, ServerMessage};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    time::{timeout, Duration},
};

use crate::helpers::{next_message, TestApp};

#[tokio::test]
async fn test_connection_receives_welcome_with_identity_metadata() {
    let app = TestApp::spawn().await;
    let stream = app.connect().await;
    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let message = next_message(&mut lines).await.unwrap();
    match message {
        ServerMessage::Welcome {
            player_id: _,
            player_glyph,
            player_name,
            tick_rate_hz,
            world,
        } => {
            assert_eq!(tick_rate_hz, 128);
            assert_eq!(player_glyph, 'A');
            assert_eq!(player_name, "Pilot-A");
            assert!(!world.features.is_empty());
        }
        other => panic!("expected welcome message, got {other:?}"),
    }
}

#[tokio::test]
async fn test_renaming_and_movement_are_reflected_in_snapshots() {
    let app = TestApp::spawn().await;
    let stream = app.connect().await;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let player_id = match next_message(&mut lines).await.unwrap() {
        ServerMessage::Welcome {
            player_id,
            player_glyph: _,
            player_name: _,
            tick_rate_hz: _,
            world: _,
        } => player_id,
        other => panic!("expected welcome message, got {other:?}"),
    };

    writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::SetName {
                name: "Nova".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::Move { dx: 3, dy: 2 })
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();
    writer.flush().await.unwrap();

    let snapshot = timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut lines).await.unwrap() {
                ServerMessage::Snapshot { snapshot } => {
                    if let Some(player) = snapshot.players.get(&player_id) {
                        if player.name == "Nova" && player.position.x == 3 && player.position.y == 2
                        {
                            break player.clone();
                        }
                    }
                }
                ServerMessage::Chat { .. } | ServerMessage::Welcome { .. } => {}
                ServerMessage::Error { message } => panic!("unexpected server error: {message}"),
            }
        }
    })
    .await
    .expect("timed out waiting for renamed player snapshot");

    assert_eq!(snapshot.glyph, 'A');
    assert_eq!(snapshot.score, 0);
}

#[tokio::test]
async fn test_chat_broadcasts_to_other_clients() {
    let app = TestApp::spawn().await;
    let first_stream = app.connect().await;
    let second_stream = app.connect().await;

    let (first_reader, mut first_writer) = first_stream.into_split();
    let (second_reader, _second_writer) = second_stream.into_split();
    let mut first_lines = BufReader::new(first_reader).lines();
    let mut second_lines = BufReader::new(second_reader).lines();

    let _ = next_message(&mut first_lines).await.unwrap();
    let _ = next_message(&mut second_lines).await.unwrap();

    first_writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::SetName {
                name: "Alice".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    first_writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::SendChat {
                text: "hello there".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    first_writer.flush().await.unwrap();

    let chat_message = timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut second_lines).await.unwrap() {
                ServerMessage::Chat { message }
                    if message.kind == ChatKind::Player
                        && message.from == "Alice"
                        && message.text == "hello there" =>
                {
                    break message;
                }
                ServerMessage::Chat { .. }
                | ServerMessage::Snapshot { .. }
                | ServerMessage::Welcome { .. } => {}
                ServerMessage::Error { message } => panic!("unexpected server error: {message}"),
            }
        }
    })
    .await
    .expect("timed out waiting for player chat");

    assert_eq!(chat_message.from, "Alice");
}
