//! Web Candlestick Chart Server
//!
//! A web server that serves historical OHLC data and live WebSocket updates.
//! Run with: cargo run --example chart_server
//! Then open: http://localhost:3000
//!
//! Configuration via environment variables:
//! - CHART_SERVER_HOST: Server bind address (default: 0.0.0.0)
//! - CHART_SERVER_PORT: Server port (default: 3000)
//! - KRAKEN_WS_URL: Kraken WebSocket URL (default: wss://ws.kraken.com/v2)
//! - KRAKEN_REST_URL: Kraken REST API base URL (default: https://api.kraken.com/0/public)
//! - TRADING_PAIRS: Comma-separated list of trading pairs (default: BTC/USD,ETH/USD,SOL/USD)

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{Html, IntoResponse},
    routing::get,
};
use forge_sdk::KrakenClient;
use forge_sdk::model::message::KrakenMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

// ============================================================================
// CONFIGURATION
// ============================================================================

/// Server configuration - all values can be overridden via environment variables
#[derive(Clone, Debug)]
pub struct Config {
    /// Server bind host
    pub host: String,
    /// Server port
    pub port: u16,
    /// Kraken WebSocket URL
    pub kraken_ws_url: String,
    /// Kraken REST API base URL
    pub kraken_rest_url: String,
    /// Trading pairs to subscribe to
    pub trading_pairs: Vec<String>,
    /// OHLC intervals to subscribe to (in minutes)
    pub intervals: Vec<i32>,
    /// Default interval for API queries
    pub default_interval: i32,
    /// Broadcast channel capacity
    pub broadcast_capacity: usize,
    /// Reconnect delay in seconds
    pub reconnect_delay_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
            kraken_ws_url: "wss://ws.kraken.com/v2".to_string(),
            kraken_rest_url: "https://api.kraken.com/0/public".to_string(),
            trading_pairs: vec![
                "BTC/USD".to_string(),
                "ETH/USD".to_string(),
                "SOL/USD".to_string(),
            ],
            intervals: vec![1, 5, 15, 30, 60, 240, 1440],
            default_interval: 60,
            broadcast_capacity: 100,
            reconnect_delay_secs: 5,
        }
    }
}

impl Config {
    /// Load configuration from environment variables with defaults
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(host) = std::env::var("CHART_SERVER_HOST") {
            config.host = host;
        }
        if let Ok(port) = std::env::var("CHART_SERVER_PORT") {
            if let Ok(p) = port.parse() {
                config.port = p;
            }
        }
        if let Ok(ws_url) = std::env::var("KRAKEN_WS_URL") {
            config.kraken_ws_url = ws_url;
        }
        if let Ok(rest_url) = std::env::var("KRAKEN_REST_URL") {
            config.kraken_rest_url = rest_url;
        }
        if let Ok(pairs) = std::env::var("TRADING_PAIRS") {
            config.trading_pairs = pairs.split(',').map(|s| s.trim().to_string()).collect();
        }

        config
    }

    /// Get the full server bind address
    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Get the OHLC REST endpoint URL
    pub fn ohlc_url(&self) -> String {
        format!("{}/OHLC", self.kraken_rest_url)
    }
}

// ============================================================================
// APPLICATION STATE
// ============================================================================

#[derive(Clone)]
struct AppState {
    tx: broadcast::Sender<String>,
    config: Arc<Config>,
}

// ============================================================================
// API TYPES
// ============================================================================

#[derive(Deserialize)]
struct OhlcQuery {
    pair: String,
    interval: Option<i32>,
}

#[derive(Serialize)]
struct OhlcCandle {
    time: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

// ============================================================================
// MAIN ENTRY POINT
// ============================================================================

#[tokio::main]
async fn main() {
    // Initialize TLS crypto provider
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    // Load configuration
    let config = Arc::new(Config::from_env());

    println!("🚀 Starting Kraken Chart Server...");
    println!("   Host: {}:{}", config.host, config.port);
    println!("   Pairs: {:?}", config.trading_pairs);
    println!("   Intervals: {:?}", config.intervals);

    // Create broadcast channel for live updates
    let (tx, _rx) = broadcast::channel::<String>(config.broadcast_capacity);
    let state = AppState {
        tx: tx.clone(),
        config: config.clone(),
    };

    // Start Kraken WebSocket listener in background
    let config_clone = config.clone();
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        run_kraken_ws(tx_clone, config_clone).await;
    });

    // Build router
    let app = Router::new()
        .route("/", get(serve_html))
        .route("/api/ohlc", get(get_historical_ohlc))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let bind_addr = config.bind_addr();
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
    println!(
        "📊 Chart server running at http://localhost:{}",
        config.port
    );

    axum::serve(listener, app).await.unwrap();
}

// ============================================================================
// HTTP HANDLERS
// ============================================================================

/// Serve the HTML page
async fn serve_html() -> Html<&'static str> {
    Html(include_str!("static/chart.html"))
}

/// Fetch historical OHLC from Kraken REST API
async fn get_historical_ohlc(
    Query(params): Query<OhlcQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let interval = params.interval.unwrap_or(state.config.default_interval);

    // Convert pair format: BTC/USD -> XBTUSD (Kraken's format)
    let kraken_pair = params.pair.replace("BTC", "XBT").replace("/", "");

    let url = format!(
        "{}?pair={}&interval={}",
        state.config.ohlc_url(),
        kraken_pair,
        interval
    );

    let response = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "error": format!("Failed to fetch data: {}", e)
            }));
        }
    };

    let json: serde_json::Value = match response.json().await {
        Ok(j) => j,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "error": format!("Failed to parse response: {}", e)
            }));
        }
    };

    // Parse Kraken OHLC response
    let candles = parse_kraken_ohlc_response(&json);

    axum::Json(serde_json::json!({
        "pair": params.pair,
        "interval": interval,
        "candles": candles
    }))
}

/// Parse Kraken's OHLC API response format
fn parse_kraken_ohlc_response(json: &serde_json::Value) -> Vec<OhlcCandle> {
    let mut candles = Vec::new();

    if let Some(result) = json.get("result") {
        for (key, value) in result.as_object().unwrap_or(&serde_json::Map::new()) {
            // Skip the "last" metadata field
            if key == "last" {
                continue;
            }

            if let Some(arr) = value.as_array() {
                for item in arr {
                    if let Some(candle) = parse_single_candle(item) {
                        candles.push(candle);
                    }
                }
            }
        }
    }

    candles
}

/// Parse a single candle from Kraken's array format
fn parse_single_candle(item: &serde_json::Value) -> Option<OhlcCandle> {
    let arr = item.as_array()?;
    if arr.len() < 5 {
        return None;
    }

    Some(OhlcCandle {
        time: arr[0].as_i64().unwrap_or(0),
        open: parse_price(&arr[1]),
        high: parse_price(&arr[2]),
        low: parse_price(&arr[3]),
        close: parse_price(&arr[4]),
    })
}

/// Parse price from Kraken's string format
fn parse_price(value: &serde_json::Value) -> f64 {
    value.as_str().unwrap_or("0").parse().unwrap_or(0.0)
}

// ============================================================================
// WEBSOCKET HANDLERS
// ============================================================================

/// WebSocket upgrade handler
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

/// Handle WebSocket connection - broadcast OHLC updates to client
async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();

    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg)).await.is_err() {
            break;
        }
    }
}

// ============================================================================
// KRAKEN WEBSOCKET CLIENT
// ============================================================================

/// Connect to Kraken WebSocket and broadcast OHLC updates
async fn run_kraken_ws(tx: broadcast::Sender<String>, config: Arc<Config>) {
    loop {
        println!(
            "📡 Connecting to Kraken WebSocket: {}",
            config.kraken_ws_url
        );

        let mut client = match KrakenClient::connect(&config.kraken_ws_url).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("❌ Connection error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(
                    config.reconnect_delay_secs,
                ))
                .await;
                continue;
            }
        };

        // Subscribe to OHLC for all configured pairs and intervals
        for pair in &config.trading_pairs {
            for interval in &config.intervals {
                let sub = format!(
                    r#"{{"method":"subscribe", "params":{{"channel":"ohlc", "symbol":["{}"], "interval":{}}}}}"#,
                    pair, interval
                );
                let _ = client.send_raw(sub).await;
            }
        }

        println!(
            "✅ Subscribed to {} pairs × {} intervals",
            config.trading_pairs.len(),
            config.intervals.len()
        );

        let mut stream = client.stream();
        while let Some(msg_res) = stream.recv().await {
            if let Ok(txt) = msg_res {
                if let Ok(msg) = serde_json::from_str::<KrakenMessage>(&txt) {
                    if let KrakenMessage::OHLC(ohlc_msg) = msg {
                        let update = build_ohlc_update(&ohlc_msg);
                        let _ = tx.send(update);
                    }
                }
            }
        }

        println!(
            "⚠️ WebSocket disconnected, reconnecting in {}s...",
            config.reconnect_delay_secs
        );
        tokio::time::sleep(tokio::time::Duration::from_secs(
            config.reconnect_delay_secs,
        ))
        .await;
    }
}

/// Build JSON update message for WebSocket clients
fn build_ohlc_update(ohlc_msg: &forge_sdk::model::ohlc::OHLCMessage) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let update = serde_json::json!({
        "type": "ohlc",
        "data": ohlc_msg.data.iter().map(|c| {
            serde_json::json!({
                "symbol": c.symbol,
                "interval": c.interval,
                "time": now,
                "open": c.open,
                "high": c.high,
                "low": c.low,
                "close": c.close,
            })
        }).collect::<Vec<_>>()
    });

    update.to_string()
}
