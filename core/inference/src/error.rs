// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Error types for the inference engine.

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for inference operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Inference engine errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Model file not found or inaccessible.
    #[error("model not found: {path}")]
    ModelNotFound { path: PathBuf },

    /// Failed to load or initialize the model.
    #[error("model load failed: {reason}")]
    ModelLoadFailed { reason: String },

    /// Invalid model format or corrupted file.
    #[error("invalid model format: {reason}")]
    InvalidModelFormat { reason: String },

    /// Inference execution failed.
    #[error("inference failed: {reason}")]
    InferenceFailed { reason: String },

    /// Input tensor shape mismatch.
    #[error("input shape mismatch: expected {expected}, got {actual}")]
    ShapeMismatch { expected: String, actual: String },

    /// Unsupported device or execution provider.
    #[error("unsupported device: {device}")]
    UnsupportedDevice { device: String },

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// I/O error during model loading or output writing.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Dataloader error propagated from upstream.
    #[error("dataloader error: {0}")]
    Dataloader(#[from] blazil_dataloader::Error),

    /// Internal error (bugs or unexpected states).
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Construct an internal error from any displayable message.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// Construct a config error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }
}
