use crate::error::KrakenError;
use crate::model::message::KrakenMessage;
use crate::network::connection::ConnectionManager;
use tokio::sync::mpsc;

pub struct KrakenClient {
    // Channel to send commands to the engine
    command_sender: mpsc::Sender<String>,

    //Channel to receive events from the engine
    // We wrap it in Option so we can take it if we want to split the stream
    event_receiver: Option<mpsc::Receiver<Result<String, KrakenError>>>,
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
        })
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
}
