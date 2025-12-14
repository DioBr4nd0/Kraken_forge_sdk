use forge_sdk::network::connection::ConnectionManager;

use tokio::sync::mpsc;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // 1. Setup logging so we can see what's happening
    tracing_subscriber::fmt::init();

    // 2. Create channels
    let (tx_user_cmd, rx_engine_cmd) = mpsc::channel(32);
    let (tx_engine_event, mut rx_user_event) = mpsc::channel(100);

    // 3. Spawn the Engine
    let manager = ConnectionManager::new(
        "wss://ws.kraken.com/v2",
        tx_engine_event,
        rx_engine_cmd
    ).expect("Invalid URL");

    tokio::spawn(manager.run());

    // 4. Send a manual subscription command (Raw JSON)
    let subscribe_msg = r#"{
        "method": "subscribe",
        "params": {
            "channel": "ticker",
            "symbol": ["BTC/USD"]
        }
    }"#.to_string();

    println!("Sending subscription...");
    tx_user_cmd.send(subscribe_msg).await.unwrap();

    // 5. Listen for the response
    while let Some(msg) = rx_user_event.recv().await {
        match msg {
            Ok(text) => println!("Received: {}", text),
            Err(e) => eprintln!("Error: {}", e),
        }
    }
}