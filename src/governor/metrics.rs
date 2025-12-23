//! Health metrics from system telemetry.

/// Current system health metrics.
#[derive(Debug, Clone, Default)]
pub struct HealthMetrics {
    /// Global CPU usage percentage (0.0 - 100.0)
    pub cpu_usage_percent: f64,
    /// Available RAM in megabytes
    pub available_ram_mb: u64,
    /// Total RAM in megabytes
    pub total_ram_mb: u64,
}

impl HealthMetrics {
    /// Create new health metrics.
    pub fn new(cpu_usage_percent: f64, available_ram_mb: u64, total_ram_mb: u64) -> Self {
        Self {
            cpu_usage_percent,
            available_ram_mb,
            total_ram_mb,
        }
    }

    /// Get RAM usage as a percentage.
    pub fn ram_usage_percent(&self) -> f64 {
        if self.total_ram_mb == 0 {
            0.0
        } else {
            let used = self.total_ram_mb - self.available_ram_mb;
            (used as f64 / self.total_ram_mb as f64) * 100.0
        }
    }

    /// Check if system is under memory pressure.
    pub fn is_memory_critical(&self, threshold_mb: u64) -> bool {
        self.available_ram_mb < threshold_mb
    }
}

impl std::fmt::Display for HealthMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CPU: {:.1}%, RAM: {}MB/{:.1}MB ({:.1}% used)",
            self.cpu_usage_percent,
            self.available_ram_mb,
            self.total_ram_mb,
            self.ram_usage_percent()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ram_usage_percent() {
        let metrics = HealthMetrics::new(50.0, 4096, 16384);
        // Used: 16384 - 4096 = 12288
        // Percent: 12288 / 16384 = 75%
        assert!((metrics.ram_usage_percent() - 75.0).abs() < 0.1);
    }

    #[test]
    fn test_memory_critical() {
        let metrics = HealthMetrics::new(50.0, 500, 16384);
        assert!(metrics.is_memory_critical(512));
        assert!(!metrics.is_memory_critical(256));
    }

    #[test]
    fn test_display() {
        let metrics = HealthMetrics::new(45.5, 8192, 16384);
        let s = format!("{}", metrics);
        assert!(s.contains("45.5%"));
        assert!(s.contains("8192MB"));
    }
}
