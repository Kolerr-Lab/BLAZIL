//! Blazil Benchmark Suite — CLI entry point.
//!
//! Runs all scenarios and prints a structured report to stdout.
//! Must be executed in `--release` mode for meaningful numbers:
//!
//! ```text
//! cargo run -p blazil-bench --release
//! ```

use std::mem::size_of;

#[cfg(feature = "aeron")]
use blazil_bench::scenarios::aeron_scenario;
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

    // Parse --events N from CLI args (used by scripts/aeron-bench.sh).
    // Defaults match the hardcoded values used before arg parsing was added.
    let args: Vec<String> = std::env::args().collect();
    let events: u64 = args
        .windows(2)
        .find(|w| w[0] == "--events")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(100_000);

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
    println!("Events: tcp=10K, udp={events}, aeron={events} (E2E transport comparison)");
    println!("Runs per scenario: 1 (fast mode)\n");

    // Sharded pipeline scaling test — full 1/2/4/8 sweep with table output
    println!("[1/4] Sharded Pipeline scaling sweep (100K events x 4 configs)...");
    sharded_pipeline_scenario::run_scaling_sweep().await;

    // E2E transport comparison
    println!("[2/4] TCP E2E (10K events)...");
    let tcp_result = tcp_scenario::run(10_000).await; // TCP baseline kept at 10K — slow transport
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(tcp_result.tps)
    );

    println!("[3/4] UDP E2E ({events} events)...");
    let udp_result = udp_scenario::run(events).await;
    println!(
        "      → {} TPS",
        blazil_bench::report::fmt_commas(udp_result.tps)
    );

    // Aeron IPC E2E (only when built with --features aeron)
    #[cfg(feature = "aeron")]
    let aeron_result = {
        println!("[4/4] Aeron IPC E2E ({events} events)...");
        let r = aeron_scenario::run(events).await;
        println!("      → {} TPS", blazil_bench::report::fmt_commas(r.tps));
        Some(r)
    };
    #[cfg(not(feature = "aeron"))]
    let aeron_result: Option<blazil_bench::metrics::BenchmarkResult> = None;

    // E2E transport comparison table
    let transport_speedup = udp_result.tps as f64 / tcp_result.tps as f64;
    println!("\n=== E2E TRANSPORT COMPARISON ===");
    println!(
        "TCP E2E:   {:>12} TPS (baseline)",
        blazil_bench::report::fmt_commas(tcp_result.tps)
    );
    println!(
        "UDP E2E:   {:>12} TPS  ({:.1}x TCP)",
        blazil_bench::report::fmt_commas(udp_result.tps),
        transport_speedup,
    );
    if let Some(ref ar) = aeron_result {
        let aeron_speedup = ar.tps as f64 / tcp_result.tps as f64;
        println!(
            "Aeron IPC: {:>12} TPS  ({:.1}x TCP,  p99={}ns)",
            blazil_bench::report::fmt_commas(ar.tps),
            aeron_speedup,
            ar.p99_ns,
        );
    }
    println!("Speedup (UDP/TCP):  {:.1}x", transport_speedup);
    println!(
        "Gap closed: {:.1}% (target was 20-30x)",
        (transport_speedup / 20.0) * 100.0
    );

    // io_uring UDP transport (Linux only, requires --features io-uring)
    #[cfg(all(feature = "io-uring", target_os = "linux"))]
    {
        use blazil_bench::scenarios::io_uring_udp_scenario;
        println!("\n[5/5] io_uring UDP E2E (100K events)...");
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
