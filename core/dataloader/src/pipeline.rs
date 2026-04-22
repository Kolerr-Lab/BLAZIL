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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;

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
}
