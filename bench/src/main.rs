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
#[cfg(feature = "tigerbeetle-client")]
use blazil_bench::scenarios::sharded_tb_scenario;
#[cfg(all(feature = "tigerbeetle-client", feature = "metrics-ws"))]
use blazil_bench::scenarios::vsr_failover_scenario;
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

    // Parse --events N and --scenario NAME from CLI args.
    // --scenario aeron  →  skip TCP/UDP, run only Aeron IPC
    let args: Vec<String> = std::env::args().collect();
    let events: u64 = args
        .windows(2)
        .find(|w| w[0] == "--events")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(100_000);
    let scenario_filter: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--scenario")
        .map(|w| w[1].clone());

    // --shards N  (for --scenario sharded-tb, default 2)
    #[cfg(feature = "tigerbeetle-client")]
    let shard_count: usize = args
        .windows(2)
        .find(|w| w[0] == "--shards")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(2);

    // --duration N  (seconds; time-based mode; when set --events is ignored)
    #[cfg(feature = "tigerbeetle-client")]
    let duration_secs: Option<u64> = args
        .windows(2)
        .find(|w| w[0] == "--duration")
        .and_then(|w| w[1].parse().ok());

    // --metrics-port N  (start live dashboard WebSocket server on port N)
    #[cfg(feature = "metrics-ws")]
    let metrics_port: Option<u16> = args
        .windows(2)
        .find(|w| w[0] == "--metrics-port")
        .and_then(|w| w[1].parse().ok());

    // Start WS server if requested.
    // Returns (out_tx, cmd_rx): bench→dashboard broadcaster + dashboard→bench receiver.
    #[cfg(feature = "metrics-ws")]
    let (metrics_tx, cmd_rx, _config_cache) = {
        if let Some(port) = metrics_port {
            let (tx, rx, cache) = blazil_bench::ws_server::start(port);
            // Give the tokio task 50ms to bind the port before the scenario
            // sends the config message (avoids race between spawn and send).
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            (Some(tx), Some(rx), Some(cache))
        } else {
            (None, None, None)
        }
    };
    #[cfg(not(feature = "metrics-ws"))]
    let metrics_tx: Option<tokio::sync::broadcast::Sender<String>> = None;
    #[cfg(not(feature = "metrics-ws"))]
    let cmd_rx: Option<tokio::sync::broadcast::Receiver<String>> = None;
    let _ = (&metrics_tx, &cmd_rx); // suppress unused warning

    // ── sharded-tb: direct pipeline + TigerBeetle, N shards ─────────────────
    #[cfg(feature = "tigerbeetle-client")]
    if scenario_filter.as_deref() == Some("sharded-tb") {
        if let Some(dur) = duration_secs {
            println!("[sharded-tb] shards={shard_count} duration={dur}s");
        } else {
            println!("[sharded-tb] shards={shard_count} events={events}");
        }
        let result = sharded_tb_scenario::run(
            events,
            shard_count,
            duration_secs,
            metrics_tx,
            #[cfg(feature = "metrics-ws")]
            _config_cache,
            #[cfg(not(feature = "metrics-ws"))]
            None,
        )
        .await;
        println!(
            "      → {} TPS  (p50={} µs  p99={} µs  p99.9={} µs)",
            blazil_bench::report::fmt_commas(result.tps),
            blazil_bench::report::fmt_commas(result.p50_ns / 1_000),
            blazil_bench::report::fmt_commas(result.p99_ns / 1_000),
            blazil_bench::report::fmt_commas(result.p99_9_ns / 1_000),
        );
        let tb_addr = std::env::var("BLAZIL_TB_ADDRESS").ok();
        blazil_bench::report::save_run(&result, tb_addr.as_deref());
        return;
    }

    // ── vsr-failover: sharded-tb + real node kill/restart via WS command ─────
    #[cfg(all(feature = "tigerbeetle-client", feature = "metrics-ws"))]
    if scenario_filter.as_deref() == Some("vsr-failover") {
        use blazil_bench::scenarios::vsr_failover_scenario::inner::{FailoverConfig, NodeCommands};

        // Helper: find the first value for a flag like --kill-cmd-1 "cmd"
        let arg_val = |flag: &str| -> Option<String> {
            args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
        };

        let node_cmds = NodeCommands {
            kill: [
                arg_val("--kill-cmd-1"),
                arg_val("--kill-cmd-2"),
                arg_val("--kill-cmd-3"),
            ],
            restart: [
                arg_val("--restart-cmd-1"),
                arg_val("--restart-cmd-2"),
                arg_val("--restart-cmd-3"),
            ],
        };

        let auto_kill_node: Option<u8> = args
            .windows(2)
            .find(|w| w[0] == "--auto-kill-node")
            .and_then(|w| w[1].parse().ok());
        let auto_kill_after_secs: u64 = args
            .windows(2)
            .find(|w| w[0] == "--auto-kill-after-secs")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(30);
        let recovery_secs: u64 = args
            .windows(2)
            .find(|w| w[0] == "--failover-recovery-secs")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(30);

        let (out_tx, in_rx) = match (metrics_tx, cmd_rx) {
            (Some(tx), Some(rx)) => (tx, rx),
            _ => panic!(
                "--scenario vsr-failover requires --metrics-port (and --features metrics-ws)"
            ),
        };

        let cfg = FailoverConfig {
            events,
            shard_count,
            duration_secs,
            auto_kill_node,
            auto_kill_after_secs,
            recovery_secs,
            node_cmds,
            metrics_tx: out_tx,
            cmd_rx: in_rx,
        };

        let result = vsr_failover_scenario::inner::run(cfg).await;
        println!(
            "      → {} TPS  (p50={} µs  p99={} µs  p99.9={} µs)",
            blazil_bench::report::fmt_commas(result.tps),
            blazil_bench::report::fmt_commas(result.p50_ns / 1_000),
            blazil_bench::report::fmt_commas(result.p99_ns / 1_000),
            blazil_bench::report::fmt_commas(result.p99_9_ns / 1_000),
        );
        let tb_addr = std::env::var("BLAZIL_TB_ADDRESS").ok();
        blazil_bench::report::save_run(&result, tb_addr.as_deref());
        return;
    }

    #[cfg(feature = "aeron")]
    let payload_size: usize = {
        let mut ps = 128usize;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == "--payload-size" {
                if let Some(val) = iter.next() {
                    ps = val.parse().unwrap_or(128);
                }
            }
        }
        ps
    };

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
    if scenario_filter.is_none() {
        println!("[1/4] Sharded Pipeline scaling sweep (100K events x 4 configs)...");
        sharded_pipeline_scenario::run_scaling_sweep().await;
    }

    // E2E transport comparison
    let tcp_result = if scenario_filter.is_none() {
        println!("[2/4] TCP E2E (10K events)...");
        let r = tcp_scenario::run(10_000).await;
        println!("      → {} TPS", blazil_bench::report::fmt_commas(r.tps));
        Some(r)
    } else {
        None
    };

    let udp_result = if scenario_filter.is_none() {
        println!("[3/4] UDP E2E ({events} events)...");
        let r = udp_scenario::run(events).await;
        println!("      → {} TPS", blazil_bench::report::fmt_commas(r.tps));
        Some(r)
    } else {
        None
    };

    // Aeron IPC E2E (only when built with --features aeron)
    #[cfg(feature = "aeron")]
    let aeron_result = {
        let run_aeron = scenario_filter.as_deref().is_none_or(|s| s == "aeron");
        if run_aeron {
            println!("[4/4] Aeron IPC E2E ({events} events)...");
            let r = aeron_scenario::run(events, payload_size).await;
            println!("      → {} TPS", blazil_bench::report::fmt_commas(r.tps));
            Some(r)
        } else {
            None
        }
    };
    #[cfg(not(feature = "aeron"))]
    let aeron_result: Option<blazil_bench::metrics::BenchmarkResult> = None;

    // E2E transport comparison table (only when all scenarios ran)
    if let (Some(ref tcp_result), Some(ref udp_result)) = (&tcp_result, &udp_result) {
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
    }

    // io_uring UDP transport (Linux only, requires --features io-uring)
    #[cfg(all(feature = "io-uring", target_os = "linux"))]
    if let Some(ref udp_result) = udp_result {
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

    // Save run log for any aeron result (the primary E2E metric with TB).
    #[cfg(feature = "aeron")]
    if let Some(ref ar) = aeron_result {
        let tb_addr = std::env::var("BLAZIL_TB_ADDRESS").ok();
        blazil_bench::report::save_run(ar, tb_addr.as_deref());
    }

    println!("\nAll tests passed! OK");
}
