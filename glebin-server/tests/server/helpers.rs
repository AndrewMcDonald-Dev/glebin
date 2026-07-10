use std::{io, time::Duration};

use glebin_protocol::ServerMessage;
use tokio::{
    io::BufReader,
    net::{tcp::OwnedReadHalf, TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

pub struct TestApp {
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
}

impl TestApp {
    pub async fn spawn() -> Self {
        Self::spawn_with_config(glebin_server::server::ServerConfig::default()).await
    }

    pub async fn spawn_with_config(config: glebin_server::server::ServerConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            glebin_server::server::run_with_config_until(listener, config, async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
        });

        Self {
            port,
            shutdown: Some(shutdown_tx),
        }
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

impl Drop for TestApp {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
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
