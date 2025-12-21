use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256, Sha512};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha512 = Hmac<Sha512>;

/// Fetches a WebSocket authentication token from Kraken's REST API
///
/// # Arguments
/// * `api_key` - Your Kraken API key
/// * `api_secret` - Your Kraken API secret (base64-encoded)
///
/// # Returns
/// A token valid for 15 minutes for authenticated WebSocket connections
pub async fn get_ws_token(
    api_key: &str,
    api_secret: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // 1. Generate nonce (timestamp in milliseconds)
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .to_string();

    let url_path = "/0/private/GetWebSocketsToken";

    // 2. Prepare POST payload
    let params = format!("nonce={}", nonce);

    // 3. Generate SHA256(nonce + POST data)
    let mut sha256 = Sha256::new();
    sha256.update(nonce.as_bytes());
    sha256.update(params.as_bytes());
    let sha256_hash = sha256.finalize();

    // 4. Decode API secret from base64
    let secret_bytes = general_purpose::STANDARD.decode(api_secret)?;

    // 5. Generate HMAC-SHA512(Path + SHA256)
    let mut mac = HmacSha512::new_from_slice(&secret_bytes)?;
    mac.update(url_path.as_bytes());
    mac.update(&sha256_hash);
    let signature = general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    // 6. Send authenticated REST request
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert("API-Key", HeaderValue::from_str(api_key)?);
    headers.insert("API-Sign", HeaderValue::from_str(&signature)?);

    let resp = client
        .post("https://api.kraken.com/0/private/GetWebSocketsToken")
        .headers(headers)
        .body(params)
        .send()
        .await?;

    // 7. Parse response
    let json: serde_json::Value = resp.json().await?;

    // 8. Extract token from response
    if let Some(token) = json["result"]["token"].as_str() {
        Ok(token.to_string())
    } else {
        Err(format!("Auth Failed: {:?}", json).into())
    }
}
