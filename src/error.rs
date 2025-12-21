use thiserror::Error;

#[derive(Error, Debug)]
pub enum KrakenError {
    #[error("Network connection failed: {0}")]
    ConnectionError(String),

    #[error("WebSocket error: {0}")]
    SocketError(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("Failed to parse JSON: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("Internal channel closed")]
    ChannelClosed,

    #[error("Invalid URL")]
    UrlParseError(#[from] url::ParseError),

    #[error("Authentication failed: {0}")]
    AuthError(String),
}
