// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Benchmarks for BitNet 1-bit kernels.
//!
//! Run with: cargo bench --bench bitnet_kernels

use blazil_inference::kernels::{
    bitnet_linear_1bit, dequantize_int8, pack_weights_1bit, quantize_int8,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// Naive f32 matrix-vector multiplication (baseline).
fn naive_matvec_f32(weights: &[f32], input: &[f32], rows: usize, cols: usize, output: &mut [f32]) {
    for row in 0..rows {
        let mut sum = 0.0;
        for col in 0..cols {
            sum += input[col] * weights[row * cols + col];
        }
        output[row] = sum;
    }
}

fn bench_bitnet_linear(c: &mut Criterion) {
    let mut group = c.benchmark_group("bitnet_linear");

    // Common sizes for LLM layers
    let sizes = vec![
        (1024, 1024), // Small layer
        (4096, 4096), // Typical LLM hidden size
        (8192, 8192), // Large layer
    ];

    for (rows, cols) in sizes {
        let param = format!("{rows}x{cols}");
        let throughput = Throughput::Elements((rows * cols) as u64);
        group.throughput(throughput);

        // Prepare data
        let input: Vec<f32> = (0..cols).map(|i| (i as f32) * 0.01).collect();
        let weights_f32: Vec<f32> = (0..(rows * cols))
            .map(|i| ((i % 100) as f32 - 50.0) * 0.02)
            .collect();
        let weights_packed = pack_weights_1bit(&weights_f32, rows, cols, 0.0);
        let mut output = vec![0.0; rows];

        // Benchmark 1-bit linear
        group.bench_with_input(
            BenchmarkId::new("1bit", &param),
            &(&input, &weights_packed, rows, cols),
            |b, &(inp, wts, r, c)| {
                b.iter(|| {
                    bitnet_linear_1bit(
                        black_box(inp),
                        black_box(wts),
                        black_box(r),
                        black_box(c),
                        black_box(&mut output),
                    )
                    .unwrap()
                });
            },
        );

        // Benchmark naive f32 (baseline)
        group.bench_with_input(
            BenchmarkId::new("f32_naive", &param),
            &(&input, &weights_f32, rows, cols),
            |b, &(inp, wts, r, c)| {
                b.iter(|| {
                    naive_matvec_f32(
                        black_box(wts),
                        black_box(inp),
                        black_box(r),
                        black_box(c),
                        black_box(&mut output),
                    )
                });
            },
        );
    }

    group.finish();
}

fn bench_weight_packing(c: &mut Criterion) {
    let mut group = c.benchmark_group("weight_packing");

    let sizes = vec![(1024, 1024), (4096, 4096), (8192, 8192)];

    for (rows, cols) in sizes {
        let total = rows * cols;
        let throughput = Throughput::Elements(total as u64);
        group.throughput(throughput);

        let weights: Vec<f32> = (0..total)
            .map(|i| ((i % 100) as f32 - 50.0) * 0.02)
            .collect();

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{rows}x{cols}")),
            &(&weights, rows, cols),
            |b, &(wts, r, c)| {
                b.iter(|| {
                    pack_weights_1bit(black_box(wts), black_box(r), black_box(c), black_box(0.0))
                });
            },
        );
    }

    group.finish();
}

fn bench_int8_quantization(c: &mut Criterion) {
    let mut group = c.benchmark_group("int8_quantization");

    let sizes = vec![1024, 4096, 16384, 65536];

    for size in sizes {
        let throughput = Throughput::Elements(size as u64);
        group.throughput(throughput);

        let input: Vec<f32> = (0..size).map(|i| (i as f32) * 0.001).collect();

        group.bench_with_input(BenchmarkId::new("quantize", size), &input, |b, inp| {
            b.iter(|| quantize_int8(black_box(inp)));
        });

        // Benchmark dequantization
        let (quantized, scale) = quantize_int8(&input);
        group.bench_with_input(
            BenchmarkId::new("dequantize", size),
            &(&quantized, scale),
            |b, &(quant, sc)| {
                b.iter(|| dequantize_int8(black_box(quant), black_box(sc)));
            },
        );

        // Benchmark roundtrip
        group.bench_with_input(BenchmarkId::new("roundtrip", size), &input, |b, inp| {
            b.iter(|| {
                let (q, s) = quantize_int8(black_box(inp));
                dequantize_int8(black_box(&q), black_box(s))
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_bitnet_linear,
    bench_weight_packing,
    bench_int8_quantization
);
criterion_main!(benches);
