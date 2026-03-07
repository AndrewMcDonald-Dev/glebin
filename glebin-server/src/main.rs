use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let bind_address =
        std::env::var("GLEBIN_BIND").unwrap_or_else(|_| "127.0.0.1:9132".to_string());
    let listener = TcpListener::bind(&bind_address).await?;
    glebin_server::server::run(listener).await?;

    Ok(())
}
