pub mod auth;
pub mod book;
pub mod client;
pub mod conflation;
mod error;
pub mod model;
pub mod network;

pub use client::KrakenClient;
pub use conflation::{ConflationManager, ConflationState};
pub use error::KrakenError;
pub use model::conflated::{ConflatedMessage, TradeAggregateMessage, TradeBatchMessage};
