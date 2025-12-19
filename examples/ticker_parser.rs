use forge_sdk::network::connection::ConnectionManager;
use forge_sdk::model::message::KrakenMessage;
use tokio::sync::mpsc;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider().install_default().ok();
    tracing_subscriber::fmt::init();

    // 1. Connect
    let (tx_user_cmd, rx_engine_cmd) = mpsc::channel(32);
    let (tx_engine_event, mut rx_user_event) = mpsc::channel(100);

    let manager = ConnectionManager::new(
        "wss://ws.kraken.com/v2",
        tx_engine_event,
        rx_engine_cmd
    ).expect("Invalid URL");

    tokio::spawn(manager.run());

    // 2. Subscribe to BTC/USD ticker
    let subscribe_msg = r#"{
        "method": "subscribe",
        "params": {
            "channel": "ticker",
            "symbol": ["BTC/USD"]
        }
    }"#.to_string();

    tx_user_cmd.send(subscribe_msg).await.unwrap();

    println!("Listening & Parsing...");

    while let Some(msg_result) = rx_user_event.recv().await {
        if let Ok(json_text) = msg_result {
            let parsed: Result<KrakenMessage, _> = serde_json::from_str(&json_text);

            match parsed {
                Ok(KrakenMessage::Ticker(ticker)) => {
                    let data = &ticker.data[0];
                    println!(
                        "💰 {} | Bid: ${:.2} | Ask: ${:.2} | Vol: {:.2}", 
                        data.symbol, data.bid, data.ask, data.volume
                    );
                }
                Ok(KrakenMessage::Heartbeat { .. }) => {
                    println!("💓 Pulse");
                }
                Ok(KrakenMessage::Status { data, .. }) => {
                     println!("🟢 System Online. ID: {}", data[0].connection_id);
                }
                Ok(KrakenMessage::SubscriptionStatus { success, .. }) => {
                    println!("✅ Subscription success: {}", success);
                }
                Ok(KrakenMessage::Unknown(v)) => {
                    println!("? Unknown message: {:?}", v);
                }
                Err(e) => {
                    eprintln!("❌ Parse Error: {}", e);
                    // Print the raw string so we can debug why it failed
                    eprintln!("Raw: {}", json_text);
                }
            _ => {}
            }
            }
        }
    }