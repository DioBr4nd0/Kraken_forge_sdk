//! Adaptive Conflation Engine
//!
//! This module provides smart backpressure handling for high-traffic WebSocket streams.
//! It automatically compresses data during spikes (like market crashes) without losing
//! critical information like trade volume.
//!
//! # Architecture
//!
//! The engine operates in three states based on downstream channel pressure:
//! - **Green** (<50% full): Passthrough mode, zero latency
//! - **Yellow** (50-90%): Smart batching mode
//! - **Red** (>90%): Aggressive conflation mode
//!
//! # Example
//!
//! The conflation is handled automatically by the `ConnectionManager`. Advanced users
//! can access conflation types directly if needed.

pub mod buffer;
pub mod manager;
pub mod rules;
pub mod state;

pub use buffer::{BufferedMessage, KeyedBuffer, MessageKey, MessageType, TradeAggregate};
pub use manager::ConflationManager;
pub use state::ConflationState;
