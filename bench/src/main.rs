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

    println!("Events: sharded=100K (scaling sweep 1/2/4/8 shards)");
    println!("Events: tcp=10K, udp=100K (E2E transport comparison)");
    println!("Runs per scenario: 1 (fast mode)\n");

    // Sharded pipeline scaling test — full 1/2/4/8 sweep with table output
    println!("[1/3] Sharded Pipeline scaling sweep (100K events x 4 configs)...");
    sharded_pipeline_scenario::run_scaling_sweep().await;

    // E2E transport comparison
    println!("[2/3] TCP E2E (10K events)...");
    let tcp_result = tcp_scenario::run(10_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(tcp_result.tps)
    );

    println!("[3/3] UDP E2E (100K events)...");
    let udp_result = udp_scenario::run(100_000).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(udp_result.tps)
    );

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
    println!("Speedup:  {:.1}x over TCP", transport_speedup);
    println!(
        "Gap closed: {:.1}% (target was 20-30x)",
        (transport_speedup / 20.0) * 100.0
    );

    // io_uring UDP transport (Linux only, requires --features io-uring)
    #[cfg(all(feature = "io-uring", target_os = "linux"))]
    {
        use blazil_bench::scenarios::io_uring_udp_scenario;
        println!("[4/3] io_uring UDP E2E (100K events)...");
        let io_uring_result = io_uring_udp_scenario::run(100_000).await;
        println!(
            "      -> {} TPS",
            blazil_bench::report::fmt_commas(io_uring_result.tps)
        );
        let io_uring_speedup = io_uring_result.tps as f64 / udp_result.tps as f64;
        println!("\n=== io_uring vs epoll UDP ===");
        println!(
            "epoll UDP:    {} TPS",
            blazil_bench::report::fmt_commas(udp_result.tps)
        );
        println!(
            "io_uring UDP: {} TPS",
            blazil_bench::report::fmt_commas(io_uring_result.tps)
        );
        println!("Speedup: {:.2}x", io_uring_speedup);
    }

    println!("\nAll tests passed! OK");
}
