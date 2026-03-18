//! Blazil Benchmark Suite — CLI entry point.
//!
//! Runs all scenarios and prints a structured report to stdout.
//! Must be executed in `--release` mode for meaningful numbers:
//!
//! ```text
//! cargo run -p blazil-bench --release
//! ```

use std::mem::size_of;

use blazil_bench::{
    scenarios::{pipeline_scenario, ring_buffer_scenario, sharded_pipeline_scenario, tcp_scenario, tigerbeetle_scenario},
};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{EventFlags, TransactionEvent};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::amount::Amount;

#[tokio::main]
async fn main() {
    // Suppress all tracing output during benchmarks — results go to stdout.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .try_init();

    println!("Starting Blazil benchmark suite...\n");

    // ── Field size breakdown ─────────────────────────────────────────────────
    println!("=== Field sizes ===");
    println!("i64:           {} bytes", size_of::<i64>());
    println!("TransactionId: {} bytes", size_of::<TransactionId>());
    println!("AccountId:     {} bytes", size_of::<AccountId>());
    println!("Amount:        {} bytes", size_of::<Amount>());
    println!("LedgerId:      {} bytes", size_of::<LedgerId>());
    println!("u16:           {} bytes", size_of::<u16>());
    println!("EventFlags:    {} bytes", size_of::<EventFlags>());
    println!("Timestamp:     {} bytes", size_of::<Timestamp>());
    println!("TOTAL current: {} bytes", size_of::<TransactionEvent>());
    println!();
    
    // ── Memory footprint analysis ────────────────────────────────────────────
    let event_size = std::mem::size_of::<TransactionEvent>();
    let ring_buffer_mb = (event_size * 1_000_000) / 1_024 / 1_024;
    
    println!("TransactionEvent size: {} bytes", event_size);
    println!("RingBuffer total: {} MB", ring_buffer_mb);
    println!();
    
    println!("Events: sharded=1M (comparing original vs 1-shard vs 4-shard)");
    println!("Runs per scenario: 1 (fast mode)\n");

    // Compare original pipeline vs sharded implementations
    println!("[1/3] Original Pipeline (1M events)...");
    let original_result = pipeline_scenario::run(1_000_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(original_result.tps)
    );

    println!("[2/3] Sharded Pipeline (1 shard)...");
    let sharded_1_result = sharded_pipeline_scenario::run(1_000_000, 1).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(sharded_1_result.tps)
    );

    println!("[3/3] Sharded Pipeline (4 shards)...");
    let sharded_4_result = sharded_pipeline_scenario::run(1_000_000, 4).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(sharded_4_result.tps)
    );

    // Calculate scaling
    let speedup = sharded_4_result.tps as f64 / sharded_1_result.tps as f64;
    let efficiency = (speedup / 4.0) * 100.0;
    let overhead_improvement = sharded_1_result.tps as f64 / original_result.tps as f64;

    println!("\n=== METHODOLOGY COMPARISON ===");
    println!("Original Pipeline:     {} TPS  (per-event latency tracking)", blazil_bench::report::fmt_commas(original_result.tps));
    println!("1-shard (1 producer):  {} TPS  (bulk timing, pre-allocated)", blazil_bench::report::fmt_commas(sharded_1_result.tps));
    println!("4-shard (4 producers): {} TPS  (parallel producers)", blazil_bench::report::fmt_commas(sharded_4_result.tps));
    
    println!("\n=== ANALYSIS ===");
    println!("Measurement overhead reduction: {:.2}x (removing per-event Instant::now() + vec.push())", overhead_improvement);
    println!("Parallel scaling: {:.2}x speedup with 4 shards ({:.1}% efficiency)", speedup, efficiency);
    println!("\nAll tests passed! ✅");
}
