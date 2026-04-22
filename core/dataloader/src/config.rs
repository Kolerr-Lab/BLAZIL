// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Configuration for dataset loading and streaming.

use serde::{Deserialize, Serialize};

/// Configuration for dataset loading behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetConfig {
    /// Batch size for grouping samples.
    pub batch_size: usize,

    /// Enable shuffling (with reproducible seed).
    pub shuffle: bool,

    /// RNG seed for reproducible shuffles (ignored if shuffle=false).
    pub seed: u64,

    /// Number of worker threads for parallel I/O.
    pub num_workers: usize,

    /// Ring buffer capacity (in-flight batches).
    pub ring_capacity: usize,

    /// Enable prefetching for next batch.
    pub prefetch: bool,

    /// Shard ID for multi-GPU setups (0-based).
    pub shard_id: Option<usize>,

    /// Total number of shards (for multi-GPU).
    pub num_shards: usize,
}

impl Default for DatasetConfig {
    fn default() -> Self {
        Self {
            batch_size: 256,
            shuffle: false,
            seed: 0,
            num_workers: 4,
            ring_capacity: 8_192,
            prefetch: true,
            shard_id: None,
            num_shards: 1,
        }
    }
}

impl DatasetConfig {
    /// Create a new config with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Enable shuffling with given seed.
    pub fn with_shuffle(mut self, enable: bool) -> Self {
        self.shuffle = enable;
        self
    }

    /// Set RNG seed.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Set number of worker threads.
    pub fn with_workers(mut self, num: usize) -> Self {
        self.num_workers = num;
        self
    }

    /// Set ring buffer capacity.
    pub fn with_ring_capacity(mut self, capacity: usize) -> Self {
        self.ring_capacity = capacity;
        self
    }

    /// Configure for multi-GPU sharding.
    pub fn with_shard(mut self, shard_id: usize, num_shards: usize) -> Self {
        self.shard_id = Some(shard_id);
        self.num_shards = num_shards;
        self
    }

    /// Validate configuration.
    pub fn validate(&self) -> crate::Result<()> {
        if self.batch_size == 0 {
            return Err(crate::Error::config("batch_size must be > 0"));
        }
        if self.num_workers == 0 {
            return Err(crate::Error::config("num_workers must be > 0"));
        }
        if self.ring_capacity < self.batch_size {
            return Err(crate::Error::config("ring_capacity must be >= batch_size"));
        }
        if self.num_shards == 0 {
            return Err(crate::Error::config("num_shards must be > 0"));
        }
        if let Some(shard_id) = self.shard_id {
            if shard_id >= self.num_shards {
                return Err(crate::Error::config(format!(
                    "shard_id ({}) must be < num_shards ({})",
                    shard_id, self.num_shards
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DatasetConfig::default();
        assert_eq!(config.batch_size, 256);
        assert!(!config.shuffle);
        assert_eq!(config.num_workers, 4);
    }

    #[test]
    fn test_builder_pattern() {
        let config = DatasetConfig::new()
            .with_batch_size(512)
            .with_shuffle(true)
            .with_seed(42)
            .with_shard(0, 4);

        assert_eq!(config.batch_size, 512);
        assert!(config.shuffle);
        assert_eq!(config.seed, 42);
        assert_eq!(config.shard_id, Some(0));
        assert_eq!(config.num_shards, 4);
    }

    #[test]
    fn test_validation() {
        // Valid config
        let config = DatasetConfig::default();
        assert!(config.validate().is_ok());

        // Invalid: batch_size = 0
        let config = DatasetConfig::new().with_batch_size(0);
        assert!(config.validate().is_err());

        // Invalid: shard_id >= num_shards
        let config = DatasetConfig::new().with_shard(4, 4);
        assert!(config.validate().is_err());

        // Invalid: ring_capacity < batch_size
        let config = DatasetConfig::new()
            .with_batch_size(1000)
            .with_ring_capacity(500);
        assert!(config.validate().is_err());
    }
}
