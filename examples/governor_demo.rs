//! Governor Demo - Resource-Aware SDK Management
//!
//! This example demonstrates the Resource-Aware Governor feature
//! which monitors CPU/RAM and dynamically throttles SDK activity.

use forge_sdk::{GovernorConfig, KrakenClient};
use std::time::Duration;
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging with INFO level
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info,forge_sdk=debug"))
        .init();

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║           Resource-Aware Governor Demo                      ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    // Configure the Governor with custom thresholds
    // This example uses lower thresholds so you can see mode changes more easily
    let config = GovernorConfig::builder()
        .balanced_cpu(50.0)   // Enter Balanced mode at 50% CPU
        .survival_cpu(80.0)   // Enter Survival mode at 80% CPU
        .balanced_ram_mb(4096) // Enter Balanced if < 4GB RAM free
        .survival_ram_mb(1024) // Enter Survival if < 1GB RAM free
        .sample_interval_ms(500) // Check every 500ms
        .hysteresis(2)        // Require 2 consecutive checks before transition
        .build();

    println!("Governor Configuration:");
    println!("  • Balanced Mode: CPU > {:.0}% OR RAM < {}MB", config.balanced_cpu_threshold, config.balanced_ram_threshold_mb);
    println!("  • Survival Mode: CPU > {:.0}% OR RAM < {}MB", config.survival_cpu_threshold, config.survival_ram_threshold_mb);
    println!("  • Sample Interval: {}ms", config.sample_interval_ms);
    println!("  • Hysteresis: {} consecutive checks\n", config.hysteresis_threshold);

    // Connect to Kraken with Governor enabled
    println!("Connecting to Kraken with Governor enabled...");
    let client = KrakenClient::connect_with_governor(
        "wss://ws.kraken.com/v2",
        config,
    ).await?;

    println!("Connected! Governor is now monitoring system resources.\n");

    // Get the governor for monitoring
    let governor = client.governor().expect("Governor should be enabled");

    // Monitor system health and mode changes
    println!("Monitoring system health (press Ctrl+C to exit):\n");
    println!("{:<15} {:<10} {:<20}", "MODE", "CPU %", "RAM (MB free)");
    println!("{}", "-".repeat(50));

    loop {
        // Get current mode (nanosecond operation)
        let mode = governor.current_mode();
        
        // Get detailed metrics (slightly slower, uses RwLock)
        let metrics = governor.metrics().await;

        println!(
            "{:<15} {:<10.1} {:<20}",
            format!("{}", mode),
            metrics.cpu_usage_percent,
            metrics.available_ram_mb
        );

        // Sleep for 1 second between updates
        sleep(Duration::from_secs(1)).await;
    }
}
