//! Auth API test - tests WebSocket token retrieval and private feeds

use forge_sdk::KrakenClient;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    dotenvy::dotenv().ok();
    
    
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

   
    let api_key = env::var("KRAKEN_API_KEY")
        .expect("KRAKEN_API_KEY not set in .env");
    let api_secret = env::var("KRAKEN_API_SECRET")
        .expect("KRAKEN_API_SECRET not set in .env");

    println!("=== Kraken Auth API Test ===\n");
    println!("API Key: {}...", &api_key[..12]);

    // Connect to Kraken
    println!("\n[1] Connecting to Kraken WS...");
    let mut client = KrakenClient::connect("wss://ws.kraken.com/v2").await?;
    println!("    ✓ Connected");

    // Login with API credentials (fetches WS token)
    println!("\n[2] Authenticating with REST API...");
    match client.login(api_key, api_secret).await {
        Ok(()) => {
            println!("    ✓ Auth successful! Token received.");
        }
        Err(e) => {
            println!("    ✗ Auth failed: {}", e);
            return Err(e.into());
        }
    }

    // Subscribe to private feeds 
    println!("\n[3] Subscribing to ownTrades feed...");
    match client.subscribe_own_trades().await {
        Ok(()) => println!("    ✓ Subscribed to ownTrades"),
        Err(e) => println!("    ✗ Failed: {}", e),
    }

    println!("\n[4] Subscribing to openOrders feed...");
    match client.subscribe_open_orders().await {
        Ok(()) => println!("    ✓ Subscribed to openOrders"),
        Err(e) => println!("    ✗ Failed: {}", e),
    }

    println!("\n[5] Listening for messages (10 seconds)...\n");
    
    let mut stream = client.stream();
    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    let mut msg_count = 0;
    loop {
        tokio::select! {
            _ = &mut timeout => {
                println!("\n    Done! Received {} messages.", msg_count);
                break;
            }
            msg = stream.recv() => {
                match msg {
                    Some(Ok(data)) => {
                        msg_count += 1;
                        // Print first 200 chars
                        let preview: String = data.chars().take(200).collect();
                        println!("    MSG {}: {}", msg_count, preview);
                    }
                    Some(Err(e)) => {
                        println!("    ERR: {}", e);
                    }
                    None => {
                        println!("    Stream closed");
                        break;
                    }
                }
            }
        }
    }

    println!("\n=== Auth Test Complete ===");
    Ok(())
}
