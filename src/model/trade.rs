use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradeMessage {
    pub channel: String, 
    pub r#type: String, 
    pub data: Vec<TradeData>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradeData {
    pub symbol: String, 
    pub side: String,
    pub price: f64, 
    pub qty: f64,
    pub ord_type: String,
    pub trade_id: u64,
    pub timestamp: String,
}