use serde::{Deserialize, Serialize};
use crate::model::trade::TradeMessage;

use super::ticker::TickerMessage;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum KrakenMessage {
    // 1. Ticker
    Ticker(TickerMessage),

    // 2. Book (We fixed the types here!)
    Book(BookMessage),

    Trade(TradeMessage),

    // 3. Status
    Status {
        channel: String,
        data: Vec<SystemStatus>,
    },

    // 4. Subscription
    SubscriptionStatus {
        method: String,
        success: bool,
        // capture optional result so we can debug
        #[serde(default)] 
        result: serde_json::Value, 
    },

    // 5. Heartbeat (Strict Mode: requires type to be "heartbeat" OR channel "heartbeat" with no other data)
    // We move this to the bottom or make it specific. 
    // Actually, simplest fix is to just let it match but verify channel in logic.
    Heartbeat {
        channel: String,
    },

    // 6. Unknown
    Unknown(serde_json::Value),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BookMessage {
    pub channel: String,
    pub r#type: String,
    pub data: Vec<BookData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookData {
    pub symbol: String,
    pub checksum: u32,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}

// Book level with exact price/qty strings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookLevel {
    pub price: String, 
    pub qty: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemStatus {
    pub system: String,
    pub version: String,
    pub connection_id: u64,
}