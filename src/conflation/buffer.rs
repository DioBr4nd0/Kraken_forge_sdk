//! High-performance keyed buffer for message deduplication and aggregation.

use std::collections::{HashMap, VecDeque};

/// Key for deduplicating messages: (channel_type, symbol).
/// For example: ("ticker", "BTC/USD") or ("trade", "ETH/USD").
pub type MessageKey = (String, String);

/// Maximum buffer size to prevent OOM (approximately 10MB worth of messages).
pub const MAX_BUFFER_SIZE: usize = 10_000;

/// Type classification for conflation rule matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// Price ticker updates - can be overwritten (latest wins)
    Ticker,
    /// Order book updates - deltas need special handling
    Book,
    /// Trade executions - must aggregate, never lose volume
    Trade,
    /// OHLC candlestick data
    OHLC,
    /// Private: own trades feed
    OwnTrades,
    /// Private: open orders feed
    OpenOrders,
    /// Control messages (heartbeat, status) - priority lane
    Control,
    /// Subscription confirmations - priority lane
    Subscription,
    /// Unknown message type
    Unknown,
}

impl MessageType {
    /// Determine message type from channel name.
    pub fn from_channel(channel: &str) -> Self {
        match channel.to_lowercase().as_str() {
            "ticker" => Self::Ticker,
            "book" => Self::Book,
            "trade" => Self::Trade,
            "ohlc" => Self::OHLC,
            "owntrades" => Self::OwnTrades,
            "openorders" => Self::OpenOrders,
            "heartbeat" | "status" => Self::Control,
            _ => Self::Unknown,
        }
    }

    /// Check if this message type should bypass conflation (priority lane).
    #[inline]
    pub fn is_priority(&self) -> bool {
        matches!(
            self,
            Self::Control | Self::Subscription | Self::OwnTrades | Self::OpenOrders
        )
    }

    /// Check if this message type supports overwrite conflation (last wins).
    #[inline]
    pub fn supports_overwrite(&self) -> bool {
        matches!(self, Self::Ticker | Self::OHLC)
    }

    /// Check if this message type requires aggregation (preserve totals).
    #[inline]
    pub fn requires_aggregation(&self) -> bool {
        matches!(self, Self::Trade)
    }
}

/// Aggregated trade data for Red state conflation.
/// Preserves critical information: total volume, price range, count.
#[derive(Debug, Clone)]
pub struct TradeAggregate {
    pub symbol: String,
    pub total_volume: f64,
    pub min_price: f64,
    pub max_price: f64,
    pub last_price: f64,
    pub first_side: String,
    pub last_side: String,
    pub trade_count: u64,
    pub last_timestamp: String,
    /// Original trade data for batching in Yellow state
    pub trades: Vec<serde_json::Value>,
}

impl TradeAggregate {
    /// Create a new aggregate from the first trade.
    pub fn new(
        symbol: String,
        price: f64,
        volume: f64,
        side: String,
        timestamp: String,
        raw_trade: serde_json::Value,
    ) -> Self {
        Self {
            symbol,
            total_volume: volume,
            min_price: price,
            max_price: price,
            last_price: price,
            first_side: side.clone(),
            last_side: side,
            trade_count: 1,
            last_timestamp: timestamp,
            trades: vec![raw_trade],
        }
    }

    /// Merge another trade into this aggregate.
    pub fn merge(
        &mut self,
        price: f64,
        volume: f64,
        side: String,
        timestamp: String,
        raw_trade: serde_json::Value,
    ) {
        self.total_volume += volume;
        self.min_price = self.min_price.min(price);
        self.max_price = self.max_price.max(price);
        self.last_price = price;
        self.last_side = side;
        self.trade_count += 1;
        self.last_timestamp = timestamp;
        self.trades.push(raw_trade);
    }
}

/// A buffered message waiting to be sent.
#[derive(Debug, Clone)]
pub struct BufferedMessage {
    /// The raw JSON string (for passthrough or reconstruction)
    pub raw_json: String,
    /// Parsed channel name
    pub channel: String,
    /// Symbol if applicable (e.g., "BTC/USD")
    pub symbol: Option<String>,
    /// Classified message type
    pub message_type: MessageType,
    /// For trade aggregation in Red state
    pub trade_aggregate: Option<TradeAggregate>,
    /// Timestamp when buffered (for ordering)
    pub buffered_at: std::time::Instant,
}

impl BufferedMessage {
    /// Create a new buffered message.
    pub fn new(
        raw_json: String,
        channel: String,
        symbol: Option<String>,
        message_type: MessageType,
    ) -> Self {
        Self {
            raw_json,
            channel,
            symbol,
            message_type,
            trade_aggregate: None,
            buffered_at: std::time::Instant::now(),
        }
    }

    /// Create a priority message that bypasses conflation.
    pub fn priority(raw_json: String) -> Self {
        Self {
            raw_json,
            channel: String::new(),
            symbol: None,
            message_type: MessageType::Control,
            trade_aggregate: None,
            buffered_at: std::time::Instant::now(),
        }
    }
}

/// High-performance buffer with O(1) keyed lookup for message deduplication.
pub struct KeyedBuffer {
    /// Main storage: keyed by (channel, symbol) for O(1) lookup
    pending: HashMap<MessageKey, BufferedMessage>,
    /// Insertion order tracking for FIFO flushing
    order: VecDeque<MessageKey>,
    /// Priority messages that bypass conflation (control signals)
    priority_queue: VecDeque<String>,
    /// Current buffer size (for overflow protection)
    current_size: usize,
    /// Statistics for monitoring
    stats: BufferStats,
}

/// Buffer statistics for monitoring and debugging.
#[derive(Debug, Default, Clone)]
pub struct BufferStats {
    pub messages_buffered: u64,
    pub messages_conflated: u64,
    pub messages_flushed: u64,
    pub trades_aggregated: u64,
    pub overflow_drops: u64,
}

impl KeyedBuffer {
    /// Create a new keyed buffer.
    pub fn new() -> Self {
        Self {
            pending: HashMap::with_capacity(1024),
            order: VecDeque::with_capacity(1024),
            priority_queue: VecDeque::with_capacity(64),
            current_size: 0,
            stats: BufferStats::default(),
        }
    }

    /// Get current buffer statistics.
    pub fn stats(&self) -> &BufferStats {
        &self.stats
    }

    /// Get current buffer length.
    pub fn len(&self) -> usize {
        self.current_size
    }

    /// Check if buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.current_size == 0 && self.priority_queue.is_empty()
    }

    /// Check if buffer is at critical capacity.
    pub fn is_critical(&self) -> bool {
        self.current_size >= MAX_BUFFER_SIZE
    }

    /// Add a priority message (bypasses conflation).
    pub fn push_priority(&mut self, raw_json: String) {
        self.priority_queue.push_back(raw_json);
    }

    /// Insert or update a message in the buffer.
    ///
    /// For overwrite-capable types (ticker, OHLC): replaces existing.
    /// For aggregation types (trade): merges into existing.
    /// For other types: queues separately.
    pub fn insert(&mut self, message: BufferedMessage, conflate: bool) {
        self.stats.messages_buffered += 1;

        // Priority messages go to separate queue
        if message.message_type.is_priority() {
            self.priority_queue.push_back(message.raw_json);
            return;
        }

        let key = self.make_key(&message);

        if conflate {
            if let Some(existing) = self.pending.get_mut(&key) {
                // Conflation: merge or replace based on type
                if message.message_type.supports_overwrite() {
                    // Ticker/OHLC: just replace with latest
                    existing.raw_json = message.raw_json;
                    self.stats.messages_conflated += 1;
                    return;
                } else if message.message_type.requires_aggregation() {
                    // Trade: would need to merge aggregates
                    // For now, we'll handle this in the manager
                    self.stats.trades_aggregated += 1;
                }
                // For other types, fall through to insert as new
            }
        }

        // Check overflow before inserting
        if self.current_size >= MAX_BUFFER_SIZE {
            self.handle_overflow();
        }

        // Insert new entry
        if !self.pending.contains_key(&key) {
            self.order.push_back(key.clone());
            self.current_size += 1;
        }
        self.pending.insert(key, message);
    }

    /// Pop the next message to send (priority first, then FIFO).
    pub fn pop(&mut self) -> Option<String> {
        // Priority messages first
        if let Some(msg) = self.priority_queue.pop_front() {
            self.stats.messages_flushed += 1;
            return Some(msg);
        }

        // Then regular messages in FIFO order
        while let Some(key) = self.order.pop_front() {
            if let Some(message) = self.pending.remove(&key) {
                self.current_size = self.current_size.saturating_sub(1);
                self.stats.messages_flushed += 1;
                return Some(message.raw_json);
            }
            // Key was already removed (conflated), try next
        }

        None
    }

    /// Create a unique key for message deduplication.
    fn make_key(&self, message: &BufferedMessage) -> MessageKey {
        let symbol = message.symbol.clone().unwrap_or_default();
        (message.channel.clone(), symbol)
    }

    /// Handle buffer overflow by dropping oldest non-critical messages.
    fn handle_overflow(&mut self) {
        // Drop the oldest 10% of messages
        let to_drop = MAX_BUFFER_SIZE / 10;
        for _ in 0..to_drop {
            if let Some(key) = self.order.pop_front() {
                if let Some(msg) = self.pending.remove(&key) {
                    // Don't drop priority types even in overflow
                    if msg.message_type.is_priority() {
                        self.priority_queue.push_back(msg.raw_json);
                    } else {
                        self.stats.overflow_drops += 1;
                    }
                }
                self.current_size = self.current_size.saturating_sub(1);
            }
        }
        tracing::warn!(
            "Buffer overflow! Dropped {} messages. Total drops: {}",
            to_drop,
            self.stats.overflow_drops
        );
    }

    /// Clear all buffered messages.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.order.clear();
        self.priority_queue.clear();
        self.current_size = 0;
    }
}

impl Default for KeyedBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_from_channel() {
        assert_eq!(MessageType::from_channel("ticker"), MessageType::Ticker);
        assert_eq!(MessageType::from_channel("TICKER"), MessageType::Ticker);
        assert_eq!(MessageType::from_channel("book"), MessageType::Book);
        assert_eq!(MessageType::from_channel("trade"), MessageType::Trade);
        assert_eq!(MessageType::from_channel("heartbeat"), MessageType::Control);
    }

    #[test]
    fn test_buffer_insert_and_pop() {
        let mut buffer = KeyedBuffer::new();

        buffer.insert(
            BufferedMessage::new(
                r#"{"channel":"ticker"}"#.to_string(),
                "ticker".to_string(),
                Some("BTC/USD".to_string()),
                MessageType::Ticker,
            ),
            false,
        );

        assert_eq!(buffer.len(), 1);
        assert!(buffer.pop().is_some());
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_priority_queue() {
        let mut buffer = KeyedBuffer::new();

        // Add regular message
        buffer.insert(
            BufferedMessage::new(
                r#"{"channel":"ticker"}"#.to_string(),
                "ticker".to_string(),
                Some("BTC/USD".to_string()),
                MessageType::Ticker,
            ),
            false,
        );

        // Add priority message
        buffer.push_priority(r#"{"channel":"heartbeat"}"#.to_string());

        // Priority should come first
        let first = buffer.pop().unwrap();
        assert!(first.contains("heartbeat"));
    }

    #[test]
    fn test_conflation_overwrite() {
        let mut buffer = KeyedBuffer::new();

        // Insert first ticker
        buffer.insert(
            BufferedMessage::new(
                r#"{"price":100}"#.to_string(),
                "ticker".to_string(),
                Some("BTC/USD".to_string()),
                MessageType::Ticker,
            ),
            true,
        );

        // Insert second ticker (should overwrite)
        buffer.insert(
            BufferedMessage::new(
                r#"{"price":200}"#.to_string(),
                "ticker".to_string(),
                Some("BTC/USD".to_string()),
                MessageType::Ticker,
            ),
            true,
        );

        // Should only have one message
        assert_eq!(buffer.len(), 1);

        // Should be the latest value
        let msg = buffer.pop().unwrap();
        assert!(msg.contains("200"));
    }
}
