use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    env_logger::init();
    let listener = TcpListener::bind("127.0.0.1:9132").await.unwrap();
    glebin_server::server::run(listener).await;
}
