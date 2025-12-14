use forge_sdk::KrakenClient;
use forge_sdk::book::checksum::validate_checksum;
use forge_sdk::book::orderbook::LocalBook;
use forge_sdk::model::message::{BookData, BookLevel, BookMessage, KrakenMessage};

use regex::Regex;

fn manually_parse_book_message(json: &str) -> Result<KrakenMessage, serde_json::Error> {
    let channel_re = Regex::new(r#""channel":"([^"]+)""#).unwrap();
    let type_re = Regex::new(r#""type":"([^"]+)""#).unwrap();
    let symbol_re = Regex::new(r#""symbol":"([^"]+)""#).unwrap();
    let checksum_re = Regex::new(r#""checksum":(\d+)"#).unwrap();

    let channel = channel_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let type_str = type_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let symbol = symbol_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let checksum: u32 = checksum_re
        .captures(json)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);

    let bids = extract_price_qty_array_from_raw(json, "bids");
    let asks = extract_price_qty_array_from_raw(json, "asks");

    let book_data = BookData {
        symbol,
        checksum,
        bids,
        asks,
    };

    let book_msg = BookMessage {
        channel,
        r#type: type_str,
        data: vec![book_data],
    };

    Ok(KrakenMessage::Book(book_msg))
}

fn extract_price_qty_array_from_raw(json: &str, field_name: &str) -> Vec<BookLevel> {
    let array_pattern = format!(r#""{}":\[(.*?)\]"#, field_name);
    let array_re = Regex::new(&array_pattern).unwrap();

    let array_content = array_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("");

    let obj_re = Regex::new(r#"\{"price":([\d.]+),"qty":([\d.]+)\}"#).unwrap();

    obj_re
        .captures_iter(array_content)
        .map(|cap| {
            let price = cap.get(1).unwrap().as_str().to_string();
            let qty = cap.get(2).unwrap().as_str().to_string();
            BookLevel { price, qty }
        })
        .collect()
}

#[tokio::main]
async fn main() {
    // 1. Setup
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    println!("🚀 Starting Kraken Forge Client...");

    // 2. Connect (Using our new Facade!)
    let mut client = KrakenClient::connect("wss://ws.kraken.com/v2")
        .await
        .expect("Failed to connect");
    println!("✅ Connected.");

    // 3. Subscribe
    client
        .subscribe_book(&["BTC/USD"], 10)
        .await
        .expect("Failed to sub");

    println!("✅ Subscribed to Book.");

    // NEW: Subscribe to Trades
    let trade_sub = r#"{
        "method": "subscribe",
        "params": {
            "channel": "trade",
            "symbol": ["BTC/USD"]
        }
    }"#
    .to_string();
    client.send_command(trade_sub).await.unwrap();

    // 4. The Loop
    let mut book = LocalBook::new();
    let mut stream = client.stream();

    println!("📡 Listening for data...");

    while let Some(msg_res) = stream.recv().await {
        if let Ok(txt) = msg_res {
            let parsed = if txt.contains("\"channel\":\"book\"") {
                manually_parse_book_message(&txt)
            } else {
                serde_json::from_str::<KrakenMessage>(&txt)
            };

            match parsed {
                Ok(parsed) => {
                    match parsed {
                        KrakenMessage::Book(msg) => {
                            let data = &msg.data[0];

                            if msg.r#type == "snapshot" {
                                println!(
                                    "📸 Snapshot: {} bids, {} asks",
                                    data.bids.len(),
                                    data.asks.len()
                                );
                                book.apply_snapshot(data.bids.clone(), data.asks.clone());
                            } else {
                                book.apply_updates(data.bids.clone(), true);
                                book.apply_updates(data.asks.clone(), false);
                            }

                            let is_valid = validate_checksum(&book, data.checksum);
                            if is_valid {
                                println!("✅ MATCH! (RPC Checksum: {})", data.checksum);
                            } else {
                                println!("❌ FAIL! (RPC Checksum: {})", data.checksum);
                            }
                        }
                        KrakenMessage::Trade(msg) => {
                            for trade in msg.data {
                                let value = trade.price * trade.qty;
                                if value > 10000.0 {
                                    // Filter for trades > $10k
                                    println!(
                                        "🚨 WHALE ALERT: {} bought/sold {:.4} BTC (${:.2})",
                                        trade.side.to_uppercase(),
                                        trade.qty,
                                        value
                                    );
                                } else {
                                    // Optional: Print small trades
                                    // println!("Trade: {} ${}", trade.side, trade.price);
                                }
                            }
                        }
                        KrakenMessage::Heartbeat { channel } => {
                            if channel == "heartbeat" {
                                println!("💓 Heartbeat");
                            }
                        }
                        KrakenMessage::SubscriptionStatus { success, .. } => {
                            println!("🔔 Subscription Status: Success={}", success);
                        }
                        _ => println!("ℹ️ Other Message: {:?}", parsed),
                    }
                }
                Err(e) => {
                    eprintln!("Parse Error: {}", e);
                }
            }
        }
    }
}
