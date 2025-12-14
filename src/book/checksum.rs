use crc32fast::Hasher;
use super::orderbook::LocalBook;

pub fn validate_checksum(book: &LocalBook, remote_checksum: u32) -> bool {
    let mut hasher = Hasher::new();
    
    // 1. Process Asks (Lowest 10, ascending order)
    for (_, (price_str, qty_str)) in book.asks.iter().take(10) {
        let p = format_value(price_str);
        let q = format_value(qty_str);
        hasher.update(p.as_bytes());
        hasher.update(q.as_bytes());
    }

    // 2. Process Bids (Highest 10, descending order)
    for (_, (price_str, qty_str)) in book.bids.iter().rev().take(10) {
        let p = format_value(price_str);
        let q = format_value(qty_str);
        hasher.update(p.as_bytes());
        hasher.update(q.as_bytes());
    }

    let local = hasher.finalize();
    local == remote_checksum
}

fn format_value(val: &str) -> String {
    let without_dot = val.replace('.', "");
    let trimmed = without_dot.trim_start_matches('0');
    // Edge case: if all zeros (like "0.0" -> "00" -> ""), return "0"
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}