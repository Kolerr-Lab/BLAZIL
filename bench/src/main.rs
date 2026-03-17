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
    report,
    scenarios::{pipeline_scenario, ring_buffer_scenario, tcp_scenario, tigerbeetle_scenario},
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
    
    println!("Events: ring_buffer=1M  pipeline=1M  tcp=10K  tb=10K");
    println!("Runs per scenario: 3 (median reported)\n");

    println!("[1/4] Ring buffer (raw)...");
    let ring_result = ring_buffer_scenario::run(1_000_000);
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(ring_result.tps)
    );

    println!("[2/4] Pipeline (in-memory)...");
    let pipeline_result = pipeline_scenario::run(1_000_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(pipeline_result.tps)
    );

    println!("[3/4] End-to-end TCP...");
    let tcp_result = tcp_scenario::run(100_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(tcp_result.tps)
    );

    println!("[4/4] TigerBeetle (real)...");
    let tb_result = tigerbeetle_scenario::run(10_000).await;
    if let Some(ref r) = tb_result {
        println!("      → {} TPS", blazil_bench::report::fmt_commas(r.tps));
    }

    report::print_report(
        &ring_result,
        &pipeline_result,
        &tcp_result,
        tb_result.as_ref(),
    );
}
