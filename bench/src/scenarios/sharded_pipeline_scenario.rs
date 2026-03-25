//! Sharded pipeline throughput benchmark.
//!
//! Tests independent sharded pipelines with configurable shard count.
//! Each shard has its own ring buffer and full handler chain.
//! Events are routed by account ID for deterministic processing.

use std::sync::Arc;
use std::time::Instant;

use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::event::TransactionEvent;
use blazil_engine::sharded_pipeline::ShardedPipeline;

use crate::metrics::BenchmarkResult;
use crate::report::fmt_commas;

const WARMUP_EVENTS: u64 = 10_000;
const BENCH_EVENTS: u64 = 1_000_000;
const CAPACITY_PER_SHARD: usize = 1_048_576;
const MAX_AMOUNT_UNITS: u64 = 1_000_000;

/// Run the full shard-scaling sweep (1 / 2 / 4 / 8 shards) and print a table.
///
/// Called from `bench/src/main.rs` as a drop-in for the previous two separate
/// `run(1M, 1)` / `run(1M, 4)` calls.
pub async fn run_scaling_sweep() {
    tokio::task::spawn_blocking(scaling_sweep_blocking)
        .await
        .expect("benchmark thread panicked")
}

fn scaling_sweep_blocking() {
    let shard_counts: &[usize] = &[1, 2, 4, 8];
    let mut results: Vec<(usize, BenchmarkResult)> = Vec::new();

    for &sc in shard_counts {
        let r = run_once_blocking(BENCH_EVENTS, sc);
        results.push((sc, r));
    }

    let baseline_tps = results[0].1.tps as f64;

    println!();
    println!("  +---------+-------------+----------+----------+------------+");
    println!("  | Shards  | TPS         | P99 (ns) | P99.9    | Efficiency |");
    println!("  +---------+-------------+----------+----------+------------+");
    for (sc, r) in &results {
        let efficiency = if *sc == 1 {
            "baseline  ".to_string()
        } else {
            let speedup = r.tps as f64 / baseline_tps;
            let eff = (speedup / *sc as f64) * 100.0;
            format!("{:>8.1}% ", eff)
        };
        println!(
            "  | {:<7} | {:>11} | {:>8} | {:>8} | {} |",
            sc,
            fmt_commas(r.tps),
            r.p99_ns,
            r.p99_9_ns,
            efficiency,
        );
    }
    println!("  +---------+-------------+----------+----------+------------+");
    println!();
}

/// Run the sharded pipeline scenario with the specified shard count once.
pub async fn run(events: u64, shard_count: usize) -> BenchmarkResult {
    tokio::task::spawn_blocking(move || run_once_blocking(events, shard_count))
        .await
        .expect("benchmark thread panicked")
}

/// Synchronous benchmark body for sharded pipeline.
///
/// Public so that Criterion bench targets can call it directly.
pub fn run_once_blocking(events: u64, shard_count: usize) -> BenchmarkResult {
    println!("Running with {} shards", shard_count);
    // Create sharded pipeline with N independent shards
    let sharded = Arc::new(
        ShardedPipeline::new(shard_count, CAPACITY_PER_SHARD, MAX_AMOUNT_UNITS)
            .expect("valid sharded pipeline"),
    );

    // Warmup with single-threaded producer
    for i in 0..WARMUP_EVENTS {
        let event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::from_u64(i),
            AccountId::new(),
            1_00_u64,
            LedgerId::USD,
            1,
        );
        publish_with_backpressure(&sharded, event);
    }

    // Wait for warmup to complete
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Multi-threaded producers: spawn N producer threads (match shard count)
    // Each producer handles events_per_thread events
    let num_producers = shard_count;
    let events_per_thread = events / num_producers as u64;

    let barrier = Arc::new(std::sync::Barrier::new(num_producers + 1)); // +1 for main thread
    let mut handles = Vec::new();

    for thread_id in 0..num_producers {
        let sharded = Arc::clone(&sharded);
        let barrier = Arc::clone(&barrier);

        let handle = std::thread::spawn(move || {
            // Pre-generate events that ALL route to ONE specific shard
            // This gives perfect cache locality: each producer only touches ONE ring buffer
            //
            // Example for 4 shards:
            //   Thread 0 → AccountIds 0, 4, 8, 12...  → ALL map to shard 0
            //   Thread 1 → AccountIds 1, 5, 9, 13...  → ALL map to shard 1
            //   Thread 2 → AccountIds 2, 6, 10, 14...  → ALL map to shard 2
            //   Thread 3 → AccountIds 3, 7, 11, 15...  → ALL map to shard 3
            //
            // This is the LMAX Disruptor pattern: 1 producer per ring buffer!
            let target_shard = thread_id;
            let mut thread_events = Vec::with_capacity(events_per_thread as usize);

            for i in 0..events_per_thread {
                // Generate account ID that maps to target_shard
                // Formula: account_id = (i * shard_count) + target_shard
                // Verification: account_id % shard_count == target_shard ✓
                let account_id = (i * shard_count as u64) + target_shard as u64;

                let event = TransactionEvent::new(
                    TransactionId::new(),
                    AccountId::from_u64(account_id),
                    AccountId::new(),
                    1_00_u64,
                    LedgerId::USD,
                    1,
                );
                thread_events.push(event);
            }

            // Wait for all producers to be ready (all events pre-generated)
            barrier.wait();

            // Timed section: record per-event publish latency.
            // `publish_with_backpressure` is sync, so Instant::now() before/after
            // each call accurately captures ring-buffer round-trip cost.
            let mut latencies = Vec::with_capacity(events_per_thread as usize);
            let start = Instant::now();
            for event in thread_events {
                let t0 = Instant::now();
                publish_with_backpressure(&sharded, event);
                latencies.push(t0.elapsed().as_nanos() as u64);
            }
            let duration = start.elapsed();

            (events_per_thread, duration, latencies)
        });
        handles.push(handle);
    }

    // Start all producers simultaneously
    barrier.wait();
    let overall_start = Instant::now();

    // Wait for all producers to finish and collect results
    let mut total_events = 0;
    let mut max_duration = std::time::Duration::ZERO;
    let mut all_latencies: Vec<u64> = Vec::new();
    for handle in handles {
        let (thread_events, thread_duration, mut lats) =
            handle.join().expect("producer thread panicked");
        total_events += thread_events;
        max_duration = max_duration.max(thread_duration);
        all_latencies.append(&mut lats);
    }

    let _overall_duration = overall_start.elapsed();

    // Wait for all shards to finish processing (after timing stops).
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Drop the Arc — refcount is 1 (all producer threads have joined),
    // so Drop fires immediately and calls ShardedPipeline::stop_internal().
    drop(sharded);

    // Use max thread duration as the effective duration (bottleneck)
    BenchmarkResult::new(
        &format!(
            "Sharded Pipeline ({} shards, {} producers)",
            shard_count, num_producers
        ),
        total_events,
        max_duration,
        &mut all_latencies,
    )
}

/// Publish with spin-retry on backpressure.
///
/// Spins up to 1 000 times (fast path), then yields to the OS scheduler to
/// avoid pegging a core when the ring is genuinely full.
fn publish_with_backpressure(sharded: &ShardedPipeline, event: TransactionEvent) -> i64 {
    let mut event = event;
    let mut spins = 0usize;
    loop {
        match sharded.publish_event(event) {
            Ok(seq) => return seq,
            Err(_) => {
                spins += 1;
                if spins < 1_000 {
                    std::hint::spin_loop();
                } else {
                    std::thread::yield_now();
                    spins = 0;
                }
                event = TransactionEvent::new(
                    TransactionId::new(),
                    AccountId::new(),
                    AccountId::new(),
                    1_00_u64,
                    LedgerId::USD,
                    1,
                );
            }
        }
    }
}
