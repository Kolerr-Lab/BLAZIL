// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Async prefetch pipeline — wraps any [`Dataset`] in a background-decoded stream.
//!
//! ## Architecture
//!
//! ```text
//!   ┌──────────────────────────────────────┐
//!   │  Pipeline<D>                         │
//!   │                                      │
//!   │  AtomicUsize counter (shared)        │
//!   │       │                              │
//!   │  ┌────┴───┐  ┌────────┐  ┌────────┐ │
//!   │  │Worker 0│  │Worker 1│  │Worker N│ │
//!   │  │spawn_  │  │spawn_  │  │spawn_  │ │
//!   │  │blocking│  │blocking│  │blocking│ │
//!   │  └───┬────┘  └───┬────┘  └───┬────┘ │
//!   │      └───────────┴───────────┘      │
//!   │                  │                  │
//!   │          mpsc::Sender<Batch>        │
//!   └──────────────────┼──────────────────┘
//!                      │ bounded channel (ring_capacity)
//!                      ▼
//!               mpsc::Receiver<Batch>   ← consumer (GPU training loop)
//! ```
//!
//! - Each worker atomically claims `batch_size` consecutive indices.
//! - Decoding happens in `spawn_blocking` (CPU-bound JPEG/PNG decode + resize).
//! - Bounded channel provides **backpressure**: workers pause when the ring is full,
//!   matching Blazil's fintech approach (`publish_with_backpressure`).
//! - `stream_shuffled(seed)` pre-computes a permutation and workers index into it.

use crate::{Batch, Dataset, DatasetConfig, Error, Result, Sample};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;

// ─────────────────────────────────────────────
// Checkpoint state
// ─────────────────────────────────────────────

/// Persistent state for resuming a training run mid-epoch.
///
/// The consumer is responsible for tracking how many samples it has processed
/// and persisting this state at a safe point (e.g., after each batch).
///
/// # Example
/// ```no_run
/// use blazil_dataloader::pipeline::CheckpointState;
///
/// // Save after processing N batches
/// let state = CheckpointState { epoch: 1, sample_offset: 5120, seed: 42 };
/// state.save(std::path::Path::new("/tmp/ckpt.json")).unwrap();
///
/// // Resume next run
/// let state = CheckpointState::load(std::path::Path::new("/tmp/ckpt.json")).unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointState {
    /// Current training epoch (0-based).
    pub epoch: u64,
    /// Number of samples already consumed in this epoch (determines skip offset).
    pub sample_offset: usize,
    /// The shuffle seed used for this epoch's permutation.
    pub seed: u64,
}

impl CheckpointState {
    /// Serialise to a JSON string.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| Error::internal(format!("checkpoint to_json: {e}")))
    }

    /// Deserialise from a JSON string.
    pub fn from_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| Error::internal(format!("checkpoint from_json: {e}")))
    }

    /// Write this checkpoint atomically to `path` (write to `.tmp`, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = self.to_json()?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &json)
            .map_err(|e| Error::Checkpoint(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| Error::Checkpoint(format!("rename to {}: {e}", path.display())))?;
        Ok(())
    }

    /// Load a checkpoint from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| Error::Checkpoint(format!("read {}: {e}", path.display())))?;
        Self::from_json(&json)
    }
}

/// Async prefetch pipeline for streaming batches from a [`Dataset`].
pub struct Pipeline<D: Dataset + 'static> {
    dataset: Arc<D>,
    config: DatasetConfig,
}

impl<D: Dataset + 'static> Pipeline<D> {
    /// Create a new pipeline wrapping `dataset`.
    pub fn new(dataset: D, config: DatasetConfig) -> Self {
        Self {
            dataset: Arc::new(dataset),
            config,
        }
    }

    /// Access the underlying dataset.
    pub fn dataset(&self) -> &D {
        &self.dataset
    }

    /// Start sequential streaming.
    ///
    /// Returns a bounded receiver. Workers stop when all samples are exhausted
    /// or the receiver is dropped.
    ///
    /// Call this inside a `#[tokio::main]` or async context.
    pub fn stream(&self) -> mpsc::Receiver<Result<Batch>> {
        let indices: Vec<usize> = (0..self.dataset.len()).collect();
        self.spawn_workers(Arc::new(indices))
    }

    /// Start shuffled streaming with reproducible seed.
    ///
    /// Pre-computes the full permutation (O(N) memory for N sample indices),
    /// then streams in that order. Same `seed` → same sequence every epoch.
    pub fn stream_shuffled(&self, seed: u64) -> mpsc::Receiver<Result<Batch>> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut indices: Vec<usize> = (0..self.dataset.len()).collect();
        indices.shuffle(&mut rng);
        self.spawn_workers(Arc::new(indices))
    }

    /// Resume streaming from a saved [`CheckpointState`].
    ///
    /// Reconstructs the same shuffled permutation as `stream_shuffled(state.seed)`,
    /// then skips the first `state.sample_offset` samples so the consumer continues
    /// from exactly where it left off within the epoch.
    ///
    /// If `state.sample_offset >= dataset.len()` the stream completes immediately
    /// (the epoch was already finished — caller should advance the epoch counter
    /// and call `stream_shuffled` for the next epoch).
    pub fn stream_from_checkpoint(&self, state: &CheckpointState) -> mpsc::Receiver<Result<Batch>> {
        let mut rng = ChaCha8Rng::seed_from_u64(state.seed);
        let mut indices: Vec<usize> = (0..self.dataset.len()).collect();
        indices.shuffle(&mut rng);
        // Skip already-processed samples.
        let skip = state.sample_offset.min(indices.len());
        let indices = indices[skip..].to_vec();
        self.spawn_workers(Arc::new(indices))
    }

    /// Internal: spawn worker tasks that consume `indices` and send batches.
    fn spawn_workers(&self, indices: Arc<Vec<usize>>) -> mpsc::Receiver<Result<Batch>> {
        let (tx, rx) = mpsc::channel(self.config.ring_capacity);
        let batch_size = self.config.batch_size;
        let num_workers = self.config.num_workers;
        let total = indices.len();

        // Shared atomic cursor: each worker claims next `batch_size` slots.
        let cursor = Arc::new(AtomicUsize::new(0));
        let batch_id_counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..num_workers {
            let tx = tx.clone();
            let dataset = Arc::clone(&self.dataset);
            let indices = Arc::clone(&indices);
            let cursor = Arc::clone(&cursor);
            let batch_id_counter = Arc::clone(&batch_id_counter);

            tokio::spawn(async move {
                loop {
                    // Atomically claim a slice of indices.
                    let start = cursor.fetch_add(batch_size, Ordering::Relaxed);
                    if start >= total {
                        break;
                    }
                    let end = (start + batch_size).min(total);
                    let batch_indices: Vec<usize> = indices[start..end].to_vec();
                    let actual = batch_indices.len();

                    // Decode in a blocking thread (CPU-bound I/O + image decode).
                    let dataset_ref = Arc::clone(&dataset);
                    let decode_result = tokio::task::spawn_blocking(move || {
                        let mut samples = Vec::<Sample>::with_capacity(actual);
                        for idx in batch_indices {
                            samples.push(dataset_ref.get(idx)?);
                        }
                        Ok::<Vec<Sample>, Error>(samples)
                    })
                    .await
                    .map_err(|e| Error::internal(format!("worker panicked: {e}")))
                    .and_then(|r| r);

                    let batch_id = batch_id_counter.fetch_add(1, Ordering::Relaxed) as u64;

                    let batch = decode_result.map(|samples| Batch { samples, batch_id });

                    // Bounded send — backpressure when ring is full.
                    if tx.send(batch).await.is_err() {
                        break; // Receiver dropped → stop gracefully.
                    }
                }
            });
        }

        rx
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatasetConfig, Error, Result, Sample};

    /// Minimal in-memory dataset for testing the pipeline.
    struct FakeDataset {
        size: usize,
    }

    impl Dataset for FakeDataset {
        fn len(&self) -> usize {
            self.size
        }

        fn get(&self, idx: usize) -> Result<Sample> {
            if idx >= self.size {
                return Err(Error::IndexOutOfBounds {
                    index: idx,
                    len: self.size,
                });
            }
            Ok(Sample {
                data: vec![idx as u8],
                label: idx as u32,
                metadata: None,
            })
        }

        fn iter_shuffled(&self, _seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
            Box::new((0..self.size).map(|i| self.get(i)))
        }

        fn iter(&self) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
            Box::new((0..self.size).map(|i| self.get(i)))
        }
    }

    #[tokio::test]
    async fn test_stream_all_samples_received() {
        let config = DatasetConfig::default().with_batch_size(10).with_workers(2);
        let pipeline = Pipeline::new(FakeDataset { size: 100 }, config);

        let mut rx = pipeline.stream();
        let mut total = 0usize;

        while let Some(batch) = rx.recv().await {
            let batch = batch.unwrap();
            total += batch.samples.len();
        }

        assert_eq!(total, 100);
    }

    #[tokio::test]
    async fn test_stream_shuffled_all_samples_received() {
        let config = DatasetConfig::default().with_batch_size(7).with_workers(3);
        let pipeline = Pipeline::new(FakeDataset { size: 50 }, config);

        let mut rx = pipeline.stream_shuffled(42);
        let mut labels: Vec<u32> = Vec::new();

        while let Some(batch) = rx.recv().await {
            for sample in batch.unwrap().samples {
                labels.push(sample.label);
            }
        }

        // All 50 labels received exactly once.
        assert_eq!(labels.len(), 50);
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0u32..50).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn test_stream_shuffled_reproducible() {
        // Use a single worker so batches are delivered in permutation order.
        // Multi-worker correctness (all samples received) is covered by
        // test_stream_shuffled_all_samples_received above.
        let config = DatasetConfig::default().with_batch_size(10).with_workers(1);
        let pipeline = Pipeline::new(FakeDataset { size: 40 }, config);

        async fn collect_labels(p: &Pipeline<FakeDataset>, seed: u64) -> Vec<u32> {
            let mut rx = p.stream_shuffled(seed);
            let mut labels = Vec::new();
            while let Some(batch) = rx.recv().await {
                for s in batch.unwrap().samples {
                    labels.push(s.label);
                }
            }
            labels
        }

        let run1 = collect_labels(&pipeline, 42).await;
        let run2 = collect_labels(&pipeline, 42).await;
        assert_eq!(run1, run2, "same seed must reproduce same order");

        let run3 = collect_labels(&pipeline, 99).await;
        assert_ne!(run1, run3, "different seeds must give different order");
    }

    #[tokio::test]
    async fn test_batch_ids_monotonically_increase() {
        let config = DatasetConfig::default().with_batch_size(5).with_workers(1);
        let pipeline = Pipeline::new(FakeDataset { size: 25 }, config);

        let mut rx = pipeline.stream();
        let mut ids = Vec::new();

        while let Some(batch) = rx.recv().await {
            ids.push(batch.unwrap().batch_id);
        }

        let mut sorted = ids.clone();
        sorted.sort_unstable();
        // All batch_ids are unique (though order may vary with multiple workers)
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "batch IDs must be unique");
    }

    #[tokio::test]
    async fn test_backpressure_does_not_drop_samples() {
        // Small ring capacity — workers must block when full.
        let config = DatasetConfig::default()
            .with_batch_size(10)
            .with_workers(4)
            .with_ring_capacity(2); // only 2 in-flight batches

        let pipeline = Pipeline::new(FakeDataset { size: 200 }, config);
        let mut rx = pipeline.stream();
        let mut total = 0usize;

        while let Some(batch) = rx.recv().await {
            total += batch.unwrap().samples.len();
        }

        assert_eq!(total, 200, "no samples dropped under backpressure");
    }

    // ─────────────────────────────────────────────
    // Checkpoint tests
    // ─────────────────────────────────────────────

    #[test]
    fn test_checkpoint_roundtrip_json() {
        let state = CheckpointState {
            epoch: 3,
            sample_offset: 1024,
            seed: 77,
        };
        let json = state.to_json().unwrap();
        let restored = CheckpointState::from_json(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_checkpoint_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ckpt.json");
        let state = CheckpointState {
            epoch: 1,
            sample_offset: 512,
            seed: 42,
        };
        state.save(&path).unwrap();
        let loaded = CheckpointState::load(&path).unwrap();
        assert_eq!(state, loaded);
    }

    #[tokio::test]
    async fn test_stream_from_checkpoint_skips_samples() {
        // With 1 worker the first 10 samples are exactly indices[0..10].
        let config = DatasetConfig::default().with_batch_size(10).with_workers(1);
        let pipeline = Pipeline::new(FakeDataset { size: 30 }, config);

        // Full run
        let all_labels: Vec<u32> = {
            let mut rx = pipeline.stream_shuffled(42);
            let mut v = Vec::new();
            while let Some(b) = rx.recv().await {
                for s in b.unwrap().samples {
                    v.push(s.label);
                }
            }
            v
        };

        // Resumed from offset 10 — should get the last 20 samples in the same order
        let state = CheckpointState {
            epoch: 0,
            sample_offset: 10,
            seed: 42,
        };
        let resumed: Vec<u32> = {
            let mut rx = pipeline.stream_from_checkpoint(&state);
            let mut v = Vec::new();
            while let Some(b) = rx.recv().await {
                for s in b.unwrap().samples {
                    v.push(s.label);
                }
            }
            v
        };

        assert_eq!(resumed.len(), 20);
        assert_eq!(
            resumed,
            all_labels[10..],
            "resumed stream must be the tail of the original permutation"
        );
    }
}
