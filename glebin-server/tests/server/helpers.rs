use std::{io, time::Duration};

use glebin_protocol::ServerMessage;
use tokio::{
    io::BufReader,
    net::{tcp::OwnedReadHalf, TcpListener, TcpStream},
    time::timeout,
};

pub struct TestApp {
    port: u16,
}

impl TestApp {
    pub async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            glebin_server::server::run(listener).await.unwrap();
        });

        Self { port }
    }

    pub async fn connect(&self) -> TcpStream {
        timeout(
            Duration::from_secs(1),
            TcpStream::connect(format!("127.0.0.1:{}", self.port)),
        )
        .await
        .expect("timed out connecting to test server")
        .expect("failed to connect to test server")
    }
}

pub async fn next_message(
    lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
) -> io::Result<ServerMessage> {
    let line = timeout(Duration::from_secs(1), lines.next_line())
        .await
        .expect("timed out waiting for server message")?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "server closed connection unexpectedly",
            )
        })?;

    serde_json::from_str(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
