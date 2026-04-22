// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Blazil Dataloader benchmark tool.
//!
//! Measures dataset streaming throughput and batch latency.
//! Mimics a GPU training loop: pop batch → simulate GPU transfer → next batch.
//!
//! # Usage
//! ```bash
//! ./target/release/ml-bench \
//!   --dataset imagenet \
//!   --path /data/imagenet \
//!   --batch-size 256 \
//!   --duration 120
//! ```

use blazil_dataloader::{datasets::ImageNetDataset, Dataset, DatasetConfig, Pipeline};
use clap::Parser;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::timeout;

// ─────────────────────────────────────────────
// CLI Arguments
// ─────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "ml-bench")]
#[command(version = "0.1.0")]
#[command(about = "Blazil Dataloader benchmark — dataset streaming throughput")]
struct Args {
    /// Dataset type (currently: imagenet)
    #[arg(long, default_value = "imagenet")]
    dataset: String,

    /// Path to dataset root directory
    #[arg(long)]
    path: String,

    /// Batch size (samples per batch)
    #[arg(long, default_value_t = 256)]
    batch_size: usize,

    /// Benchmark duration in seconds
    #[arg(long, default_value_t = 120)]
    duration: u64,

    /// Number of parallel decode workers
    #[arg(long, default_value_t = 8)]
    num_workers: usize,

    /// Ring buffer depth (in-flight batches)
    #[arg(long, default_value_t = 256)]
    ring_capacity: usize,

    /// Shuffle samples (reproducible with --seed)
    #[arg(long)]
    shuffle: bool,

    /// RNG seed for reproducible shuffling
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Warmup duration in seconds before recording metrics
    #[arg(long, default_value_t = 5)]
    warmup: u64,

    /// Print progress every N seconds
    #[arg(long, default_value_t = 10)]
    report_interval: u64,

    /// Verbose debug output
    #[arg(long, short)]
    verbose: bool,
}

// ─────────────────────────────────────────────
// Metrics
// ─────────────────────────────────────────────

struct Metrics {
    total_samples: AtomicU64,
    total_batches: AtomicU64,
    total_errors: AtomicU64,
}

impl Metrics {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            total_samples: AtomicU64::new(0),
            total_batches: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        })
    }
}

// Approximate percentile from a sorted slice.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 * pct / 100.0) as usize).min(sorted.len() - 1);
    sorted[idx]
}

// ─────────────────────────────────────────────
// Entry Point
// ─────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(if args.verbose { "debug" } else { "info" })
        .init();

    let config = DatasetConfig::default()
        .with_batch_size(args.batch_size)
        .with_workers(args.num_workers)
        .with_ring_capacity(args.ring_capacity)
        .with_shuffle(args.shuffle)
        .with_seed(args.seed);

    // ── Load dataset ──────────────────────────
    print_header(&args);
    let t_load = Instant::now();

    let dataset = match args.dataset.as_str() {
        "imagenet" => ImageNetDataset::open(&args.path, config.clone())?,
        other => anyhow::bail!("Unknown dataset '{}'. Supported: imagenet", other),
    };

    let num_samples = dataset.len();
    let num_classes = dataset.num_classes();
    let load_ms = t_load.elapsed().as_millis();

    println!("  Samples      : {num_samples}");
    println!("  Classes      : {num_classes}");
    println!("  Index time   : {load_ms}ms");
    println!("{}", DIVIDER);

    if num_samples == 0 {
        anyhow::bail!("Dataset is empty — check path: {}", args.path);
    }

    // ── Build pipeline ────────────────────────
    let pipeline = Pipeline::new(dataset, config);

    // ── Warmup ───────────────────────────────
    if args.warmup > 0 {
        println!("  Warmup: {}s ...", args.warmup);
        run_phase(&pipeline, Duration::from_secs(args.warmup), &args, false).await?;
        println!("  Warmup done.\n{}", DIVIDER);
    }

    // ── Benchmark ────────────────────────────
    println!("  Benchmark: {}s ...", args.duration);
    let (samples, batches, errors, latencies_us) =
        run_phase(&pipeline, Duration::from_secs(args.duration), &args, true).await?;

    // ── Report ───────────────────────────────
    print_report(
        args.duration as f64,
        samples,
        batches,
        errors,
        &latencies_us,
        args.batch_size,
    );

    Ok(())
}

// ─────────────────────────────────────────────
// Benchmark phase runner
// ─────────────────────────────────────────────

async fn run_phase(
    pipeline: &Pipeline<ImageNetDataset>,
    duration: Duration,
    args: &Args,
    record: bool,
) -> anyhow::Result<(u64, u64, u64, Vec<u64>)> {
    let metrics = Metrics::new();
    let mut latencies_us: Vec<u64> = Vec::new();
    let stop = Arc::new(AtomicBool::new(false));

    // Progress reporting task
    if record && args.report_interval > 0 {
        let m = Arc::clone(&metrics);
        let stop_flag = Arc::clone(&stop);
        let interval = args.report_interval;
        tokio::spawn(async move {
            let mut last_samples = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_secs(interval));
            ticker.tick().await; // skip first tick
            loop {
                ticker.tick().await;
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                let current = m.total_samples.load(Ordering::Relaxed);
                let delta = current - last_samples;
                let rate = delta / interval;
                let batches = m.total_batches.load(Ordering::Relaxed);
                let errors = m.total_errors.load(Ordering::Relaxed);
                println!(
                    "  [+{interval}s] samples={current:<10} batches={batches:<8} \
                     rate={rate:<8}/s errors={errors}"
                );
                last_samples = current;
            }
        });
    }

    let deadline = Instant::now() + duration;
    let mut rx = if args.shuffle {
        pipeline.stream_shuffled(args.seed)
    } else {
        pipeline.stream()
    };

    while Instant::now() < deadline {
        let batch_start = Instant::now();

        match timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(Ok(batch))) => {
                let n = batch.samples.len() as u64;
                if record {
                    metrics.total_samples.fetch_add(n, Ordering::Relaxed);
                    metrics.total_batches.fetch_add(1, Ordering::Relaxed);
                    latencies_us.push(batch_start.elapsed().as_micros() as u64);
                }
            }
            Ok(Some(Err(e))) => {
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::warn!(error = %e, "batch error");
            }
            Ok(None) => {
                // Dataset exhausted — loop (simulate multi-epoch training).
                rx = if args.shuffle {
                    pipeline.stream_shuffled(args.seed)
                } else {
                    pipeline.stream()
                };
            }
            Err(_) => {
                // Timeout — pipeline starved (too few workers or slow disk).
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::debug!("receive timeout — pipeline stalled");
            }
        }
    }

    stop.store(true, Ordering::Relaxed);

    let s = metrics.total_samples.load(Ordering::Relaxed);
    let b = metrics.total_batches.load(Ordering::Relaxed);
    let e = metrics.total_errors.load(Ordering::Relaxed);
    Ok((s, b, e, latencies_us))
}

// ─────────────────────────────────────────────
// Output formatting
// ─────────────────────────────────────────────

const DIVIDER: &str = "  ────────────────────────────────────────────────────────";
const DOUBLE: &str = "  ════════════════════════════════════════════════════════";

fn print_header(args: &Args) {
    println!();
    println!("{DOUBLE}");
    println!("  Blazil Dataloader Benchmark  v0.1.0");
    println!("{DOUBLE}");
    println!("  Dataset      : {}", args.dataset);
    println!("  Path         : {}", args.path);
    println!("  Batch size   : {}", args.batch_size);
    println!("  Workers      : {}", args.num_workers);
    println!("  Ring depth   : {}", args.ring_capacity);
    println!("  Shuffle      : {}", args.shuffle);
    println!("  Duration     : {}s", args.duration);
    println!("  Warmup       : {}s", args.warmup);
    println!("{DIVIDER}");
}

fn print_report(
    elapsed_s: f64,
    samples: u64,
    batches: u64,
    errors: u64,
    latencies_us: &[u64],
    _batch_size: usize,
) {
    let avg_samples_sec = samples as f64 / elapsed_s;
    let avg_batches_sec = batches as f64 / elapsed_s;
    let total_requests = samples + errors;
    let error_rate = if total_requests > 0 {
        errors as f64 / total_requests as f64 * 100.0
    } else {
        0.0
    };

    let mut sorted = latencies_us.to_vec();
    sorted.sort_unstable();
    let p50_ms = percentile(&sorted, 50.0) as f64 / 1000.0;
    let p99_ms = percentile(&sorted, 99.0) as f64 / 1000.0;
    let p999_ms = percentile(&sorted, 99.9) as f64 / 1000.0;

    println!();
    println!("{DOUBLE}");
    println!("  RESULTS");
    println!("{DOUBLE}");
    println!("  Duration         : {elapsed_s:.1}s");
    println!("  Total samples    : {samples}");
    println!("  Total batches    : {batches}");
    println!("  Errors           : {errors}  ({error_rate:.4}%)");
    println!("{DIVIDER}");
    println!("  Throughput");
    println!("    Avg            : {:.0} samples/sec", avg_samples_sec);
    println!("    Batches/sec    : {:.0} batches/sec", avg_batches_sec);
    println!("{DIVIDER}");
    println!("  Batch latency (decode + queue)");
    println!("    P50            : {p50_ms:.2}ms");
    println!("    P99            : {p99_ms:.2}ms");
    println!("    P999           : {p999_ms:.2}ms");
    println!("{DOUBLE}");

    let status = if error_rate <= 0.0 {
        "  ✓  Error rate: 0.00%  (target met)"
    } else if error_rate < 0.01 {
        "  ⚠  Error rate < 0.01%"
    } else {
        "  ✗  Error rate exceeds 0.01% threshold"
    };
    println!("{status}");

    let throughput_target = 10_000_000.0;
    if avg_samples_sec >= throughput_target {
        println!(
            "  ✓  Throughput: {:.1}M samples/sec  (target: 10M+)",
            avg_samples_sec / 1_000_000.0
        );
    } else {
        println!(
            "  ↑  Throughput: {:.1}M samples/sec  (target: 10M+ — add workers or faster disk)",
            avg_samples_sec / 1_000_000.0
        );
    }

    // Data volume
    let bytes_per_sample = 224 * 224 * 3; // 224×224 RGB
    let total_gb = (samples as f64 * bytes_per_sample as f64) / (1024.0_f64.powi(3));
    let bandwidth_gb_s = total_gb / elapsed_s;
    println!("  Bandwidth        : {bandwidth_gb_s:.2} GB/s  ({total_gb:.1} GB total)");

    println!("{DOUBLE}");
    println!();
}
