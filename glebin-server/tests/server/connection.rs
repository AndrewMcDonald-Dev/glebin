use crate::helpers::TestApp;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[tokio::test]
async fn test_connection() {
    // Arrange
    let port = TestApp::spawn_server().await;
    println!("Waiting for client to connect...");

    // Act
    // Assert
    let _ = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_connection_with_message() {
    // Arrange
    let port = TestApp::spawn_server().await;
    println!("Waiting for client to connect...");
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();

    // Act
    let payload = serde_json::to_string(&(0.0, 0.0)).unwrap();
    println!("Sending message to client: {}", payload);
    stream.write_all(payload.as_bytes()).await.unwrap();
    println!("Flushing message to client...");
    stream.flush().await.unwrap();
    println!("Waiting for message from client...");
    stream.readable().await.unwrap();
    println!("Reading message from client...");
    let mut buffer = vec![0; 128];

    // Assert
    match stream.try_read(&mut buffer) {
        Ok(0) => {
            panic!("Client disconnected (client closed connection.)");
        }
        Ok(n) => {
            let message = String::from_utf8_lossy(&buffer[..n]);
            println!("Received message from client: {}", message);
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            panic!("Client disconnected (client closed connection.)");
        }
        Err(e) => {
            panic!("Socker read error for client: {:?}", e);
        }
    }
}
