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
            player_color,
            tick_rate_hz,
            world,
        } => {
            assert_eq!(tick_rate_hz, 128);
            assert_eq!(player_glyph, 'A');
            assert_eq!(player_name, "Pilot-A");
            assert!(player_color >= 16);
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
            player_color: _,
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
    assert!(snapshot.ui_color >= 16);
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
    assert_eq!(chat_message.glyph, Some('A'));
    assert!(chat_message.ui_color.is_some());
}

#[tokio::test]
async fn test_duplicate_requested_names_get_numbered() {
    let app = TestApp::spawn().await;
    let first_stream = app.connect().await;
    let second_stream = app.connect().await;

    let (first_reader, mut first_writer) = first_stream.into_split();
    let (second_reader, mut second_writer) = second_stream.into_split();
    let mut first_lines = BufReader::new(first_reader).lines();
    let mut second_lines = BufReader::new(second_reader).lines();

    let first_player_id = match next_message(&mut first_lines).await.unwrap() {
        ServerMessage::Welcome {
            player_id,
            player_glyph: _,
            player_name: _,
            player_color: _,
            tick_rate_hz: _,
            world: _,
        } => player_id,
        other => panic!("expected welcome message, got {other:?}"),
    };
    let second_player_id = match next_message(&mut second_lines).await.unwrap() {
        ServerMessage::Welcome {
            player_id,
            player_glyph: _,
            player_name: _,
            player_color: _,
            tick_rate_hz: _,
            world: _,
        } => player_id,
        other => panic!("expected welcome message, got {other:?}"),
    };

    for writer in [&mut first_writer, &mut second_writer] {
        writer
            .write_all(
                glebin_protocol::to_line(&ClientMessage::SetName {
                    name: "andrew".to_string(),
                })
                .unwrap()
                .as_bytes(),
            )
            .await
            .unwrap();
        writer.flush().await.unwrap();
    }

    let names = timeout(Duration::from_secs(1), async {
        loop {
            let mut first_name = None;
            let mut second_name = None;

            if let ServerMessage::Snapshot { snapshot } =
                next_message(&mut first_lines).await.unwrap()
            {
                first_name = snapshot
                    .players
                    .get(&first_player_id)
                    .map(|player| player.name.clone());
                second_name = snapshot
                    .players
                    .get(&second_player_id)
                    .map(|player| player.name.clone());
            }

            if first_name.is_none() || second_name.is_none() {
                if let ServerMessage::Snapshot { snapshot } =
                    next_message(&mut second_lines).await.unwrap()
                {
                    first_name = first_name.or_else(|| {
                        snapshot
                            .players
                            .get(&first_player_id)
                            .map(|player| player.name.clone())
                    });
                    second_name = second_name.or_else(|| {
                        snapshot
                            .players
                            .get(&second_player_id)
                            .map(|player| player.name.clone())
                    });
                }
            }

            if let (Some(first_name), Some(second_name)) = (first_name, second_name) {
                if [first_name.as_str(), second_name.as_str()].contains(&"andrew")
                    && [first_name.as_str(), second_name.as_str()].contains(&"andrew2")
                {
                    break (first_name, second_name);
                }
            }
        }
    })
    .await
    .expect("timed out waiting for unique duplicate names");

    assert_ne!(names.0, names.1);
}

#[tokio::test]
async fn test_whispers_are_private_and_reply_works() {
    let app = TestApp::spawn().await;
    let first_stream = app.connect().await;
    let second_stream = app.connect().await;
    let third_stream = app.connect().await;

    let (first_reader, mut first_writer) = first_stream.into_split();
    let (second_reader, mut second_writer) = second_stream.into_split();
    let (third_reader, _third_writer) = third_stream.into_split();
    let mut first_lines = BufReader::new(first_reader).lines();
    let mut second_lines = BufReader::new(second_reader).lines();
    let mut third_lines = BufReader::new(third_reader).lines();

    let first_player_id = match next_message(&mut first_lines).await.unwrap() {
        ServerMessage::Welcome {
            player_id,
            player_glyph: _,
            player_name: _,
            player_color: _,
            tick_rate_hz: _,
            world: _,
        } => player_id,
        other => panic!("expected welcome message, got {other:?}"),
    };
    let second_player_id = match next_message(&mut second_lines).await.unwrap() {
        ServerMessage::Welcome {
            player_id,
            player_glyph: _,
            player_name: _,
            player_color: _,
            tick_rate_hz: _,
            world: _,
        } => player_id,
        other => panic!("expected welcome message, got {other:?}"),
    };
    let _ = next_message(&mut third_lines).await.unwrap();

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
    second_writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::SetName {
                name: "Bob".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    first_writer.flush().await.unwrap();
    second_writer.flush().await.unwrap();

    timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut first_lines).await.unwrap() {
                ServerMessage::Snapshot { snapshot } => {
                    let first_name = snapshot
                        .players
                        .get(&first_player_id)
                        .map(|player| player.name.as_str());
                    let second_name = snapshot
                        .players
                        .get(&second_player_id)
                        .map(|player| player.name.as_str());
                    if first_name == Some("Alice") && second_name == Some("Bob") {
                        break;
                    }
                }
                ServerMessage::Chat { .. } | ServerMessage::Welcome { .. } => {}
                ServerMessage::Error { message } => panic!("unexpected server error: {message}"),
            }
        }
    })
    .await
    .expect("timed out waiting for renamed players");

    first_writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::SendChat {
                text: "/w Bob hush hush".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    first_writer.flush().await.unwrap();

    let first_whisper = timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut first_lines).await.unwrap() {
                ServerMessage::Chat { message }
                    if message.kind == ChatKind::Whisper
                        && message.from == "Alice"
                        && message.to.as_deref() == Some("Bob")
                        && message.text == "hush hush" =>
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
    .expect("timed out waiting for sender whisper echo");
    assert_eq!(first_whisper.glyph, Some('A'));

    let second_whisper = timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut second_lines).await.unwrap() {
                ServerMessage::Chat { message }
                    if message.kind == ChatKind::Whisper
                        && message.from == "Alice"
                        && message.to.as_deref() == Some("Bob")
                        && message.text == "hush hush" =>
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
    .expect("timed out waiting for recipient whisper");
    assert_eq!(second_whisper.glyph, Some('A'));

    let third_observed_whisper = timeout(Duration::from_millis(300), async {
        loop {
            match next_message(&mut third_lines).await.unwrap() {
                ServerMessage::Chat { message } if message.kind == ChatKind::Whisper => {
                    break Some(message);
                }
                ServerMessage::Chat { .. }
                | ServerMessage::Snapshot { .. }
                | ServerMessage::Welcome { .. } => {}
                ServerMessage::Error { message } => panic!("unexpected server error: {message}"),
            }
        }
    })
    .await
    .ok()
    .flatten();
    assert!(
        third_observed_whisper.is_none(),
        "third client should not receive whispers"
    );

    second_writer
        .write_all(
            glebin_protocol::to_line(&ClientMessage::SendChat {
                text: "/r hi back".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    second_writer.flush().await.unwrap();

    let reply = timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut first_lines).await.unwrap() {
                ServerMessage::Chat { message }
                    if message.kind == ChatKind::Whisper
                        && message.from == "Bob"
                        && message.to.as_deref() == Some("Alice")
                        && message.text == "hi back" =>
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
    .expect("timed out waiting for whisper reply");
    assert_eq!(reply.glyph, Some('B'));
}

#[tokio::test]
async fn test_new_clients_receive_recent_chat_history() {
    let app = TestApp::spawn().await;
    let first_stream = app.connect().await;
    let (first_reader, mut first_writer) = first_stream.into_split();
    let mut first_lines = BufReader::new(first_reader).lines();

    let _ = next_message(&mut first_lines).await.unwrap();
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
                text: "history check".to_string(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    first_writer.flush().await.unwrap();

    timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut first_lines).await.unwrap() {
                ServerMessage::Chat { message }
                    if message.kind == ChatKind::Player
                        && message.from == "Alice"
                        && message.text == "history check" =>
                {
                    break;
                }
                ServerMessage::Chat { .. }
                | ServerMessage::Snapshot { .. }
                | ServerMessage::Welcome { .. } => {}
                ServerMessage::Error { message } => panic!("unexpected server error: {message}"),
            }
        }
    })
    .await
    .expect("timed out waiting for initial public chat");

    let second_stream = app.connect().await;
    let (second_reader, _) = second_stream.into_split();
    let mut second_lines = BufReader::new(second_reader).lines();

    let _ = next_message(&mut second_lines).await.unwrap();
    let history_message = timeout(Duration::from_secs(1), async {
        loop {
            match next_message(&mut second_lines).await.unwrap() {
                ServerMessage::Chat { message }
                    if message.kind == ChatKind::Player
                        && message.from == "Alice"
                        && message.text == "history check" =>
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
    .expect("timed out waiting for history replay");

    assert_eq!(history_message.to, None);
}
