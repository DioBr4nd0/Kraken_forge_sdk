use std::collections::BTreeMap;
use ordered_float::OrderedFloat;
use crate::model::message::BookLevel;

/// Default order book depth
const DEFAULT_MAX_DEPTH: usize = 10;

/// Local order book with configurable depth.
///
/// The Governor can reduce depth during Survival mode to save RAM.
#[derive(Debug, Clone)]
pub struct LocalBook {
    // Key: f64 (For Sorting)
    // Value: (PriceString, QtyString) (For Checksums)
    pub bids: BTreeMap<OrderedFloat<f64>, (String, String)>, 
    pub asks: BTreeMap<OrderedFloat<f64>, (String, String)>,
    /// Maximum depth to maintain (can be reduced by Governor)
    max_depth: usize,
}

impl Default for LocalBook {
    fn default() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

impl LocalBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a LocalBook with custom max depth.
    pub fn with_depth(max_depth: usize) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            max_depth,
        }
    }

    /// Get current max depth.
    #[inline]
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    pub fn apply_snapshot(&mut self, bids: Vec<BookLevel>, asks: Vec<BookLevel>) {
        self.bids.clear();
        self.asks.clear();
        self.apply_updates(bids, true);
        self.apply_updates(asks, false);
    }

    pub fn apply_updates(&mut self, updates: Vec<BookLevel>, is_bid: bool) {
        let book = if is_bid { &mut self.bids } else { &mut self.asks };

        for level in updates {
            // Parse to f64 just for the sorting key
            let price_f64: f64 = level.price.parse().unwrap_or_default();
            let qty_f64: f64 = level.qty.parse().unwrap_or_default();
            
            let key = OrderedFloat(price_f64);
            
            if qty_f64 == 0.0 {
                book.remove(&key);
            } else {
                // Store the original Strings!
                book.insert(key, (level.price, level.qty));
            }
        }
        
        // Truncate to max_depth after updates
        let max_depth = self.max_depth;
        if book.len() > max_depth {
            if is_bid {
                // For bids: remove the lowest prices (worst bids)
                let keys_to_remove: Vec<_> = book.keys().take(book.len() - max_depth).cloned().collect();
                for key in keys_to_remove {
                    book.remove(&key);
                }
            } else {
                // For asks: remove the highest prices (worst asks)
                let keys_to_remove: Vec<_> = book.keys().rev().take(book.len() - max_depth).cloned().collect();
                for key in keys_to_remove {
                    book.remove(&key);
                }
            }
        }
    }

    /// Set max depth (used by Governor during Survival mode).
    ///
    /// Immediately truncates book if current depth exceeds new limit.
    pub fn prune_to_depth(&mut self, depth: usize) {
        self.max_depth = depth;
        // Truncate bids
        if self.bids.len() > depth {
            let keys_to_remove: Vec<_> = self.bids.keys().take(self.bids.len() - depth).cloned().collect();
            for key in keys_to_remove {
                self.bids.remove(&key);
            }
        }
        // Truncate asks
        if self.asks.len() > depth {
            let keys_to_remove: Vec<_> = self.asks.keys().rev().take(self.asks.len() - depth).cloned().collect();
            for key in keys_to_remove {
                self.asks.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_depth() {
        let book = LocalBook::new();
        assert_eq!(book.max_depth(), DEFAULT_MAX_DEPTH);
    }

    #[test]
    fn test_custom_depth() {
        let book = LocalBook::with_depth(5);
        assert_eq!(book.max_depth(), 5);
    }
}