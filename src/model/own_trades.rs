use serde::{Deserialize, Serialize};

/// Message containing user's own executed trades
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OwnTradesMessage {
    pub channel: String,
    pub r#type: String,
    pub data: Vec<OwnTrade>,
    #[serde(default)]
    pub snapshot: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OwnTrade {
    pub symbol: String,
    pub side: String,       // "buy" or "sell"
    pub order_type: String, // "limit", "market", etc.
    pub price: f64,
    pub qty: f64,
    pub cost: f64,
    pub fee: f64,
    pub trade_id: String,
    pub order_id: String,
    pub timestamp: String,
    pub maker: bool,
}
