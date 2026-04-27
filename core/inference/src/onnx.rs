// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! ONNX inference backend using Tract (pure Rust).
//!
//! Tract is a production-grade inference engine maintained by Sonos,
//! used in production by Hugging Face, Snips, and others.
//!
//! Benefits over ONNX Runtime bindings:
//! - Pure Rust → no C/C++ build dependencies
//! - Stable API (no yanked versions or RC chaos)
//! - Better cross-compilation story
//! - Integrated graph optimization

use crate::{
    config::{Device, InferenceConfig, OptimizationLevel},
    model::{InferenceModel, Prediction},
    Error, Result,
};
use blazil_dataloader::Sample;
use ndarray::{Array4, Axis};
use std::sync::Arc;
use tract_onnx::prelude::*;

// Type alias to simplify complex Tract types
type RunnableModel = Arc<TypedRunnableModel<TypedModel>>;
type ModelMetadata = ((usize, usize, usize, usize), Option<usize>);

/// ONNX inference model using Tract.
///
/// Thread-safe: uses `Arc<TypedRunnableModel<TypedModel>>` for thread-safe sharing.
pub struct OnnxModel {
    model: RunnableModel,
    config: InferenceConfig,
    input_shape: (usize, usize, usize, usize), // (B, C, H, W)
    num_classes: Option<usize>,
}

impl OnnxModel {
    /// Initialize Tract (no-op, kept for API compatibility).
    pub fn init_environment() -> Result<()> {
        // Tract is pure Rust, no global environment initialization needed.
        Ok(())
    }

    /// Load and optimize the ONNX model.
    fn load_and_optimize(config: &InferenceConfig) -> Result<RunnableModel> {
        config.validate()?;

        // Load ONNX model
        let model = tract_onnx::onnx()
            .model_for_path(&config.model_path)
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("tract load '{}': {e}", config.model_path.display()),
            })?;

        // Convert to TypedModel
        let typed_model = model.into_typed().map_err(|e| Error::ModelLoadFailed {
            reason: format!("type inference: {e}"),
        })?;

        // Apply optimization
        let optimized_model = match config.optimization_level {
            OptimizationLevel::Disable => typed_model,
            OptimizationLevel::Basic => {
                typed_model
                    .into_optimized()
                    .map_err(|e| Error::ModelLoadFailed {
                        reason: format!("basic optimization: {e}"),
                    })?
            }
            OptimizationLevel::Extended | OptimizationLevel::All => typed_model
                .into_optimized()
                .map_err(|e| Error::ModelLoadFailed {
                    reason: format!("optimization: {e}"),
                })?
                .into_decluttered()
                .map_err(|e| Error::ModelLoadFailed {
                    reason: format!("declutter: {e}"),
                })?,
        };

        // Compile to runnable plan
        let runnable = optimized_model
            .into_runnable()
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("compile: {e}"),
            })?;

        Ok(Arc::new(runnable))
    }

    /// Infer input shape and output classes from the runnable model metadata.
    fn infer_metadata(model: &TypedRunnableModel<TypedModel>) -> Result<ModelMetadata> {
        // Get input shape from the runnable model's underlying TypedModel
        let typed_model = model.model();

        let input_fact = typed_model
            .input_fact(0)
            .map_err(|e| Error::InvalidModelFormat {
                reason: format!("no input 0: {e}"),
            })?;

        let input_shape = &input_fact.shape;

        if input_shape.len() != 4 {
            return Err(Error::InvalidModelFormat {
                reason: format!("expected 4D input (NCHW), got {input_shape:?}"),
            });
        }

        // Convert TDim to usize - handle symbolic batch dimensions
        let batch_size = input_shape[0].to_i64().unwrap_or(1) as usize; // Default to 1 if symbolic
        let channels = input_shape[1]
            .to_i64()
            .map_err(|e| Error::InvalidModelFormat {
                reason: format!("channels not concrete: {e}"),
            })? as usize;
        let height = input_shape[2]
            .to_i64()
            .map_err(|e| Error::InvalidModelFormat {
                reason: format!("height not concrete: {e}"),
            })? as usize;
        let width = input_shape[3]
            .to_i64()
            .map_err(|e| Error::InvalidModelFormat {
                reason: format!("width not concrete: {e}"),
            })? as usize;

        // Get output shape (for classification: [B, num_classes])
        let output_fact = typed_model
            .output_fact(0)
            .map_err(|e| Error::InvalidModelFormat {
                reason: format!("no output 0: {e}"),
            })?;

        let output_shape = &output_fact.shape;

        // Extract num_classes from last dimension (ignore batch dimension)
        let num_classes = if output_shape.len() >= 2 {
            output_shape[output_shape.len() - 1]
                .to_i64()
                .ok()
                .map(|v| v as usize)
        } else {
            None
        };

        Ok(((batch_size, channels, height, width), num_classes))
    }
}

impl OnnxModel {
    /// Internal method that assumes batch size matches model expectations
    fn run_batch_inner(
        &self,
        samples: &[Sample],
        batch_size: usize,
        c: usize,
        h: usize,
        w: usize,
    ) -> Result<Vec<Prediction>> {
        // Convert samples to ndarray tensor: [B, C, H, W] f32.
        let mut input_tensor = Array4::<f32>::zeros((batch_size, c, h, w));

        for (b, sample) in samples.iter().enumerate() {
            let expected_len = h * w * c;
            if sample.data.len() != expected_len {
                return Err(Error::ShapeMismatch {
                    expected: format!("{h}×{w}×{c} = {expected_len}"),
                    actual: sample.data.len().to_string(),
                });
            }

            // Convert HWC u8 → CHW f32 normalized [0, 1]
            for y in 0..h {
                for x in 0..w {
                    for ch in 0..c {
                        let idx = (y * w * c) + (x * c) + ch;
                        let pixel = sample.data[idx] as f32 / 255.0;
                        input_tensor[[b, ch, y, x]] = pixel;
                    }
                }
            }
        }

        // Run inference - convert ndarray → Tensor → TValue
        let input_tensor_dyn = input_tensor.into_dyn();
        let tensor = Tensor::from(input_tensor_dyn);
        let outputs = self
            .model
            .run(tvec![tensor.into()])
            .map_err(|e| Error::InferenceFailed {
                reason: format!("tract run: {e}"),
            })?;

        // Extract output tensor
        let output = outputs.first().ok_or_else(|| Error::InferenceFailed {
            reason: "model produced no outputs".to_string(),
        })?;

        let output_tensor = output
            .to_array_view::<f32>()
            .map_err(|e| Error::InferenceFailed {
                reason: format!("extract output as f32: {e}"),
            })?;

        // Convert to predictions
        // Assume shape [B, num_classes] for classification
        if output_tensor.ndim() != 2 {
            return Err(Error::InferenceFailed {
                reason: format!(
                    "expected 2D output [batch, classes], got shape {:?}",
                    output_tensor.shape()
                ),
            });
        }

        let predictions: Vec<Prediction> = output_tensor
            .axis_iter(Axis(0))
            .map(|row| {
                let logits: Vec<f32> = row.iter().copied().collect();
                Prediction::from_logits(logits)
            })
            .collect();

        if predictions.len() != batch_size {
            return Err(Error::InferenceFailed {
                reason: format!(
                    "output batch size mismatch: expected {}, got {}",
                    batch_size,
                    predictions.len()
                ),
            });
        }

        Ok(predictions)
    }
}

impl InferenceModel for OnnxModel {
    fn load(config: InferenceConfig) -> Result<Self> {
        Self::init_environment()?;

        if config.device != Device::Cpu {
            tracing::warn!(
                "Tract backend currently supports CPU only; ignoring device {:?}",
                config.device
            );
        }

        tracing::info!(
            model = %config.model_path.display(),
            optimization = ?config.optimization_level,
            "Loading ONNX model via Tract",
        );

        let model = Self::load_and_optimize(&config)?;
        let (input_shape, num_classes) = Self::infer_metadata(&model)?;

        tracing::info!(
            input_shape = ?input_shape,
            num_classes = ?num_classes,
            "ONNX model loaded successfully",
        );

        Ok(Self {
            model,
            config,
            input_shape,
            num_classes,
        })
    }

    fn run_batch(&self, samples: &[Sample]) -> Result<Vec<Prediction>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        let (model_batch_size, c, h, w) = self.input_shape;

        // If the model expects a fixed batch size and we have a different number of samples,
        // process in chunks or pad/truncate as needed.
        if samples.len() != model_batch_size {
            tracing::debug!(
                model_batch_size,
                actual_samples = samples.len(),
                "Batch size mismatch - processing in chunks"
            );

            let mut all_predictions = Vec::with_capacity(samples.len());

            // Process samples in chunks matching the model's batch size
            for chunk in samples.chunks(model_batch_size) {
                let chunk_predictions = if chunk.len() < model_batch_size {
                    // Pad the last chunk by repeating the last sample
                    let mut padded_chunk: Vec<Sample> = chunk.to_vec();
                    let last_sample = chunk.last().unwrap().clone();
                    while padded_chunk.len() < model_batch_size {
                        padded_chunk.push(last_sample.clone());
                    }
                    let padded_predictions =
                        self.run_batch_inner(&padded_chunk, model_batch_size, c, h, w)?;
                    // Only keep the actual predictions, discard padding
                    padded_predictions.into_iter().take(chunk.len()).collect()
                } else {
                    self.run_batch_inner(chunk, model_batch_size, c, h, w)?
                };

                all_predictions.extend(chunk_predictions);
            }

            return Ok(all_predictions);
        }

        // Batch size matches - run directly
        self.run_batch_inner(samples, model_batch_size, c, h, w)
    }

    fn input_shape(&self) -> (usize, usize, usize, usize) {
        self.input_shape
    }

    fn num_classes(&self) -> Option<usize> {
        self.num_classes
    }

    fn config(&self) -> &InferenceConfig {
        &self.config
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_onnx_model_init_environment() {
        // Should not panic on multiple calls.
        OnnxModel::init_environment().unwrap();
        OnnxModel::init_environment().unwrap();
    }

    // Integration tests with real ONNX models are in tests/integration_test.rs
}
