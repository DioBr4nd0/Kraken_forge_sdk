//! Conflation state machine based on channel pressure.

/// Channel pressure state determining conflation behavior.
///
/// The state is computed from the ratio of used capacity to max capacity
/// of the downstream event channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflationState {
    /// Channel < 50% full: Passthrough mode, zero latency.
    /// All messages are forwarded immediately without buffering.
    Green,

    /// Channel 50-90% full: Smart batching mode.
    /// Messages are grouped by type before sending.
    Yellow,

    /// Channel > 90% full: Aggressive conflation mode.
    /// Tickers are overwritten, trades are aggregated.
    Red,
}

impl ConflationState {
    /// Determine state from channel capacity metrics.
    ///
    /// # Arguments
    /// * `current_len` - Current number of items in the channel
    /// * `max_capacity` - Maximum channel capacity
    ///
    /// # Returns
    /// The appropriate conflation state based on fill ratio.
    #[inline]
    pub fn from_capacity(current_len: usize, max_capacity: usize) -> Self {
        if max_capacity == 0 {
            return Self::Red; // Safety: treat zero capacity as critical
        }

        let ratio = current_len as f64 / max_capacity as f64;

        if ratio < 0.5 {
            Self::Green
        } else if ratio < 0.9 {
            Self::Yellow
        } else {
            Self::Red
        }
    }

    /// Check if this state requires buffering.
    #[inline]
    pub fn requires_buffering(&self) -> bool {
        matches!(self, Self::Yellow | Self::Red)
    }

    /// Check if this state requires aggressive conflation.
    #[inline]
    pub fn requires_conflation(&self) -> bool {
        matches!(self, Self::Red)
    }

    /// Get a human-readable state name for logging/metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Red => "red",
        }
    }
}

impl std::fmt::Display for ConflationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_thresholds() {
        // Green: < 50%
        assert_eq!(
            ConflationState::from_capacity(0, 100),
            ConflationState::Green
        );
        assert_eq!(
            ConflationState::from_capacity(49, 100),
            ConflationState::Green
        );

        // Yellow: 50% - 90%
        assert_eq!(
            ConflationState::from_capacity(50, 100),
            ConflationState::Yellow
        );
        assert_eq!(
            ConflationState::from_capacity(89, 100),
            ConflationState::Yellow
        );

        // Red: >= 90%
        assert_eq!(
            ConflationState::from_capacity(90, 100),
            ConflationState::Red
        );
        assert_eq!(
            ConflationState::from_capacity(100, 100),
            ConflationState::Red
        );
    }

    #[test]
    fn test_zero_capacity_is_red() {
        assert_eq!(ConflationState::from_capacity(0, 0), ConflationState::Red);
    }

    #[test]
    fn test_requires_buffering() {
        assert!(!ConflationState::Green.requires_buffering());
        assert!(ConflationState::Yellow.requires_buffering());
        assert!(ConflationState::Red.requires_buffering());
    }
}
