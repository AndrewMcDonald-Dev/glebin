use tokio::net::TcpListener;

pub struct TestApp;

impl TestApp {
    pub async fn spawn_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(glebin::server::run(listener));
        port
    }
}
