//! Criterion bench: sharded pipeline throughput.
//!
//! Reads shard count from BLAZIL_SHARD_COUNT env var (or falls back to
//! default_shard_count()).  Run with:
//!
//!   BLAZIL_SHARD_COUNT=4 cargo bench --bench sharded_pipeline_scenario

use std::sync::Arc;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::event::TransactionEvent;
use blazil_engine::sharded_pipeline::{from_env, ShardedPipeline};

// BENCH_EVENTS: measures single-thread producer write throughput.
// This bench intentionally measures ring buffer write latency per producer,
// NOT multi-shard scaling efficiency (use `cargo run -p blazil-bench --release` for that).
// Higher shard count = more cache misses from single producer = expected lower throughput here.
const BENCH_EVENTS: u64 = 1_000_000;
const CAPACITY_PER_SHARD: usize = 1_048_576;
const MAX_AMOUNT_UNITS: u64 = 1_000_000;

fn sharded_pipeline_bench(c: &mut Criterion) {
    let shard_count = from_env();

    let mut group = c.benchmark_group("sharded_pipeline");
    // Each bench call processes BENCH_EVENTS elements — Criterion will report thrpt.
    group.throughput(Throughput::Elements(BENCH_EVENTS));
    // Reduce sample size to avoid overly long CI runs.
    group.sample_size(10);

    group.bench_with_input(
        BenchmarkId::from_parameter(shard_count),
        &shard_count,
        |b, &sc| {
            // ── One-time pipeline setup (not timed) ──────────────────────────
            // Build once and warm up before the Criterion timing loop so the
            // 100 ms warmup-settle sleep is paid only once, not per iteration.
            let sharded = Arc::new(
                ShardedPipeline::new(sc, CAPACITY_PER_SHARD, MAX_AMOUNT_UNITS)
                    .expect("valid sharded pipeline"),
            );
            // Warmup: prime the ring buffers and shard worker threads.
            for i in 0u64..200 {
                let event = TransactionEvent::new(
                    TransactionId::new(),
                    AccountId::from_u64(i * sc as u64), // routes to shard 0
                    AccountId::new(),
                    100u64,
                    LedgerId::USD,
                    1,
                );
                sharded.publish_event(event).ok();
            }
            // Let shard workers drain the warmup events.
            std::thread::sleep(std::time::Duration::from_millis(50));

            // ── Criterion timing loop ─────────────────────────────────────────
            // Each iteration: pre-generate events (not timed), then time only
            // the ring-buffer publish phase.  Ring capacity (1 M) >> BENCH_EVENTS
            // (1 K) so publish_event never spins on backpressure.
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    // Pre-generate (outside timed region).
                    let events: Vec<TransactionEvent> = (0..BENCH_EVENTS)
                        .map(|i| {
                            // Spread across shards for realistic routing.
                            let account_id = i * sc as u64 + (i % sc as u64);
                            TransactionEvent::new(
                                TransactionId::new(),
                                AccountId::from_u64(account_id),
                                AccountId::new(),
                                100u64,
                                LedgerId::USD,
                                1,
                            )
                        })
                        .collect();

                    // Timed: pure ring-buffer publish, no allocation.
                    let start = Instant::now();
                    for event in events {
                        sharded.publish_event(event).ok();
                    }
                    total += start.elapsed();
                }
                total
            });
        },
    );

    group.finish();
}

criterion_group!(benches, sharded_pipeline_bench);
criterion_main!(benches);
