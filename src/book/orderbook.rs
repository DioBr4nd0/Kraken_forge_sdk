// use std::collections::BTreeMap;
// use ordered_float::OrderedFloat;

// #[derive(Debug, Clone, Default)]
// pub struct LocalBook {
//     // We use BTreeMap so it is ALWAYS sorted by price.
//     // OrderedFloat allows using f64 as a key.
//     // High Bids = Good (Reverse Sort typically needed for "Best Bid", 
//     // but standard BTreeMap sorts Low -> High. We'll handle that in accessors).
//     // // Value: (PriceString, VolumeString) - kept for Checksum accuracy
//     pub bids: BTreeMap<OrderedFloat<f64>, (String, String)>,
//     pub asks: BTreeMap<OrderedFloat<f64>, (String, String)>,
// }

// impl LocalBook {
//     pub fn new() -> Self {
//         Self::default()
//     } 

//     /// Process a snapshot (clear & replace)
//     pub fn apply_snapshot(&mut self, bids: Vec<(String, String)>, asks: Vec<(String, String)>) {
//         self.bids.clear();
//         self.asks.clear();
//         self.apply_updates(bids, true);
//         self.apply_updates(asks, false);
// }

//     ///Process updates
//     /// Kraken sends data as strings: "prices", "volume"
//     pub fn apply_updates(&mut self, updates: Vec<(String, String)>, is_bid: bool) {
//         let book = if is_bid {&mut self.bids} else {&mut self.asks};

//         for (price_str, vol_str) in updates {
//             let price: f64 = price_str.parse().unwrap_or_default();
//             let volume : f64 = vol_str.parse().unwrap_or_default();

//             let key = OrderedFloat(price);

//             if volume == 0.0 {
//                 book.remove(&key);
//             } else {
//                 book.insert(key, (price_str, vol_str));
//             }
//         }
//     }
//     // pub fn best_bid(&self) -> Option<(f64, f64)> {
//     //     self.bids.iter().next_back().map(|(p,v)| (p.into_inner(), *v))
//     // }

//     // pub fn best_ask(&self) -> Option<(f64, f64)> {
//     //     self.asks.iter().next().map(|(p,v)| (p.into_inner(), *v))
//     // }
// }

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