// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Blazil Dataloader — High-performance dataset streaming for AI/ML workloads.
//!
//! **Design Goals:**
//! - Zero-copy I/O via io_uring and memory-mapped files
//! - Backpressure-aware streaming (never drop samples)
//! - Reproducible shuffling with seeded RNG
//! - Multi-GPU sharding support
//! - VSR-based fault-tolerant checkpointing
//!
//! **Throughput Target:** 10M+ samples/sec on 8× GPU setup
//!
//! # Example
//! ```no_run
//! use blazil_dataloader::datasets::ImageNetDataset;
//! use blazil_dataloader::{DatasetConfig, Dataset};
//!
//! let config = DatasetConfig::default()
//!     .with_batch_size(256)
//!     .with_shuffle(true)
//!     .with_seed(42);
//!
//! let dataset = ImageNetDataset::open("/data/imagenet", config)?;
//! for sample in dataset.iter() {
//!     let sample = sample?;
//!     // Process sample...
//! }
//! # Ok::<(), blazil_dataloader::Error>(())
//! ```

pub mod config;
pub mod datasets;
pub mod error;
pub mod pipeline;
pub mod readers;
pub mod transform;

// TODO: Add CUDA IPC support
// #[cfg(feature = "cuda")]
// pub mod ipc;

pub use config::DatasetConfig;
pub use error::{Error, Result};
pub use pipeline::Pipeline;
pub use transform::{Transform, TransformChain};

/// Sample represents a single training example.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Raw data (e.g., image bytes, embeddings)
    pub data: Vec<u8>,
    /// Label or target value
    pub label: u32,
    /// Optional metadata (e.g., filename, timestamp)
    pub metadata: Option<serde_json::Value>,
}

/// Batch represents a collection of samples for efficient processing.
#[derive(Debug)]
pub struct Batch {
    pub samples: Vec<Sample>,
    pub batch_id: u64,
}

/// Dataset trait — common interface for all dataset types.
pub trait Dataset: Send + Sync {
    /// Total number of samples in the dataset.
    fn len(&self) -> usize;

    /// Returns true if dataset is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get a single sample by index.
    fn get(&self, idx: usize) -> Result<Sample>;

    /// Create a shuffled iterator with given seed.
    fn iter_shuffled(&self, seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_>;

    /// Create a sequential iterator.
    fn iter(&self) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        self.iter_shuffled(0) // seed=0 = no shuffle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_creation() {
        let sample = Sample {
            data: vec![1, 2, 3],
            label: 5,
            metadata: None,
        };
        assert_eq!(sample.label, 5);
        assert_eq!(sample.data.len(), 3);
    }
}
