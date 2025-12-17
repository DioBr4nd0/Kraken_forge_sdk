use std::collections::BTreeMap;
use ordered_float::OrderedFloat;
use crate::model::message::BookLevel;

#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    // Key: f64 (For Sorting)
    // Value: (PriceString, QtyString) (For Checksums)
    pub bids: BTreeMap<OrderedFloat<f64>, (String, String)>, 
    pub asks: BTreeMap<OrderedFloat<f64>, (String, String)>,
}

impl LocalBook {
    pub fn new() -> Self {
        Self::default()
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
        
        // Truncate to depth 10 after updates
        // For asks: keep lowest 10 (best asks), remove higher prices
        // For bids: keep highest 10 (best bids), remove lower prices
        if book.len() > 10 {
            if is_bid {
                // For bids: remove the lowest prices (worst bids)
                let keys_to_remove: Vec<_> = book.keys().take(book.len() - 10).cloned().collect();
                for key in keys_to_remove {
                    book.remove(&key);
                }
            } else {
                // For asks: remove the highest prices (worst asks)
                let keys_to_remove: Vec<_> = book.keys().rev().take(book.len() - 10).cloned().collect();
                for key in keys_to_remove {
                    book.remove(&key);
                }
            }
        }
    }
}