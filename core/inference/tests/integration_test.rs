// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Integration tests with real ONNX models.
//!
//! These tests download small pre-trained models from the ONNX Model Zoo
//! and verify end-to-end inference correctness.

use blazil_dataloader::Sample;
use blazil_inference::{Device, InferenceConfig, InferenceModel, OnnxModel};
use std::path::PathBuf;

/// Download a test model from ONNX Model Zoo if not cached.
///
/// Returns the path to the downloaded `.onnx` file.
fn download_test_model(model_name: &str, url: &str) -> PathBuf {
    let cache_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let model_path = cache_dir.join(format!("{}.onnx", model_name));

    if model_path.exists() {
        eprintln!("Using cached model: {}", model_path.display());
        return model_path;
    }

    eprintln!("Downloading test model from {}", url);
    let response = ureq::get(url).call().expect("Failed to download model");

    let mut file = std::fs::File::create(&model_path).expect("Failed to create model file");
    std::io::copy(&mut response.into_reader(), &mut file).expect("Failed to write model");

    eprintln!("Model downloaded to {}", model_path.display());
    model_path
}

#[test]
#[ignore = "downloads ~5 MB ONNX model from github.com — run with --ignored on a machine with internet access"]
fn test_load_onnx_model_cpu() {
    // SqueezeNet 1.1 — small (5 MB), fast, 1000-class ImageNet classifier.
    let model_path = download_test_model(
        "squeezenet1.1",
        "https://github.com/onnx/models/raw/main/validated/vision/classification/squeezenet/model/squeezenet1.1-7.onnx",
    );

    let config = InferenceConfig::new(model_path)
        .with_device(Device::Cpu)
        .with_batch_size(1);

    let model = OnnxModel::load(config).expect("Failed to load model");

    assert_eq!(model.input_shape(), (1, 3, 224, 224));
    assert_eq!(model.num_classes(), Some(1000));
}

#[test]
#[ignore = "downloads ~5 MB ONNX model from github.com — run with --ignored on a machine with internet access"]
fn test_run_inference_squeezenet() {
    let model_path = download_test_model(
        "squeezenet1.1",
        "https://github.com/onnx/models/raw/main/validated/vision/classification/squeezenet/model/squeezenet1.1-7.onnx",
    );

    let config = InferenceConfig::new(model_path)
        .with_device(Device::Cpu)
        .with_batch_size(2);

    let model = OnnxModel::load(config).expect("Failed to load model");

    // Create 2 dummy samples: 224×224×3 RGB (HWC format).
    let dummy_image = vec![128u8; 224 * 224 * 3]; // mid-grey
    let samples = vec![
        Sample {
            data: dummy_image.clone(),
            label: 0,
            metadata: None,
        },
        Sample {
            data: dummy_image,
            label: 1,
            metadata: None,
        },
    ];

    let predictions = model.run_batch(&samples).expect("Inference failed");

    assert_eq!(predictions.len(), 2);
    for pred in &predictions {
        assert!(pred.class_id.is_some());
        assert!(pred.confidence > 0.0 && pred.confidence <= 1.0);
        assert_eq!(pred.probabilities.as_ref().unwrap().len(), 1000);
    }
}

#[test]
#[cfg(feature = "cuda")]
fn test_load_onnx_model_cuda() {
    let model_path = download_test_model(
        "squeezenet1.1",
        "https://github.com/onnx/models/raw/main/validated/vision/classification/squeezenet/model/squeezenet1.1-7.onnx",
    );

    let config = InferenceConfig::new(model_path)
        .with_device(Device::Cuda)
        .with_cuda_device(0)
        .with_batch_size(4);

    let model = OnnxModel::load(config).expect("Failed to load CUDA model");

    // Run a quick inference to verify CUDA EP is working.
    let dummy = vec![128u8; 224 * 224 * 3];
    let samples = vec![Sample {
        data: dummy,
        label: 0,
        metadata: None,
    }];

    let predictions = model.run_batch(&samples).expect("CUDA inference failed");
    assert_eq!(predictions.len(), 1);
}
