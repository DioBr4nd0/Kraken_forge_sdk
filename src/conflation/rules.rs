//! Conflation rules for different message types.
//!
//! These rules respect data semantics:
//! - Tickers: Latest wins (overwrite)
//! - Order Book: Delta handling (sum volumes at same price)
//! - Trades: Aggregate (preserve total volume, price range)

use serde_json::Value;

use super::buffer::{MessageType, TradeAggregate};

/// Parse a raw JSON message and extract metadata for conflation.
#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub channel: String,
    pub symbol: Option<String>,
    pub message_type: MessageType,
    pub is_subscription: bool,
    pub raw: String,
}

impl ParsedMessage {
    /// Parse a raw JSON string into a structured message.
    pub fn parse(raw: &str) -> Self {
        let value: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => {
                return Self {
                    channel: String::new(),
                    symbol: None,
                    message_type: MessageType::Unknown,
                    is_subscription: false,
                    raw: raw.to_string(),
                };
            }
        };

        // Check for subscription response
        if value.get("method").is_some() || value.get("success").is_some() {
            return Self {
                channel: String::new(),
                symbol: None,
                message_type: MessageType::Subscription,
                is_subscription: true,
                raw: raw.to_string(),
            };
        }

        // Extract channel
        let channel = value
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Determine message type
        let message_type = MessageType::from_channel(&channel);

        // Extract symbol from data array if present
        let symbol = value
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("symbol"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        Self {
            channel,
            symbol,
            message_type,
            is_subscription: false,
            raw: raw.to_string(),
        }
    }

    /// Check if this message should bypass conflation entirely.
    pub fn is_priority(&self) -> bool {
        self.message_type.is_priority() || self.is_subscription
    }
}

/// Extract trade data for aggregation.
#[derive(Debug, Clone)]
pub struct TradeData {
    pub symbol: String,
    pub price: f64,
    pub volume: f64,
    pub side: String,
    pub timestamp: String,
    pub raw: Value,
}

impl TradeData {
    /// Extract trade data from a parsed trade message.
    pub fn from_json(value: &Value) -> Option<Vec<Self>> {
        let data = value.get("data")?.as_array()?;

        let trades: Vec<Self> = data
            .iter()
            .filter_map(|item| {
                let symbol = item.get("symbol")?.as_str()?.to_string();
                let price = item.get("price")?.as_f64()?;
                let volume = item.get("qty")?.as_f64()?;
                let side = item.get("side")?.as_str()?.to_string();
                let timestamp = item.get("timestamp")?.as_str()?.to_string();

                Some(Self {
                    symbol,
                    price,
                    volume,
                    side,
                    timestamp,
                    raw: item.clone(),
                })
            })
            .collect();

        if trades.is_empty() {
            None
        } else {
            Some(trades)
        }
    }
}

/// Create a trade aggregate from a single trade message.
pub fn create_trade_aggregate(raw: &str) -> Option<TradeAggregate> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let trades = TradeData::from_json(&value)?;

    if trades.is_empty() {
        return None;
    }

    let first = &trades[0];
    let mut aggregate = TradeAggregate::new(
        first.symbol.clone(),
        first.price,
        first.volume,
        first.side.clone(),
        first.timestamp.clone(),
        first.raw.clone(),
    );

    for trade in trades.iter().skip(1) {
        aggregate.merge(
            trade.price,
            trade.volume,
            trade.side.clone(),
            trade.timestamp.clone(),
            trade.raw.clone(),
        );
    }

    Some(aggregate)
}

/// Merge two trade aggregates.
pub fn merge_trade_aggregates(existing: &mut TradeAggregate, new_raw: &str) -> bool {
    let value: Value = match serde_json::from_str(new_raw) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let trades = match TradeData::from_json(&value) {
        Some(t) => t,
        None => return false,
    };

    for trade in trades {
        existing.merge(
            trade.price,
            trade.volume,
            trade.side,
            trade.timestamp,
            trade.raw,
        );
    }

    true
}

/// Build a batched trade message from an aggregate (Yellow state).
pub fn build_trade_batch_json(aggregate: &TradeAggregate) -> String {
    let batch = serde_json::json!({
        "channel": "trade",
        "type": "batch",
        "data": aggregate.trades,
        "conflated": true,
        "stats": {
            "trade_count": aggregate.trade_count,
            "total_volume": aggregate.total_volume,
            "min_price": aggregate.min_price,
            "max_price": aggregate.max_price,
        }
    });
    batch.to_string()
}

/// Build an aggregated trade summary message (Red state).
pub fn build_trade_aggregate_json(aggregate: &TradeAggregate) -> String {
    let summary = serde_json::json!({
        "channel": "trade",
        "type": "aggregate",
        "data": [{
            "symbol": aggregate.symbol,
            "total_volume": aggregate.total_volume,
            "min_price": aggregate.min_price,
            "max_price": aggregate.max_price,
            "last_price": aggregate.last_price,
            "trade_count": aggregate.trade_count,
            "timestamp": aggregate.last_timestamp,
        }],
        "conflated": true
    });
    summary.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ticker_message() {
        let raw =
            r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD","bid":50000}]}"#;
        let parsed = ParsedMessage::parse(raw);

        assert_eq!(parsed.channel, "ticker");
        assert_eq!(parsed.symbol, Some("BTC/USD".to_string()));
        assert_eq!(parsed.message_type, MessageType::Ticker);
        assert!(!parsed.is_priority());
    }

    #[test]
    fn test_parse_heartbeat() {
        let raw = r#"{"channel":"heartbeat"}"#;
        let parsed = ParsedMessage::parse(raw);

        assert_eq!(parsed.channel, "heartbeat");
        assert_eq!(parsed.message_type, MessageType::Control);
        assert!(parsed.is_priority());
    }

    #[test]
    fn test_parse_subscription() {
        let raw = r#"{"method":"subscribe","success":true}"#;
        let parsed = ParsedMessage::parse(raw);

        assert!(parsed.is_subscription);
        assert!(parsed.is_priority());
    }

    #[test]
    fn test_trade_aggregate() {
        let raw = r#"{"channel":"trade","type":"update","data":[
            {"symbol":"BTC/USD","price":50000,"qty":1.5,"side":"buy","timestamp":"2024-01-01T00:00:00Z"}
        ]}"#;

        let agg = create_trade_aggregate(raw).unwrap();
        assert_eq!(agg.symbol, "BTC/USD");
        assert_eq!(agg.total_volume, 1.5);
        assert_eq!(agg.trade_count, 1);
    }
}
