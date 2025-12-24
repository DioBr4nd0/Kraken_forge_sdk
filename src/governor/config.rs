//! Governor configuration with tunable thresholds.

/// Configuration for the Resource-Aware Governor.
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// CPU threshold to enter Balanced mode (default: 60%)
    pub balanced_cpu_threshold: f64,
    /// CPU threshold to enter Survival mode (default: 90%)
    pub survival_cpu_threshold: f64,
    /// RAM threshold (MB free) to enter Balanced mode (default: 2048 MB)
    pub balanced_ram_threshold_mb: u64,
    /// RAM threshold (MB free) to enter Survival mode (default: 512 MB)
    pub survival_ram_threshold_mb: u64,
    /// Sampling interval in milliseconds (default: 500ms)
    pub sample_interval_ms: u64,
    /// Number of consecutive checks before mode transition (default: 2)
    pub hysteresis_threshold: u8,
    /// Order book depth in Balanced mode (default: 100)
    pub balanced_book_depth: usize,
    /// Order book depth in Survival mode (default: 10)
    pub survival_book_depth: usize,
    /// Enable message filtering in non-Performance modes (default: true)
    pub enable_message_filtering: bool,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            balanced_cpu_threshold: 60.0,
            survival_cpu_threshold: 90.0,
            balanced_ram_threshold_mb: 2048,
            survival_ram_threshold_mb: 512,
            sample_interval_ms: 500,
            hysteresis_threshold: 2,
            balanced_book_depth: 100,
            survival_book_depth: 10,
            enable_message_filtering: true,
        }
    }
}

impl GovernorConfig {
    /// Create a new config builder.
    pub fn builder() -> GovernorConfigBuilder {
        GovernorConfigBuilder::default()
    }

    /// Create a stricter config that limits CPU more aggressively.
    /// Use this if you want to reserve more CPU for your trading algorithm.
    pub fn low_footprint() -> Self {
        Self {
            balanced_cpu_threshold: 30.0,
            survival_cpu_threshold: 50.0,
            balanced_ram_threshold_mb: 4096,
            survival_ram_threshold_mb: 1024,
            ..Default::default()
        }
    }

    /// Create a permissive config that allows higher resource usage.
    /// Use this on dedicated trading servers with no other workloads.
    pub fn high_performance() -> Self {
        Self {
            balanced_cpu_threshold: 80.0,
            survival_cpu_threshold: 95.0,
            balanced_ram_threshold_mb: 1024,
            survival_ram_threshold_mb: 256,
            ..Default::default()
        }
    }
}

/// Builder pattern for GovernorConfig.
#[derive(Default)]
pub struct GovernorConfigBuilder {
    config: GovernorConfig,
}

impl GovernorConfigBuilder {
    /// Set the CPU threshold for Balanced mode.
    pub fn balanced_cpu(mut self, threshold: f64) -> Self {
        self.config.balanced_cpu_threshold = threshold;
        self
    }

    /// Set the CPU threshold for Survival mode.
    pub fn survival_cpu(mut self, threshold: f64) -> Self {
        self.config.survival_cpu_threshold = threshold;
        self
    }

    /// Set the RAM threshold (MB free) for Balanced mode.
    pub fn balanced_ram_mb(mut self, threshold: u64) -> Self {
        self.config.balanced_ram_threshold_mb = threshold;
        self
    }

    /// Set the RAM threshold (MB free) for Survival mode.
    pub fn survival_ram_mb(mut self, threshold: u64) -> Self {
        self.config.survival_ram_threshold_mb = threshold;
        self
    }

    /// Set the sampling interval in milliseconds.
    pub fn sample_interval_ms(mut self, interval: u64) -> Self {
        self.config.sample_interval_ms = interval;
        self
    }

    /// Set the hysteresis threshold (consecutive checks before transition).
    pub fn hysteresis(mut self, threshold: u8) -> Self {
        self.config.hysteresis_threshold = threshold;
        self
    }

    /// Set the order book depth in Survival mode.
    pub fn survival_book_depth(mut self, depth: usize) -> Self {
        self.config.survival_book_depth = depth;
        self
    }

    /// Set the order book depth in Balanced mode.
    pub fn balanced_book_depth(mut self, depth: usize) -> Self {
        self.config.balanced_book_depth = depth;
        self
    }

    /// Enable or disable message filtering in non-Performance modes.
    pub fn enable_message_filtering(mut self, enable: bool) -> Self {
        self.config.enable_message_filtering = enable;
        self
    }

    /// Build the configuration.
    pub fn build(self) -> GovernorConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GovernorConfig::default();
        assert_eq!(config.balanced_cpu_threshold, 60.0);
        assert_eq!(config.survival_cpu_threshold, 90.0);
    }

    #[test]
    fn test_builder() {
        let config = GovernorConfig::builder()
            .balanced_cpu(40.0)
            .survival_cpu(70.0)
            .build();

        assert_eq!(config.balanced_cpu_threshold, 40.0);
        assert_eq!(config.survival_cpu_threshold, 70.0);
    }

    #[test]
    fn test_presets() {
        let low = GovernorConfig::low_footprint();
        let high = GovernorConfig::high_performance();

        assert!(low.balanced_cpu_threshold < high.balanced_cpu_threshold);
        assert!(low.survival_cpu_threshold < high.survival_cpu_threshold);
    }
}
