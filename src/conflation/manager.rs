//! The ConflationManager orchestrates adaptive backpressure handling.
//!
//! It sits between the WebSocket reader and the output channel, providing:
//! - Fast-path passthrough in Green state
//! - Smart batching in Yellow state  
//! - Aggressive conflation in Red state

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::error::KrakenError;

use super::buffer::{BufferedMessage, KeyedBuffer, MessageType, TradeAggregate};
use super::rules::{self, ParsedMessage};
use super::state::ConflationState;

/// Configuration for the ConflationManager.
#[derive(Debug, Clone)]
pub struct ConflationConfig {
    /// Channel capacity for state calculations (should match event_sender capacity)
    pub channel_capacity: usize,
    /// Enable conflation (can be disabled for debugging)
    pub enabled: bool,
    /// Log state transitions
    pub log_transitions: bool,
}

impl Default for ConflationConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 100,
            enabled: true,
            log_transitions: true,
        }
    }
}

/// The core ConflationManager that handles adaptive backpressure.
pub struct ConflationManager {
    /// The keyed buffer for pending messages
    buffer: KeyedBuffer,
    /// Current conflation state
    state: ConflationState,
    /// Previous state (for transition logging)
    prev_state: ConflationState,
    /// Configuration
    config: ConflationConfig,
    /// Trade aggregates keyed by symbol
    trade_aggregates: std::collections::HashMap<String, TradeAggregate>,
}

impl ConflationManager {
    /// Create a new ConflationManager with default configuration.
    pub fn new() -> Self {
        Self::with_config(ConflationConfig::default())
    }

    /// Create a new ConflationManager with custom configuration.
    pub fn with_config(config: ConflationConfig) -> Self {
        Self {
            buffer: KeyedBuffer::new(),
            state: ConflationState::Green,
            prev_state: ConflationState::Green,
            config,
            trade_aggregates: std::collections::HashMap::new(),
        }
    }

    /// Get current conflation state.
    pub fn state(&self) -> ConflationState {
        self.state
    }

    /// Get buffer statistics.
    pub fn stats(&self) -> &super::buffer::BufferStats {
        self.buffer.stats()
    }

    /// Get current buffer length.
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if conflation is currently active (not in Green state).
    pub fn is_conflating(&self) -> bool {
        self.state != ConflationState::Green
    }

    /// Update the conflation state based on channel capacity.
    pub fn update_state(&mut self, current_len: usize, max_capacity: usize) {
        self.prev_state = self.state;
        self.state = ConflationState::from_capacity(current_len, max_capacity);

        if self.config.log_transitions && self.state != self.prev_state {
            match self.state {
                ConflationState::Green => {
                    debug!("Conflation state: GREEN (passthrough mode)");
                }
                ConflationState::Yellow => {
                    warn!(
                        "Conflation state: YELLOW (batching mode) - channel at {}%",
                        (current_len as f64 / max_capacity as f64 * 100.0) as u32
                    );
                }
                ConflationState::Red => {
                    warn!(
                        "Conflation state: RED (aggressive conflation) - channel at {}%",
                        (current_len as f64 / max_capacity as f64 * 100.0) as u32
                    );
                }
            }
        }
    }

    /// Process an incoming message based on current state.
    ///
    /// Returns `Some(String)` if the message should be sent immediately (fast path).
    /// Returns `None` if the message was buffered for later.
    pub fn process(&mut self, raw: &str) -> Option<String> {
        if !self.config.enabled {
            // Conflation disabled, always passthrough
            return Some(raw.to_string());
        }

        // Parse message to extract metadata
        let parsed = ParsedMessage::parse(raw);

        // Priority messages always go through immediately
        if parsed.is_priority() {
            trace!("Priority message bypassing conflation: {}", parsed.channel);
            return Some(raw.to_string());
        }

        // In Green state, passthrough everything
        if self.state == ConflationState::Green {
            return Some(raw.to_string());
        }

        // Buffer the message for conflation
        self.buffer_message(parsed);
        None
    }

    /// Buffer a message for later sending with appropriate conflation.
    fn buffer_message(&mut self, parsed: ParsedMessage) {
        let should_conflate = self.state.requires_conflation();
        let symbol = parsed.symbol.clone().unwrap_or_default();

        // Special handling for trades - use aggregation
        if parsed.message_type == MessageType::Trade && should_conflate {
            self.aggregate_trade(&parsed.raw, &symbol);
            return;
        }

        // Create buffered message
        let message = BufferedMessage::new(
            parsed.raw.clone(),
            parsed.channel.clone(),
            parsed.symbol,
            parsed.message_type,
        );

        self.buffer.insert(message, should_conflate);
    }

    /// Aggregate a trade message into the running aggregate.
    fn aggregate_trade(&mut self, raw: &str, symbol: &str) {
        if let Some(existing) = self.trade_aggregates.get_mut(symbol) {
            rules::merge_trade_aggregates(existing, raw);
        } else if let Some(aggregate) = rules::create_trade_aggregate(raw) {
            self.trade_aggregates.insert(symbol.to_string(), aggregate);
        }
    }

    /// Try to flush buffered messages to the channel.
    ///
    /// This should be called regularly to drain the buffer when capacity is available.
    /// Returns the number of messages flushed.
    pub async fn try_flush(&mut self, sender: &mpsc::Sender<Result<String, KrakenError>>) -> usize {
        let mut flushed = 0;

        // First, flush any aggregated trades if we're transitioning out of Red
        if self.prev_state == ConflationState::Red && self.state != ConflationState::Red {
            flushed += self.flush_trade_aggregates(sender).await;
        }

        // Then flush the regular buffer
        while sender.capacity() > 0 {
            if let Some(msg) = self.buffer.pop() {
                match sender.try_send(Ok(msg)) {
                    Ok(()) => {
                        flushed += 1;
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Channel full again, stop flushing
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Channel closed, stop
                        break;
                    }
                }
            } else {
                // Buffer empty
                break;
            }
        }

        if flushed > 0 {
            trace!("Flushed {} messages from conflation buffer", flushed);
        }

        flushed
    }

    /// Flush all trade aggregates as batched/aggregated messages.
    async fn flush_trade_aggregates(
        &mut self,
        sender: &mpsc::Sender<Result<String, KrakenError>>,
    ) -> usize {
        let mut flushed = 0;

        let aggregates: Vec<_> = self.trade_aggregates.drain().collect();

        for (_, aggregate) in aggregates {
            // Decide format based on trade count
            let msg = if aggregate.trade_count > 10 {
                // Many trades: send aggregate summary
                rules::build_trade_aggregate_json(&aggregate)
            } else {
                // Few trades: send batch
                rules::build_trade_batch_json(&aggregate)
            };

            match sender.try_send(Ok(msg)) {
                Ok(()) => {
                    flushed += 1;
                }
                Err(_) => {
                    // Can't send, will be lost (acceptable in crisis mode)
                    break;
                }
            }
        }

        flushed
    }

    /// Force flush all buffered content (used during shutdown or reconnect).
    pub async fn force_flush(
        &mut self,
        sender: &mpsc::Sender<Result<String, KrakenError>>,
    ) -> usize {
        let mut flushed = 0;

        // Flush trade aggregates first
        flushed += self.flush_trade_aggregates(sender).await;

        // Then flush entire buffer
        while let Some(msg) = self.buffer.pop() {
            if sender.send(Ok(msg)).await.is_ok() {
                flushed += 1;
            } else {
                break;
            }
        }

        flushed
    }

    /// Clear all buffered state (used during disconnect).
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.trade_aggregates.clear();
        self.state = ConflationState::Green;
        self.prev_state = ConflationState::Green;
    }
}

impl Default for ConflationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_green_passthrough() {
        let mut manager = ConflationManager::new();
        manager.update_state(10, 100); // 10% = Green

        let msg = r#"{"channel":"ticker","data":[{"symbol":"BTC/USD"}]}"#;
        let result = manager.process(msg);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), msg);
    }

    #[test]
    fn test_red_buffers() {
        let mut manager = ConflationManager::new();
        manager.update_state(95, 100); // 95% = Red

        let msg = r#"{"channel":"ticker","data":[{"symbol":"BTC/USD"}]}"#;
        let result = manager.process(msg);

        assert!(result.is_none());
        assert_eq!(manager.buffer_len(), 1);
    }

    #[test]
    fn test_priority_bypass() {
        let mut manager = ConflationManager::new();
        manager.update_state(95, 100); // 95% = Red

        let msg = r#"{"channel":"heartbeat"}"#;
        let result = manager.process(msg);

        // Priority should bypass even in Red state
        assert!(result.is_some());
    }

    #[test]
    fn test_state_transitions() {
        let mut manager = ConflationManager::new();

        manager.update_state(10, 100);
        assert_eq!(manager.state(), ConflationState::Green);

        manager.update_state(60, 100);
        assert_eq!(manager.state(), ConflationState::Yellow);

        manager.update_state(95, 100);
        assert_eq!(manager.state(), ConflationState::Red);
    }
}
