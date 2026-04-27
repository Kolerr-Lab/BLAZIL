// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Inference throughput benchmark.

use blazil_dataloader::Sample;
use blazil_inference::{Device, InferenceConfig, InferenceModel, OnnxModel};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

fn download_model_if_needed() -> std::path::PathBuf {
    let cache_dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let model_path = cache_dir.join("squeezenet1.1.onnx");

    if !model_path.exists() {
        eprintln!("Downloading SqueezeNet 1.1 for benchmark...");
        let response = ureq::get("https://github.com/onnx/models/raw/main/validated/vision/classification/squeezenet/model/squeezenet1.1-7.onnx")
            .call()
            .expect("Failed to download model");
        let mut file = std::fs::File::create(&model_path).expect("Failed to create file");
        std::io::copy(&mut response.into_reader(), &mut file).expect("Failed to write");
    }

    model_path
}

fn bench_inference_throughput(c: &mut Criterion) {
    let model_path = download_model_if_needed();

    let mut group = c.benchmark_group("inference_throughput");

    for batch_size in [1, 8, 16, 32, 64] {
        let config = InferenceConfig::new(&model_path)
            .with_device(Device::Cpu)
            .with_batch_size(batch_size)
            .with_threads(4, 1);

        let model = OnnxModel::load(config).expect("Failed to load model");

        // Pre-generate dummy samples.
        let dummy = vec![128u8; 224 * 224 * 3];
        let samples: Vec<Sample> = (0..batch_size)
            .map(|i| Sample {
                data: dummy.clone(),
                label: i as u32,
                metadata: None,
            })
            .collect();

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("squeezenet_cpu", batch_size),
            &batch_size,
            |b, _| {
                b.iter(|| {
                    let predictions = model.run_batch(black_box(&samples)).unwrap();
                    black_box(predictions);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_inference_throughput);
criterion_main!(benches);
