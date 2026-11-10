//! Connects to tests/signalr-hub with a bearer token and calls `Status`.
//! Usage: signalr-crate-trial [port]
use signalr_client::SignalRClient;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let port: i32 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(5199);
    let connected = SignalRClient::connect_with("127.0.0.1", "hub", |c| {
        c.with_port(port);
        c.unsecure();
        c.authenticate_bearer("good".to_string());
    })
    .await;

    match connected {
        Err(error) => {
            println!("connect: FAILED: {error}");
            std::process::exit(1);
        }
        Ok(mut client) => {
            let status = client.invoke::<serde_json::Value>("Status".to_string()).await;
            println!("Status: {status:?}");
        }
    }
}
