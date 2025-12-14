use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TickerData {
    pub symbol: String,
    pub bid: f64,
    pub bid_qty: f64, 
    pub ask: f64, 
    pub last: f64, 
    pub volume: f64, 
    pub vwap: f64,
    pub low: f64, 
    pub high: f64, 
    pub change: f64,
    pub change_pct: f64,
    pub volume_usd: f64,
}

//Kraken sends ticker updates wrapped in "data" array.
// we need this wrapper helper
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TickerMessage {
    pub channel: String, 
    pub r#type: String,
    pub data: Vec<TickerData>,
}