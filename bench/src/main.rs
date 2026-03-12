//! Blazil Benchmark Suite — CLI entry point.
//!
//! Runs all scenarios and prints a structured report to stdout.
//! Must be executed in `--release` mode for meaningful numbers:
//!
//! ```text
//! cargo run -p blazil-bench --release
//! ```

use blazil_bench::{
    report,
    scenarios::{pipeline_scenario, ring_buffer_scenario, tcp_scenario, tigerbeetle_scenario},
};

#[tokio::main]
async fn main() {
    // Suppress all tracing output during benchmarks — results go to stdout.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .try_init();

    println!("Starting Blazil benchmark suite...\n");
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
