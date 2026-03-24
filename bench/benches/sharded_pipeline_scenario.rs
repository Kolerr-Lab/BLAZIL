//! Criterion bench: sharded pipeline throughput.
//!
//! Reads shard count from BLAZIL_SHARD_COUNT env var (or falls back to
//! default_shard_count()).  Run with:
//!
//!   BLAZIL_SHARD_COUNT=4 cargo bench --bench sharded_pipeline_scenario

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use blazil_bench::scenarios::sharded_pipeline_scenario;
use blazil_engine::sharded_pipeline::from_env;

/// Event count kept small so each Criterion iteration completes in < 2 s.
const BENCH_EVENTS: u64 = 1_000;

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
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let result = sharded_pipeline_scenario::run_once_blocking(BENCH_EVENTS, sc);
                    total += std::time::Duration::from_millis(result.duration_ms);
                }
                total
            });
        },
    );

    group.finish();
}

criterion_group!(benches, sharded_pipeline_bench);
criterion_main!(benches);
