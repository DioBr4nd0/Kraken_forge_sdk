//! WebSocket connection manager with conflation and Governor support.

use crate::conflation::{ConflationManager, ConflationState};
use crate::error::KrakenError;
use crate::governor::Governor;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Duration, interval, sleep};

use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};
use url::Url;

/// Type alias for the WebSocket stream.
#[allow(dead_code)]
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Channel capacity for the event sender (used for state calculations).
const CHANNEL_CAPACITY: usize = 100;

/// Interval for flush checks when buffer has pending messages.
const FLUSH_INTERVAL_MS: u64 = 10;

/// Manages WebSocket connection with conflation support.
pub struct ConnectionManager {
    url: Url,
    event_sender: mpsc::Sender<Result<String, KrakenError>>,
    command_receiver: mpsc::Receiver<String>,
    active_subscriptions: Vec<String>,
    /// The adaptive conflation manager
    conflation: ConflationManager,
    /// Resource-Aware Governor for CPU/RAM monitoring
    governor: Option<Arc<Governor>>,
}

impl ConnectionManager {
    /// Create a new ConnectionManager.
    pub fn new(
        url: &str,
        event_sender: mpsc::Sender<Result<String, KrakenError>>,
        command_receiver: mpsc::Receiver<String>,
    ) -> Result<Self, KrakenError> {
        Ok(Self {
            url: Url::parse(url)?,
            event_sender,
            command_receiver,
            active_subscriptions: Vec::new(),
            conflation: ConflationManager::new(),
            governor: None,
        })
    }

    /// Create a new ConnectionManager with a Governor.
    ///
    /// # Arguments
    /// * `url` - WebSocket URL to connect to
    /// * `event_sender` - Channel to send events to the client
    /// * `command_receiver` - Channel to receive commands from the client
    /// * `governor` - Resource-Aware Governor for system monitoring
    pub fn with_governor(
        url: &str,
        event_sender: mpsc::Sender<Result<String, KrakenError>>,
        command_receiver: mpsc::Receiver<String>,
        governor: Arc<Governor>,
    ) -> Result<Self, KrakenError> {
        Ok(Self {
            url: Url::parse(url)?,
            event_sender,
            command_receiver,
            active_subscriptions: Vec::new(),
            conflation: ConflationManager::new(),
            governor: Some(governor),
        })
    }

    /// Get the current conflation state.
    pub fn conflation_state(&self) -> ConflationState {
        self.conflation.state()
    }

    /// Get the current channel usage ratio for monitoring.
    #[allow(dead_code)]
    fn channel_usage(&self) -> f64 {
        let capacity = self.event_sender.capacity();
        let used = CHANNEL_CAPACITY.saturating_sub(capacity);
        used as f64 / CHANNEL_CAPACITY as f64
    }

    /// Update conflation state based on current channel capacity.
    fn update_conflation_state(&mut self) {
        let capacity = self.event_sender.capacity();
        let current_len = CHANNEL_CAPACITY.saturating_sub(capacity);
        self.conflation.update_state(current_len, CHANNEL_CAPACITY);
    }

    /// Send a message through the conflation engine.
    ///
    /// This is the core of the Adaptive Conflation Engine:
    /// - In Green state: immediate passthrough
    /// - In Yellow/Red state: buffer for conflation
    ///
    /// Additionally, the Resource-Aware Governor can trigger Survival mode
    /// to drop non-critical messages when CPU/RAM is under pressure.
    async fn send_with_conflation(&mut self, text: &str) -> Result<(), ()> {
        // FAST PATH: Governor-based load shedding (nanosecond check)
        if let Some(ref governor) = self.governor {
            if governor.is_survival() {
                // In Survival mode: skip non-critical messages entirely
                // This is the cheapest possible check - no JSON parsing
                if self.is_droppable_message(text) {
                    trace!("Governor Survival: dropping non-critical message");
                    return Ok(());
                }
            }
        }

        // Update state based on current channel pressure
        self.update_conflation_state();

        // Process through conflation manager
        if let Some(msg) = self.conflation.process(text) {
            // Fast path: message should be sent immediately
            match self.event_sender.try_send(Ok(msg)) {
                Ok(()) => {
                    trace!("Fast path send successful");
                    return Ok(());
                }
                Err(mpsc::error::TrySendError::Full(msg)) => {
                    // Channel full, extract the message and buffer it
                    if let Ok(inner) = msg {
                        // Re-parse and buffer since channel was full
                        let _ = self.conflation.process(&inner);
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    // Client dropped
                    return Err(());
                }
            }
        }

        // Try to flush buffer when there's capacity
        if self.event_sender.capacity() > 0 {
            self.conflation.try_flush(&self.event_sender).await;
        }

        Ok(())
    }

    /// Check if a message can be dropped in Survival mode.
    ///
    /// # Performance
    /// Uses cheap string contains checks instead of JSON parsing.
    /// This runs in the hot path so must be extremely fast.
    #[inline]
    fn is_droppable_message(&self, text: &str) -> bool {
        // Heartbeats are always droppable
        if text.contains("\"heartbeat\"") {
            return true;
        }
        // System status messages are droppable
        if text.contains("\"status\"") && !text.contains("\"error\"") {
            return true;
        }
        // Subscription confirmations during survival can be deferred
        // but we keep them for now as they're rare
        false
    }

    /// The main event loop - runs forever until the app closes.
    ///
    /// This loop:
    /// 1. Connects to the WebSocket server
    /// 2. Resubscribes to any active subscriptions on reconnect
    /// 3. Processes incoming messages through the conflation engine
    /// 4. Sends commands to the server
    /// 5. Periodically flushes buffered messages
    pub async fn run(mut self) {
        loop {
            info!("Connecting to {}...", self.url);

            match connect_async(self.url.as_str()).await {
                Ok((ws_stream, _)) => {
                    info!("Connected to Kraken");

                    // Clear conflation state on new connection
                    self.conflation.clear();

                    // Split the socket
                    // 'write' sends messages to Kraken
                    // 'read' listens for messages from Kraken
                    let (mut write, mut read) = ws_stream.split();

                    // If we have history, replay it immediately upon connection
                    if !self.active_subscriptions.is_empty() {
                        info!(
                            "Resubscribing to {} active channels...",
                            self.active_subscriptions.len()
                        );
                        for cmd in &self.active_subscriptions {
                            if let Err(e) = write.send(Message::Text(cmd.clone().into())).await {
                                error!("Failed to resubscribe: {}", e);
                            }
                        }
                    }

                    // Flush interval for draining buffer
                    let mut flush_interval = interval(Duration::from_millis(FLUSH_INTERVAL_MS));

                    loop {
                        tokio::select! {
                            // Bias toward reading to prevent WebSocket buffer buildup
                            biased;

                            // Handle incoming WebSocket messages
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if self.send_with_conflation(&text).await.is_err() {
                                            break; // Client dropped, stop engine
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        // Auto reply with pong
                                        if let Err(e) = write.send(Message::Pong(data)).await {
                                            error!("Failed to send pong: {}", e);
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        info!("Server sent close frame");
                                        break;
                                    }
                                    Some(Err(e)) => {
                                        error!("WebSocket error: {}", e);
                                        break;
                                    }
                                    None => {
                                        warn!("Stream ended unexpectedly");
                                        break;
                                    }
                                    _ => {}
                                }
                            }

                            // Handle commands from the client
                            cmd = self.command_receiver.recv() => {
                                match cmd {
                                    Some(payload) => {
                                        // Track subscriptions for reconnection
                                        if payload.contains("\"subscribe\"") {
                                            self.active_subscriptions.push(payload.clone());
                                        }

                                        if let Err(e) = write.send(Message::Text(payload.into())).await {
                                            error!("Failed to send message to Kraken: {}", e);
                                            break;
                                        }
                                    }
                                    None => {
                                        info!("Client command channel closed, shutting down...");

                                        // Force flush remaining buffer before shutdown
                                        let flushed = self.conflation.force_flush(&self.event_sender).await;
                                        if flushed > 0 {
                                            debug!("Flushed {} messages on shutdown", flushed);
                                        }

                                        return;
                                    }
                                }
                            }

                            // Periodic flush of buffered messages
                            _ = flush_interval.tick() => {
                                // Only flush if there's something in the buffer and channel has capacity
                                if self.conflation.buffer_len() > 0 && self.event_sender.capacity() > 0 {
                                    self.conflation.try_flush(&self.event_sender).await;
                                }

                                // Log conflation stats periodically in non-green states
                                if self.conflation.is_conflating() {
                                    let stats = self.conflation.stats();
                                    debug!(
                                        "Conflation active: state={}, buffered={}, conflated={}, flushed={}",
                                        self.conflation.state(),
                                        self.conflation.buffer_len(),
                                        stats.messages_conflated,
                                        stats.messages_flushed
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Connection failed: {}. Retrying...", e);
                }
            }

            // Log stats on disconnect
            let stats = self.conflation.stats();
            if stats.messages_buffered > 0 {
                info!(
                    "Conflation stats: buffered={}, conflated={}, flushed={}, overflow_drops={}",
                    stats.messages_buffered,
                    stats.messages_conflated,
                    stats.messages_flushed,
                    stats.overflow_drops
                );
            }

            // Wait before reconnecting
            sleep(Duration::from_secs(5)).await;
        }
    }
}
