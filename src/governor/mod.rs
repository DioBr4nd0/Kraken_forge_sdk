//! Resource-Aware Governor - monitors CPU/RAM and throttles SDK activity.

mod config;
mod metrics;
mod policy;

pub use config::GovernorConfig;
pub use metrics::HealthMetrics;
pub use policy::OperationalMode;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};
use tracing::{info, warn};

/// The Governor monitors system resources and sets operational mode.
pub struct Governor {
    /// Atomic mode for lock-free access (0=Performance, 1=Balanced, 2=Survival)
    mode: AtomicU8,
    /// Current health metrics (protected by RwLock for occasional reads)
    metrics: RwLock<HealthMetrics>,
    /// Configuration
    config: GovernorConfig,
    /// Hysteresis counter for state transitions
    hysteresis: RwLock<HysteresisState>,
}

/// Tracks consecutive checks for hysteresis-based transitions.
#[derive(Default)]
struct HysteresisState {
    consecutive_high_cpu: u8,
    consecutive_low_cpu: u8,
}

impl Governor {
    /// Create a new Governor with default configuration.
    pub fn new() -> Self {
        Self::with_config(GovernorConfig::default())
    }

    /// Create a new Governor with custom configuration.
    pub fn with_config(config: GovernorConfig) -> Self {
        Self {
            mode: AtomicU8::new(OperationalMode::Performance.as_u8()),
            metrics: RwLock::new(HealthMetrics::default()),
            config,
            hysteresis: RwLock::new(HysteresisState::default()),
        }
    }

    /// Get current mode. Uses relaxed atomic for speed.
    #[inline]
    pub fn current_mode(&self) -> OperationalMode {
        OperationalMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    /// Check if we're in Survival mode (fast path for message filtering).
    #[inline]
    pub fn is_survival(&self) -> bool {
        self.mode.load(Ordering::Relaxed) == OperationalMode::Survival.as_u8()
    }

    /// Check if we're NOT in Performance mode (any throttling active).
    #[inline]
    pub fn is_throttling(&self) -> bool {
        self.mode.load(Ordering::Relaxed) != OperationalMode::Performance.as_u8()
    }

    /// Get current health metrics (for debugging/monitoring).
    pub async fn metrics(&self) -> HealthMetrics {
        self.metrics.read().await.clone()
    }

    /// Get the configuration.
    pub fn config(&self) -> &GovernorConfig {
        &self.config
    }

    /// Start the background monitoring loop.
    ///
    /// This spawns a Tokio task that runs until the Governor is dropped.
    /// Returns an `Arc<Governor>` for sharing across the SDK.
    pub fn start(config: GovernorConfig) -> Arc<Self> {
        let governor = Arc::new(Self::with_config(config));
        let governor_clone = Arc::clone(&governor);

        tokio::spawn(async move {
            governor_clone.monitor_loop().await;
        });

        info!("Governor started with {:?}", governor.config);
        governor
    }

    /// The main monitoring loop.
    async fn monitor_loop(&self) {
        use sysinfo::System;

        let mut sys = System::new();
        let mut interval = interval(Duration::from_millis(self.config.sample_interval_ms));

        loop {
            interval.tick().await;

            // Refresh only CPU and memory (minimal cost)
            sys.refresh_cpu_usage();
            sys.refresh_memory();

            // Calculate metrics
            let cpu_usage = sys.global_cpu_usage() as f64;
            let available_ram_mb = sys.available_memory() / (1024 * 1024);
            let total_ram_mb = sys.total_memory() / (1024 * 1024);

            let new_metrics = HealthMetrics {
                cpu_usage_percent: cpu_usage,
                available_ram_mb,
                total_ram_mb,
            };

            // Update stored metrics
            {
                let mut metrics = self.metrics.write().await;
                *metrics = new_metrics.clone();
            }

            // Determine new mode with hysteresis
            let new_mode = self.calculate_mode_with_hysteresis(&new_metrics).await;
            let old_mode = OperationalMode::from_u8(self.mode.load(Ordering::Relaxed));

            if new_mode != old_mode {
                self.mode.store(new_mode.as_u8(), Ordering::Relaxed);
                self.log_transition(old_mode, new_mode, &new_metrics);
            }
        }
    }

    /// Calculate operational mode with hysteresis to prevent flickering.
    async fn calculate_mode_with_hysteresis(&self, metrics: &HealthMetrics) -> OperationalMode {
        let mut hysteresis = self.hysteresis.write().await;
        let current = OperationalMode::from_u8(self.mode.load(Ordering::Relaxed));

        // Determine raw mode from metrics
        let raw_mode = if metrics.cpu_usage_percent > self.config.survival_cpu_threshold
            || metrics.available_ram_mb < self.config.survival_ram_threshold_mb
        {
            OperationalMode::Survival
        } else if metrics.cpu_usage_percent > self.config.balanced_cpu_threshold
            || metrics.available_ram_mb < self.config.balanced_ram_threshold_mb
        {
            OperationalMode::Balanced
        } else {
            OperationalMode::Performance
        };

        // Apply hysteresis: require consecutive checks before transitioning
        match (current, raw_mode) {
            // Escalating to higher mode (faster response)
            (OperationalMode::Performance, OperationalMode::Balanced)
            | (OperationalMode::Performance, OperationalMode::Survival)
            | (OperationalMode::Balanced, OperationalMode::Survival) => {
                hysteresis.consecutive_high_cpu += 1;
                hysteresis.consecutive_low_cpu = 0;
                if hysteresis.consecutive_high_cpu >= self.config.hysteresis_threshold {
                    hysteresis.consecutive_high_cpu = 0;
                    raw_mode
                } else {
                    current
                }
            }
            // De-escalating to lower mode (slower response for stability)
            (OperationalMode::Survival, OperationalMode::Balanced)
            | (OperationalMode::Survival, OperationalMode::Performance)
            | (OperationalMode::Balanced, OperationalMode::Performance) => {
                hysteresis.consecutive_low_cpu += 1;
                hysteresis.consecutive_high_cpu = 0;
                // Require more consecutive checks to de-escalate (stability)
                if hysteresis.consecutive_low_cpu >= self.config.hysteresis_threshold + 1 {
                    hysteresis.consecutive_low_cpu = 0;
                    raw_mode
                } else {
                    current
                }
            }
            // Same mode
            _ => {
                hysteresis.consecutive_high_cpu = 0;
                hysteresis.consecutive_low_cpu = 0;
                current
            }
        }
    }

    /// Log mode transitions with context.
    fn log_transition(&self, old: OperationalMode, new: OperationalMode, metrics: &HealthMetrics) {
        match new {
            OperationalMode::Performance => {
                info!("{} -> {} (CPU {:.0}%)", old, new, metrics.cpu_usage_percent);
            }
            OperationalMode::Balanced => {
                warn!("{} -> {} (CPU {:.0}%)", old, new, metrics.cpu_usage_percent);
            }
            OperationalMode::Survival => {
                warn!("{} -> {} (CPU {:.0}%) SHEDDING", old, new, metrics.cpu_usage_percent);
            }
        }
    }
}

impl Default for Governor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_atomic_access() {
        let governor = Governor::new();
        assert_eq!(governor.current_mode(), OperationalMode::Performance);
        assert!(!governor.is_survival());
        assert!(!governor.is_throttling());
    }

    #[test]
    fn test_mode_transitions() {
        let governor = Governor::new();

        // Manually set mode
        governor.mode.store(OperationalMode::Survival.as_u8(), Ordering::Relaxed);
        assert!(governor.is_survival());
        assert!(governor.is_throttling());

        governor.mode.store(OperationalMode::Balanced.as_u8(), Ordering::Relaxed);
        assert!(!governor.is_survival());
        assert!(governor.is_throttling());
    }
}
