use serde::{Deserialize, Serialize};

/// Message containing user's open orders
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenOrdersMessage {
    pub channel: String,
    pub r#type: String,
    pub data: Vec<OpenOrder>,
    #[serde(default)]
    pub snapshot: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenOrder {
    pub symbol: String,
    pub side: String,       // "buy" or "sell"
    pub order_type: String, // "limit", "market", etc.
    pub limit_price: Option<f64>,
    pub order_qty: f64,
    pub filled_qty: f64,
    pub order_id: String,
    pub timestamp: String,
    pub status: String, // "open", "pending", etc.
}
