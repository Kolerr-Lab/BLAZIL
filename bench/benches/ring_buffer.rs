use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use blazil_bench::scenarios::ring_buffer_scenario;

fn bench_ring_buffer_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("ring_buffer");
    group.sample_size(50);

    for &size in &[1_000u64, 10_000, 100_000] {
        group.bench_with_input(
            BenchmarkId::new("publish_events", size),
            &size,
            |b, &size| {
                b.iter(|| ring_buffer_scenario::run(size));
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_ring_buffer_throughput);
criterion_main!(benches);
