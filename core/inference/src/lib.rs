// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Blazil Inference — High-performance model inference for AI/ML workloads.
//!
//! **Design Goals:**
//! - Production-grade ONNX inference via Tract (pure Rust, stable)
//! - Zero-copy pipeline from dataloader to inference
//! - Batch processing for maximum throughput
//! - Thread-safe model sharing across async tasks
//! - Comprehensive error handling and logging
//!
//! **Backend:** Tract (Sonos/tract) — pure Rust, no C dependencies
//! **Throughput Target:** 10K+ inferences/sec on 8× GPU setup
//!
//! # Example — Image Classification
//! ```no_run
//! use blazil_inference::{
//!     OnnxModel, InferencePipeline, InferenceConfig, Device, InferenceModel,
//! };
//! use blazil_dataloader::{
//!     datasets::ImageNetDataset, DatasetConfig, Dataset, Pipeline,
//! };
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. Load dataset
//! let dataset = ImageNetDataset::open(
//!     "/data/imagenet",
//!     DatasetConfig::default().with_batch_size(64),
//! )?;
//!
//! // 2. Load ONNX model (CPU, pure Rust via Tract)
//! let model_config = InferenceConfig::new("resnet50.onnx")
//!     .with_device(Device::Cpu)
//!     .with_batch_size(64);
//! let model = OnnxModel::load(model_config)?;
//!
//! // 3. Create pipelines
//! let data_pipeline = Pipeline::new(dataset, DatasetConfig::default());
//! let data_stream = data_pipeline.stream();
//!
//! let inference_pipeline = InferencePipeline::new(model, 4);
//! let mut inference_stream = inference_pipeline.stream(data_stream).await?;
//!
//! // 4. Consume predictions
//! while let Some(result) = inference_stream.recv().await {
//!     let batch = result?;
//!     for pred in batch.predictions {
//!         println!("Class: {:?}, Confidence: {:.2}", pred.class_id, pred.confidence);
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod error;
pub mod model;
pub mod onnx;
pub mod pipeline;

pub use config::{Device, InferenceConfig, OptimizationLevel};
pub use error::{Error, Result};
pub use model::{InferenceModel, Prediction};
pub use onnx::OnnxModel;
pub use pipeline::{InferenceBatch, InferencePipeline};
