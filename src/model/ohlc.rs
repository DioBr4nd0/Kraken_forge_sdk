use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OHLCMessage {
    pub channel: String,
    pub r#type: String, // "update" or "snapshot"
    pub data: Vec<OHLCData>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OHLCData {
    pub symbol: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub trades: u64,
    pub interval: u64,          // e.g., 1 (minute)
    pub interval_begin: String, // Timestamp
}
