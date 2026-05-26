// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Synthetic in-memory dataset for benchmarking without real data on disk.
//!
//! Generates `num_classes` distinct images deterministically at construction
//! time using a seeded LCG mixer, then serves them round-robin.
//!
//! # Properties
//! - **Zero disk I/O**: all data lives in RAM — saturates CPU, not storage.
//! - **Deterministic**: same `seed` + `num_classes` → identical pixel values.
//! - **Scalable**: `num_samples` is virtual; actual RAM usage is
//!   `num_classes × height × width × channels` bytes (≈ 148 MiB for 1 000 classes
//!   at 224 × 224 RGB).
//! - **Pipeline-compatible**: implements [`Dataset`] — drop-in for
//!   [`ImageNetDataset`] in any [`crate::Pipeline`].
//!
//! # Example
//! ```
//! use blazil_dataloader::datasets::SyntheticDataset;
//! use blazil_dataloader::Dataset;
//!
//! // 100 000 virtual 224×224 RGB samples, 1 000 classes, seed 42
//! let ds = SyntheticDataset::new(100_000, 1_000, 224, 224, 3, 42);
//! assert_eq!(ds.len(), 100_000);
//! assert_eq!(ds.num_classes(), 1_000);
//!
//! let sample = ds.get(0).unwrap();
//! assert_eq!(sample.data.len(), 224 * 224 * 3);
//! assert_eq!(sample.label, 0);
//! ```

use crate::{Dataset, Error, Result, Sample};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::sync::Arc;

/// Synthetic in-memory dataset for CPU/AI benchmarking.
///
/// All pixel buffers are pre-computed at construction and shared via [`Arc`].
/// `get()` is effectively O(1): a single `Arc::clone` + `Vec::clone` of a
/// fixed-size slice.
pub struct SyntheticDataset {
    /// Pre-generated pixel buffers — one per class.
    /// Each buffer is `height × width × channels` bytes.
    class_images: Arc<Vec<Vec<u8>>>,
    /// Total virtual samples (defines `len()`; wraps around `num_classes`).
    num_samples: usize,
    /// Number of distinct synthetic classes.
    num_classes: usize,
    /// Image height in pixels.
    height: usize,
    /// Image width in pixels.
    width: usize,
    /// Number of channels (3 for RGB, 1 for greyscale).
    channels: usize,
}

impl std::fmt::Debug for SyntheticDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntheticDataset")
            .field("num_samples", &self.num_samples)
            .field("num_classes", &self.num_classes)
            .field("shape", &(self.height, self.width, self.channels))
            .finish()
    }
}

impl SyntheticDataset {
    /// Create a new synthetic dataset.
    ///
    /// # Arguments
    /// - `num_samples`  – virtual dataset length (wraps around `num_classes`)
    /// - `num_classes`  – number of distinct class labels (and pixel buffers)
    /// - `height`       – image height in pixels
    /// - `width`        – image width in pixels
    /// - `channels`     – number of channels (e.g. 3 for RGB)
    /// - `seed`         – RNG seed for reproducible pixel generation
    ///
    /// # Panics
    /// Panics if `num_classes == 0` or `height * width * channels == 0`.
    pub fn new(
        num_samples: usize,
        num_classes: usize,
        height: usize,
        width: usize,
        channels: usize,
        seed: u64,
    ) -> Self {
        assert!(num_classes > 0, "num_classes must be > 0");
        assert!(
            height * width * channels > 0,
            "height, width and channels must all be > 0"
        );

        let pixel_len = height * width * channels;

        // Pre-generate one image per class using a deterministic mixer.
        // Using splitmix64 / xorshift mixing — no heap allocation beyond the
        // final Vec<u8>, and no per-pixel RNG object overhead.
        let class_images: Vec<Vec<u8>> = (0..num_classes)
            .map(|c| {
                // Mix seed with class index to produce a unique per-class stream.
                let mut state = seed
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    .wrapping_add((c as u64).wrapping_mul(0x6c62_272e_07bb_0142));

                let mut buf = Vec::with_capacity(pixel_len);
                for i in 0..pixel_len {
                    // splitmix64 step — fast, high-quality mixing.
                    state = state
                        .wrapping_add((i as u64).wrapping_mul(0x517c_c1b7_2722_0a95))
                        .wrapping_add(0x9e37_79b9_7f4a_7c15);
                    let mut z = state;
                    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                    z ^= z >> 31;
                    buf.push((z & 0xff) as u8);
                }
                buf
            })
            .collect();

        Self {
            class_images: Arc::new(class_images),
            num_samples,
            num_classes,
            height,
            width,
            channels,
        }
    }

    /// Number of distinct classes.
    pub fn num_classes(&self) -> usize {
        self.num_classes
    }

    /// Image shape as `(height, width, channels)`.
    pub fn input_shape(&self) -> (usize, usize, usize) {
        (self.height, self.width, self.channels)
    }
}

impl Dataset for SyntheticDataset {
    fn len(&self) -> usize {
        self.num_samples
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.num_samples {
            return Err(Error::IndexOutOfBounds {
                index: idx,
                len: self.num_samples,
            });
        }
        let class_id = idx % self.num_classes;
        Ok(Sample {
            data: self.class_images[class_id].clone(),
            label: class_id as u32,
            metadata: None,
        })
    }

    fn iter_shuffled(&self, seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut indices: Vec<usize> = (0..self.num_samples).collect();
        indices.shuffle(&mut rng);
        Box::new(indices.into_iter().map(|i| self.get(i)))
    }

    fn iter(&self) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        Box::new((0..self.num_samples).map(|i| self.get(i)))
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const HEIGHT: usize = 224;
    const WIDTH: usize = 224;
    const CHANNELS: usize = 3;
    const PIXEL_LEN: usize = HEIGHT * WIDTH * CHANNELS;

    fn make_ds(num_samples: usize) -> SyntheticDataset {
        SyntheticDataset::new(num_samples, 1_000, HEIGHT, WIDTH, CHANNELS, 42)
    }

    #[test]
    fn test_len() {
        let ds = make_ds(100_000);
        assert_eq!(ds.len(), 100_000);
        assert!(!ds.is_empty());
    }

    #[test]
    fn test_get_basic() {
        let ds = make_ds(100_000);
        let s = ds.get(0).unwrap();
        assert_eq!(s.data.len(), PIXEL_LEN);
        assert_eq!(s.label, 0);
    }

    #[test]
    fn test_label_wraps() {
        let ds = make_ds(100_000);
        // sample at index num_classes should have label 0 again
        let s = ds.get(ds.num_classes()).unwrap();
        assert_eq!(s.label, 0);
    }

    #[test]
    fn test_out_of_bounds() {
        let ds = make_ds(10);
        assert!(ds.get(10).is_err());
    }

    #[test]
    fn test_deterministic_same_seed() {
        let ds1 = SyntheticDataset::new(100, 10, 4, 4, 3, 99);
        let ds2 = SyntheticDataset::new(100, 10, 4, 4, 3, 99);
        let s1 = ds1.get(7).unwrap();
        let s2 = ds2.get(7).unwrap();
        assert_eq!(s1.data, s2.data);
    }

    #[test]
    fn test_different_seeds_differ() {
        let ds1 = SyntheticDataset::new(100, 10, 4, 4, 3, 1);
        let ds2 = SyntheticDataset::new(100, 10, 4, 4, 3, 2);
        let s1 = ds1.get(0).unwrap();
        let s2 = ds2.get(0).unwrap();
        assert_ne!(s1.data, s2.data);
    }

    #[test]
    fn test_different_classes_differ() {
        let ds = SyntheticDataset::new(100, 10, 4, 4, 3, 42);
        let s0 = ds.get(0).unwrap();
        let s1 = ds.get(1).unwrap();
        assert_ne!(s0.data, s1.data);
    }

    #[test]
    fn test_iter_count() {
        let ds = make_ds(500);
        assert_eq!(ds.iter().count(), 500);
    }

    #[test]
    fn test_iter_shuffled_count() {
        let ds = make_ds(500);
        assert_eq!(ds.iter_shuffled(42).count(), 500);
    }

    #[test]
    fn test_num_classes() {
        let ds = SyntheticDataset::new(1_000, 1_000, 224, 224, 3, 42);
        assert_eq!(ds.num_classes(), 1_000);
    }
}
