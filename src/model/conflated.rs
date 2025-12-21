//! Conflated message types for transparent handling of batched/aggregated data.
//!
//! These types allow consumers to handle both normal single messages and
//! conflated batches/aggregates without code changes.

use serde::{Deserialize, Serialize};

use super::trade::TradeData;

/// A message that may be conflated (batched or aggregated).
///
/// This enum is used by consumers who want to explicitly handle conflated data.
/// The `conflated` field indicates whether the message was produced by the
/// conflation engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConflatedMessage {
    /// Batched trades (multiple trades grouped together)
    TradeBatch(TradeBatchMessage),
    /// Aggregated trade summary (volume/price stats only)
    TradeAggregate(TradeAggregateMessage),
    /// Normal single message (passthrough)
    Single(serde_json::Value),
}

/// A batch of trades grouped together during Yellow state.
///
/// Contains all original trade data, just delivered as a single message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeBatchMessage {
    pub channel: String,
    pub r#type: String,
    pub data: Vec<TradeData>,
    /// Always true for batched messages
    #[serde(default)]
    pub conflated: bool,
    /// Optional statistics about the batch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<BatchStats>,
}

/// Statistics about a trade batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchStats {
    pub trade_count: u64,
    pub total_volume: f64,
    pub min_price: f64,
    pub max_price: f64,
}

/// An aggregated trade summary from Red state.
///
/// This is a lossy compression - individual trades are lost but critical
/// information (total volume, price range) is preserved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAggregateMessage {
    pub channel: String,
    pub r#type: String,
    pub data: Vec<TradeAggregateData>,
    /// Always true for aggregated messages
    #[serde(default)]
    pub conflated: bool,
}

/// Aggregated trade data for a single symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAggregateData {
    pub symbol: String,
    pub total_volume: f64,
    pub min_price: f64,
    pub max_price: f64,
    pub last_price: f64,
    pub trade_count: u64,
    pub timestamp: String,
}

impl ConflatedMessage {
    /// Check if this message was conflated.
    pub fn is_conflated(&self) -> bool {
        match self {
            Self::TradeBatch(b) => b.conflated,
            Self::TradeAggregate(a) => a.conflated,
            Self::Single(_) => false,
        }
    }

    /// Get the channel name if present.
    pub fn channel(&self) -> Option<&str> {
        match self {
            Self::TradeBatch(b) => Some(&b.channel),
            Self::TradeAggregate(a) => Some(&a.channel),
            Self::Single(v) => v.get("channel").and_then(|c| c.as_str()),
        }
    }

    /// Parse a raw JSON string into a ConflatedMessage.
    pub fn parse(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }
}

impl TradeBatchMessage {
    /// Get total volume across all trades in the batch.
    pub fn total_volume(&self) -> f64 {
        self.data.iter().map(|t| t.qty).sum()
    }

    /// Get the number of trades in the batch.
    pub fn trade_count(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_message() {
        let raw = r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD"}]}"#;
        let msg = ConflatedMessage::parse(raw).unwrap();

        assert!(!msg.is_conflated());
        assert_eq!(msg.channel(), Some("ticker"));
    }

    #[test]
    fn test_parse_trade_batch() {
        let raw = r#"{
            "channel": "trade",
            "type": "batch",
            "data": [{"symbol":"BTC/USD","side":"buy","price":50000,"qty":1.5,"ord_type":"limit","trade_id":123,"timestamp":"2024-01-01"}],
            "conflated": true
        }"#;

        let msg = ConflatedMessage::parse(raw).unwrap();
        assert!(msg.is_conflated());

        if let ConflatedMessage::TradeBatch(batch) = msg {
            assert_eq!(batch.trade_count(), 1);
            assert_eq!(batch.total_volume(), 1.5);
        } else {
            panic!("Expected TradeBatch");
        }
    }

    #[test]
    fn test_parse_trade_aggregate() {
        let raw = r#"{
            "channel": "trade",
            "type": "aggregate",
            "data": [{"symbol":"BTC/USD","total_volume":100.5,"min_price":49000,"max_price":51000,"last_price":50500,"trade_count":42,"timestamp":"2024-01-01"}],
            "conflated": true
        }"#;

        let msg = ConflatedMessage::parse(raw).unwrap();
        assert!(msg.is_conflated());

        if let ConflatedMessage::TradeAggregate(agg) = msg {
            assert_eq!(agg.data[0].trade_count, 42);
            assert_eq!(agg.data[0].total_volume, 100.5);
        } else {
            panic!("Expected TradeAggregate");
        }
    }
}
