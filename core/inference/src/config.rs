// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Configuration for inference execution.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Device type for inference execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Device {
    /// CPU execution.
    #[default]
    Cpu,
    /// CUDA GPU execution (requires CUDA runtime).
    Cuda,
    /// TensorRT optimized execution (requires TensorRT SDK).
    TensorRT,
}

/// Configuration for model inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    /// Path to the ONNX model file.
    pub model_path: PathBuf,

    /// Target device for execution.
    pub device: Device,

    /// Batch size for inference (must match model input or use dynamic axes).
    pub batch_size: usize,

    /// Number of intra-op threads (CPU only, 0 = auto).
    pub intra_threads: usize,

    /// Number of inter-op threads (CPU only, 0 = auto).
    pub inter_threads: usize,

    /// Enable graph optimizations (ONNX Runtime).
    pub optimization_level: OptimizationLevel,

    /// CUDA device ID (0-based, ignored for CPU).
    pub cuda_device_id: i32,
}

/// ONNX Runtime graph optimization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptimizationLevel {
    /// Disable all optimizations.
    Disable,
    /// Basic optimizations (constant folding, redundant node elimination).
    Basic,
    /// Extended optimizations (more aggressive graph transformations).
    Extended,
    /// All optimizations including layout optimizations.
    All,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            device: Device::Cpu,
            batch_size: 1,
            intra_threads: 0, // auto
            inter_threads: 0, // auto
            optimization_level: OptimizationLevel::All,
            cuda_device_id: 0,
        }
    }
}

impl InferenceConfig {
    /// Create a new config for the given model path.
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            ..Default::default()
        }
    }

    /// Set device.
    pub fn with_device(mut self, device: Device) -> Self {
        self.device = device;
        self
    }

    /// Set batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Set CPU thread counts.
    pub fn with_threads(mut self, intra: usize, inter: usize) -> Self {
        self.intra_threads = intra;
        self.inter_threads = inter;
        self
    }

    /// Set optimization level.
    pub fn with_optimization(mut self, level: OptimizationLevel) -> Self {
        self.optimization_level = level;
        self
    }

    /// Set CUDA device ID.
    pub fn with_cuda_device(mut self, device_id: i32) -> Self {
        self.cuda_device_id = device_id;
        self
    }

    /// Validate the config.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.model_path.exists() {
            return Err(crate::Error::ModelNotFound {
                path: self.model_path.clone(),
            });
        }
        if self.batch_size == 0 {
            return Err(crate::Error::config("batch_size must be > 0"));
        }
        Ok(())
    }
}
