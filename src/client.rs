use crate::error::KrakenError;
use crate::network::connection::ConnectionManager;
use tokio::sync::mpsc;

pub struct KrakenClient {
    // Channel to send commands to the engine
    command_sender: mpsc::Sender<String>,

    //Channel to receive events from the engine
    // We wrap it in Option so we can take it if we want to split the stream
    event_receiver: Option<mpsc::Receiver<Result<String, KrakenError>>>,

    // Auth fields (optional)
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub token: Option<String>,
}

impl KrakenClient {
    /// Connects to Kraken Websocket API
    pub async fn connect(url: &str) -> Result<Self, KrakenError> {
        let (tx_user_cmd, rx_engine_cmd) = mpsc::channel(32);
        let (tx_engine_event, rx_user_event) = mpsc::channel(100);

        // Span the Engine in background
        // Note:: We use unwrap here just to match your current error handling style,
        // but normally we'd pass the error up.
        let manager = ConnectionManager::new(url, tx_engine_event, rx_engine_cmd)
            .map_err(|e| KrakenError::ConnectionError(e.to_string()))?;

        tokio::spawn(manager.run());

        Ok(Self {
            command_sender: tx_user_cmd,
            event_receiver: Some(rx_user_event),
            api_key: None,
            api_secret: None,
            token: None,
        })
    }

    /// Login with API credentials (for authenticated feeds)
    pub async fn login(&mut self, api_key: String, api_secret: String) -> Result<(), KrakenError> {
        // Fetch WebSocket token from REST API
        let token = crate::auth::get_ws_token(&api_key, &api_secret)
            .await
            .map_err(|e| KrakenError::AuthError(e.to_string()))?;

        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
        self.token = Some(token);

        Ok(())
    }

    /// Place a new order (requires authentication)
    ///
    /// # Arguments
    /// * `symbol` - Trading pair (e.g., "BTC/USD")
    /// * `side` - "buy" or "sell"
    /// * `order_type` - "limit" or "market"
    /// * `quantity` - Order quantity
    /// * `price` - Limit price (optional for market orders)
    ///
    /// # Safety
    /// This method places real orders. Use with caution and test with small amounts first.
    pub async fn add_order(
        &self,
        symbol: &str,
        side: &str,
        order_type: &str,
        quantity: f64,
        price: Option<f64>,
    ) -> Result<(), KrakenError> {
        let token = self.token.as_ref().ok_or_else(|| {
            KrakenError::AuthError("Not authenticated. Call login() first.".to_string())
        })?;

        let mut params = serde_json::json!({
            "method": "add_order",
            "params": {
                "token": token,
                "order_type": order_type,
                "side": side,
                "symbol": symbol,
                "order_qty": quantity,
                "time_in_force": "gtc"
            }
        });

        // Add limit price if provided
        if let Some(p) = price {
            params["params"]["limit_price"] = serde_json::json!(p);
        }

        self.send_command(params.to_string()).await
    }

    /// Cancel one or more orders (requires authentication)
    ///
    /// # Arguments
    /// * `order_ids` - List of order IDs to cancel
    ///
    /// # Safety
    /// This method cancels real orders. Ensure you're canceling the correct orders.
    pub async fn cancel_order(&self, order_ids: &[&str]) -> Result<(), KrakenError> {
        let token = self.token.as_ref().ok_or_else(|| {
            KrakenError::AuthError("Not authenticated. Call login() first.".to_string())
        })?;

        let payload = serde_json::json!({
            "method": "cancel_order",
            "params": {
                "token": token,
                "order_id": order_ids
            }
        });

        self.send_command(payload.to_string()).await
    }

    /// Subscribe to private ownTrades feed (requires authentication)
    pub async fn subscribe_own_trades(&self) -> Result<(), KrakenError> {
        let token = self.token.as_ref().ok_or_else(|| {
            KrakenError::AuthError("Not authenticated. Call login() first.".to_string())
        })?;

        let payload = serde_json::json!({
            "method": "subscribe",
            "params": {
                "channel": "ownTrades",
                "token": token,
                "snapshot": true
            }
        });

        self.send_command(payload.to_string()).await
    }

    /// Subscribe to private openOrders feed (requires authentication)
    pub async fn subscribe_open_orders(&self) -> Result<(), KrakenError> {
        let token = self.token.as_ref().ok_or_else(|| {
            KrakenError::AuthError("Not authenticated. Call login() first.".to_string())
        })?;

        let payload = serde_json::json!({
            "method": "subscribe",
            "params": {
                "channel": "openOrders",
                "token": token,
                "snapshot": true
            }
        });

        self.send_command(payload.to_string()).await
    }
    /// Subscribe to the Ticker feed
    pub async fn subscribe_ticker(&self, pairs: &[&str]) -> Result<(), KrakenError> {
        let payload = serde_json::json!({
            "method":"subscribe",
            "params" : {
                "channel" :"ticker",
                "symbol" : pairs
             }
        });
        self.send_command(payload.to_string()).await
    }

    /// Subscribe to the Order Book (Depth 10 is standard for visualizers)
    pub async fn subscribe_book(&self, pairs: &[&str], depth: u32) -> Result<(), KrakenError> {
        let payload = serde_json::json!({
            "method": "subscribe",
            "params": {
                "channel": "book",
                "symbol": pairs,
                "depth": depth
            }
        });
        self.send_command(payload.to_string()).await
    }

    /// Helper to send JSON commands
    pub async fn send_command(&self, json: String) -> Result<(), KrakenError> {
        self.command_sender
            .send(json)
            .await
            .map_err(|_| KrakenError::ChannelClosed)
    }

    /// Get the event stream to listen for updates
    pub fn stream(&mut self) -> mpsc::Receiver<Result<String, KrakenError>> {
        self.event_receiver.take().expect("Stream already taken!")
    }

    pub async fn send_raw(&self, json: String) -> Result<(), KrakenError> {
        self.command_sender
            .send(json)
            .await
            .map_err(|_| KrakenError::ChannelClosed)
    }
}
