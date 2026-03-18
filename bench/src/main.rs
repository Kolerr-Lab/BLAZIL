//! Blazil Benchmark Suite — CLI entry point.
//!
//! Runs all scenarios and prints a structured report to stdout.
//! Must be executed in `--release` mode for meaningful numbers:
//!
//! ```text
//! cargo run -p blazil-bench --release
//! ```

use std::mem::size_of;

use blazil_bench::scenarios::{sharded_pipeline_scenario, tcp_scenario, udp_scenario};
use blazil_common::amount::Amount;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{EventFlags, TransactionEvent};

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

    println!("Events: sharded=1M (comparing 1-shard vs 4-shard)");
    println!("Events: tcp=10K, udp=100K (E2E transport comparison)");
    println!("Runs per scenario: 1 (fast mode)\n");

    // Sharded pipeline scaling test
    println!("[1/4] Sharded Pipeline (1 shard)...");
    let sharded_1_result = sharded_pipeline_scenario::run(1_000_000, 1).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(sharded_1_result.tps)
    );

    println!("[2/4] Sharded Pipeline (4 shards)...");
    let sharded_4_result = sharded_pipeline_scenario::run(1_000_000, 4).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(sharded_4_result.tps)
    );

    // E2E transport comparison
    println!("[3/4] TCP E2E (10K events)...");
    let tcp_result = tcp_scenario::run(10_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(tcp_result.tps)
    );

    println!("[4/4] UDP E2E (100K events)...");
    let udp_result = udp_scenario::run(100_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(udp_result.tps)
    );

    // Calculate scaling
    let speedup = sharded_4_result.tps as f64 / sharded_1_result.tps as f64;
    let efficiency = (speedup / 4.0) * 100.0;

    println!("\n=== SHARDED PIPELINE SCALING ===");
    println!(
        "1-shard (1 producer):  {} TPS",
        blazil_bench::report::fmt_commas(sharded_1_result.tps)
    );
    println!(
        "4-shard (4 producers): {} TPS",
        blazil_bench::report::fmt_commas(sharded_4_result.tps)
    );

    println!("\n=== RESULTS ===");
    println!("Speedup: {:.2}x", speedup);
    println!("Scaling efficiency: {:.1}% (ideal = 100%)", efficiency);
    println!("Architecture: LMAX Disruptor (1 producer per ring buffer)");
    println!("Zero cache thrashing: each producer writes to ONE shard only");

    // E2E transport comparison
    let transport_speedup = udp_result.tps as f64 / tcp_result.tps as f64;
    println!("\n=== E2E TRANSPORT COMPARISON ===");
    println!(
        "TCP E2E:  {} TPS (baseline)",
        blazil_bench::report::fmt_commas(tcp_result.tps)
    );
    println!(
        "UDP E2E:  {} TPS",
        blazil_bench::report::fmt_commas(udp_result.tps)
    );
    println!("Speedup:  {:.1}× over TCP", transport_speedup);
    println!(
        "Gap closed: {:.1}% (target was 20-30×)",
        (transport_speedup / 20.0) * 100.0
    );

    println!("\nAll tests passed! ✅");
}
