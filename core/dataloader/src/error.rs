// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Error types for Blazil Dataloader.

use std::path::PathBuf;

/// Result type alias for dataloader operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Comprehensive error type for all dataloader operations.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Dataset not found at path: {path}")]
    DatasetNotFound { path: PathBuf },

    #[error("Invalid dataset format: {reason}")]
    InvalidFormat { reason: String },

    #[error("Index out of bounds: {index} >= {len}")]
    IndexOutOfBounds { index: usize, len: usize },

    #[error("Corrupted sample at index {index}: {reason}")]
    CorruptedSample { index: usize, reason: String },

    #[error("Image decode error: {0}")]
    ImageDecode(#[from] image::ImageError),

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    // TODO: Re-enable when arrow is added back
    // #[error("Parquet error: {0}")]
    // Parquet(#[from] parquet::errors::ParquetError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Backpressure timeout: ring buffer full for {duration_ms}ms")]
    BackpressureTimeout { duration_ms: u64 },

    // TODO: Add CUDA support
    // #[cfg(feature = "cuda")]
    // #[error("CUDA error: {0}")]
    // Cuda(String),

    #[error("Checkpoint error: {0}")]
    Checkpoint(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Create a config error with custom message.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Create an internal error with custom message.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = Error::IndexOutOfBounds {
            index: 100,
            len: 50,
        };
        assert_eq!(err.to_string(), "Index out of bounds: 100 >= 50");
    }

    #[test]
    fn test_config_error() {
        let err = Error::config("Invalid batch size");
        assert!(matches!(err, Error::Config(_)));
    }
}
