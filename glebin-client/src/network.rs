use std::io;

use futures_util::StreamExt;
use glebin_protocol::{
    to_line, ClientMessage, ServerMessage, Snapshot, MAX_SERVER_LINE_LEN, PROTOCOL_VERSION,
};
use tokio::{
    io::AsyncWriteExt,
    net::{tcp::OwnedWriteHalf, TcpStream},
    sync::{mpsc, watch},
};
use tokio_util::codec::{FramedRead, LinesCodec};

#[derive(Debug)]
pub enum AppEvent {
    Server(ServerMessage),
    Disconnected(String),
}

pub async fn connect(
    address: &str,
    requested_name: String,
) -> io::Result<(
    OwnedWriteHalf,
    mpsc::Receiver<AppEvent>,
    watch::Receiver<Option<Snapshot>>,
)> {
    let stream = TcpStream::connect(address).await?;
    let (reader, mut writer) = stream.into_split();
    let (event_tx, event_rx) = mpsc::channel::<AppEvent>(128);
    let (snapshot_tx, snapshot_rx) = watch::channel::<Option<Snapshot>>(None);

    let name_payload = to_line(&ClientMessage::SetName {
        name: requested_name,
    })
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(name_payload.as_bytes()).await?;

    tokio::spawn(async move {
        let mut lines =
            FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_SERVER_LINE_LEN));
        while let Some(line_result) = lines.next().await {
            let line = match line_result {
                Ok(line) => line,
                Err(error) => {
                    let _ = event_tx
                        .send(AppEvent::Disconnected(format!(
                            "Invalid server frame: {error}"
                        )))
                        .await;
                    return;
                }
            };
            match serde_json::from_str::<ServerMessage>(&line) {
                Ok(ServerMessage::Welcome {
                    protocol_version, ..
                }) if protocol_version != PROTOCOL_VERSION => {
                    let _ = event_tx
                        .send(AppEvent::Disconnected(format!(
                            "Protocol mismatch: server {protocol_version}, client {PROTOCOL_VERSION}"
                        )))
                        .await;
                    return;
                }
                Ok(ServerMessage::Snapshot { snapshot }) => {
                    snapshot_tx.send_replace(Some(snapshot));
                }
                Ok(message) => {
                    if event_tx.send(AppEvent::Server(message)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = event_tx
                        .send(AppEvent::Disconnected(format!(
                            "Invalid server message: {error}"
                        )))
                        .await;
                    return;
                }
            }
        }
        let _ = event_tx
            .send(AppEvent::Disconnected(
                "Server closed the connection".to_string(),
            ))
            .await;
    });

    Ok((writer, event_rx, snapshot_rx))
}
