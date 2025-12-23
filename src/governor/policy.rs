//! Operational mode policy and state machine.

/// Operational mode determining SDK behavior.
///
/// # Performance Characteristics
///
/// - **Performance** (Green): Zero overhead. Full fidelity data.
/// - **Balanced** (Yellow): Smart batching enabled. Minor latency increase.
/// - **Survival** (Red): Aggressive load shedding. Non-critical messages dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OperationalMode {
    /// Full performance mode. CPU < 60%, RAM healthy.
    /// All messages processed, zero latency.
    Performance = 0,

    /// Balanced mode. CPU 60-90% or RAM getting tight.
    /// Smart batching enabled, non-critical logs reduced.
    Balanced = 1,

    /// Survival mode. CPU > 90% or RAM critical.
    /// Aggressive load shedding: heartbeats dropped, L2 depth reduced.
    Survival = 2,
}

impl OperationalMode {
    /// Convert to u8 for atomic storage.
    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Convert from u8 (from atomic load).
    #[inline]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Performance,
            1 => Self::Balanced,
            2 => Self::Survival,
            _ => Self::Performance, // Fallback to safe mode
        }
    }

    /// Check if this mode requires message filtering.
    #[inline]
    pub const fn requires_filtering(&self) -> bool {
        matches!(self, Self::Survival)
    }

    /// Check if this mode should enable batching.
    #[inline]
    pub const fn requires_batching(&self) -> bool {
        matches!(self, Self::Balanced | Self::Survival)
    }

    /// Get recommended order book depth for this mode.
    #[inline]
    pub const fn book_depth(&self, normal_depth: usize, survival_depth: usize) -> usize {
        match self {
            Self::Performance | Self::Balanced => normal_depth,
            Self::Survival => survival_depth,
        }
    }

    /// Get human-readable name.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Performance => "Performance",
            Self::Balanced => "Balanced",
            Self::Survival => "Survival",
        }
    }
}

impl std::fmt::Display for OperationalMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Default for OperationalMode {
    fn default() -> Self {
        Self::Performance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u8_conversion() {
        assert_eq!(OperationalMode::Performance.as_u8(), 0);
        assert_eq!(OperationalMode::Balanced.as_u8(), 1);
        assert_eq!(OperationalMode::Survival.as_u8(), 2);

        assert_eq!(OperationalMode::from_u8(0), OperationalMode::Performance);
        assert_eq!(OperationalMode::from_u8(1), OperationalMode::Balanced);
        assert_eq!(OperationalMode::from_u8(2), OperationalMode::Survival);
    }

    #[test]
    fn test_invalid_u8_fallback() {
        // Invalid values should fall back to Performance for safety
        assert_eq!(OperationalMode::from_u8(255), OperationalMode::Performance);
    }

    #[test]
    fn test_filtering_requirements() {
        assert!(!OperationalMode::Performance.requires_filtering());
        assert!(!OperationalMode::Balanced.requires_filtering());
        assert!(OperationalMode::Survival.requires_filtering());
    }

    #[test]
    fn test_batching_requirements() {
        assert!(!OperationalMode::Performance.requires_batching());
        assert!(OperationalMode::Balanced.requires_batching());
        assert!(OperationalMode::Survival.requires_batching());
    }

    #[test]
    fn test_book_depth() {
        assert_eq!(OperationalMode::Performance.book_depth(10, 5), 10);
        assert_eq!(OperationalMode::Balanced.book_depth(10, 5), 10);
        assert_eq!(OperationalMode::Survival.book_depth(10, 5), 5);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", OperationalMode::Performance), "Performance");
        assert_eq!(format!("{}", OperationalMode::Survival), "Survival");
    }
}
