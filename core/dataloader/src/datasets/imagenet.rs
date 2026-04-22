// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! ImageNet ILSVRC 2012 dataset implementation.
//!
//! **Expected directory format:**
//! ```text
//! <root>/
//!   train/
//!     n01440764/           ← synset folder = class label
//!       n01440764_10026.JPEG
//!       ...
//!     n01443537/
//!       ...
//!   val/                   ← optional validation split
//!     n01440764/
//!       ...
//! ```
//! Class indices are assigned by **alphabetical sort** of synset folder names,
//! producing a deterministic 0-based integer label for each class.

use crate::{Dataset, DatasetConfig, Error, Result, Sample};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::path::{Path, PathBuf};

/// File extensions recognised as images (case-insensitive).
const IMAGE_EXTENSIONS: &[&str] = &["jpeg", "jpg", "png", "webp"];

/// Standard ImageNet input size after resize.
const INPUT_SIZE: u32 = 224;

/// ImageNet ILSVRC 2012 dataset.
///
/// Loads the dataset index at construction time (directory scan).
/// Each `get()` call reads + decodes one image synchronously.
/// Use [`crate::Pipeline`] to prefetch batches in background threads.
#[derive(Debug)]
pub struct ImageNetDataset {
    config: DatasetConfig,
    /// Flat list of (absolute_path, class_index) pairs.
    /// Class index = alphabetical rank of synset folder name (0-based).
    entries: Vec<(PathBuf, u32)>,
    /// Total number of distinct classes found.
    num_classes: usize,
}

impl ImageNetDataset {
    /// Open an ImageNet split from `root`.
    ///
    /// `root` can be:
    /// - A directory that contains `train/` → loads train split.
    /// - A directory that contains synset folders directly → loads as-is.
    pub fn open(root: impl AsRef<Path>, config: DatasetConfig) -> Result<Self> {
        config.validate()?;
        let root = root.as_ref();

        if !root.exists() {
            return Err(Error::DatasetNotFound {
                path: root.to_path_buf(),
            });
        }

        // Resolve the split directory.
        let split_dir = if root.join("train").is_dir() {
            root.join("train")
        } else {
            root.to_path_buf()
        };

        let (entries, num_classes) = Self::scan_split_dir(&split_dir)?;

        if entries.is_empty() {
            return Err(Error::InvalidFormat {
                reason: format!(
                    "No images found under {} — expected synset subfolders with JPEG/PNG files",
                    split_dir.display()
                ),
            });
        }

        tracing::info!(
            split = %split_dir.display(),
            total_samples = entries.len(),
            num_classes = num_classes,
            "ImageNet dataset loaded",
        );

        Ok(Self {
            config,
            entries,
            num_classes,
        })
    }

    /// Returns the number of distinct classes.
    pub fn num_classes(&self) -> usize {
        self.num_classes
    }

    /// Returns dataset config.
    pub fn config(&self) -> &DatasetConfig {
        &self.config
    }

    /// Build a deterministically ordered index of (path, label) pairs.
    ///
    /// Synsets are sorted alphabetically so the mapping is reproducible
    /// across runs without an external label file.
    fn scan_split_dir(split_dir: &Path) -> Result<(Vec<(PathBuf, u32)>, usize)> {
        let mut synsets: Vec<PathBuf> = std::fs::read_dir(split_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();

        if synsets.is_empty() {
            return Err(Error::InvalidFormat {
                reason: format!("No synset subdirectories in {}", split_dir.display()),
            });
        }

        synsets.sort(); // alphabetical → deterministic class indices

        let mut entries = Vec::new();
        for (class_idx, synset_dir) in synsets.iter().enumerate() {
            let label = class_idx as u32;
            let mut images: Vec<PathBuf> = std::fs::read_dir(synset_dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && Self::is_image(p))
                .collect();
            images.sort(); // deterministic ordering within class
            for path in images {
                entries.push((path, label));
            }
        }

        let num_classes = synsets.len();
        Ok((entries, num_classes))
    }

    fn is_image(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                IMAGE_EXTENSIONS
                    .iter()
                    .any(|&v| v.eq_ignore_ascii_case(ext))
            })
            .unwrap_or(false)
    }

    /// Decode one sample: read → JPEG/PNG decode → resize to 224×224 RGB.
    fn decode_at(&self, idx: usize) -> Result<Sample> {
        let (path, label) = &self.entries[idx];

        // Read raw file bytes.
        let bytes = std::fs::read(path).map_err(|e| Error::CorruptedSample {
            index: idx,
            reason: format!("read '{}' failed: {e}", path.display()),
        })?;

        // Decode image (supports JPEG, PNG, WebP).
        let img = image::load_from_memory(&bytes).map_err(|e| Error::CorruptedSample {
            index: idx,
            reason: format!("decode '{}' failed: {e}", path.display()),
        })?;

        // Resize to INPUT_SIZE × INPUT_SIZE (standard ImageNet preprocessing).
        // image 0.25 uses Triangle (bilinear) as the quality/speed default.
        let img = img.resize_exact(INPUT_SIZE, INPUT_SIZE, image::imageops::FilterType::Triangle);

        // Convert to packed RGB bytes: H×W×C = 224×224×3 = 150,528 bytes.
        let data = img.into_rgb8().into_raw();

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| serde_json::json!({ "filename": s }));

        Ok(Sample {
            data,
            label: *label,
            metadata: filename,
        })
    }

    /// Return a shuffled index permutation for one epoch.
    /// Deterministic for the same seed; different seeds give different orders.
    pub fn shuffled_indices(&self, seed: u64) -> Vec<usize> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut indices: Vec<usize> = (0..self.entries.len()).collect();
        indices.shuffle(&mut rng);
        indices
    }
}

impl Dataset for ImageNetDataset {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.len() {
            return Err(Error::IndexOutOfBounds {
                index: idx,
                len: self.len(),
            });
        }
        self.decode_at(idx)
    }

    fn iter_shuffled(&self, seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        let indices = self.shuffled_indices(seed);
        Box::new(indices.into_iter().map(move |i| self.decode_at(i)))
    }

    fn iter(&self) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        Box::new((0..self.len()).map(move |i| self.decode_at(i)))
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use tempfile::TempDir;

    /// Create a tiny synthetic dataset on disk.
    fn make_test_dataset(dir: &TempDir, num_classes: usize, images_per_class: usize) -> PathBuf {
        let root = dir.path().to_path_buf();
        let train = root.join("train");

        for class in 0..num_classes {
            let synset = format!("n{:08}", class);
            let class_dir = train.join(&synset);
            std::fs::create_dir_all(&class_dir).unwrap();

            for img_idx in 0..images_per_class {
                let img_path = class_dir.join(format!("{synset}_{img_idx:06}.png"));
                // 8×8 solid-colour PNG — tiny, decodes fast.
                let colour = [(class as u8).wrapping_mul(30), 128, 64];
                let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
                    ImageBuffer::from_fn(8, 8, |_, _| Rgb(colour));
                img.save(&img_path).unwrap();
            }
        }

        root
    }

    #[test]
    fn test_open_dataset() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 3, 5);
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();
        assert_eq!(ds.len(), 15); // 3 classes × 5 images
        assert_eq!(ds.num_classes(), 3);
    }

    #[test]
    fn test_get_decodes_to_224x224_rgb() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 2, 3);
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();

        let sample = ds.get(0).unwrap();
        // 224 × 224 × 3 RGB bytes
        assert_eq!(sample.data.len(), (224 * 224 * 3) as usize);
        assert_eq!(sample.label, 0);
        assert!(sample.metadata.is_some());
    }

    #[test]
    fn test_class_ordering_is_alphabetical() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 4, 1); // 1 image per class
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();

        // 4 classes in alphabetical synset order → labels 0,1,2,3
        let labels: Vec<u32> = (0..ds.len()).map(|i| ds.get(i).unwrap().label).collect();
        assert_eq!(labels, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_shuffled_indices_reproducible() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 5, 10);
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();

        let a = ds.shuffled_indices(42);
        let b = ds.shuffled_indices(42);
        assert_eq!(a, b, "same seed must produce same order");

        let c = ds.shuffled_indices(99);
        assert_ne!(a, c, "different seeds must produce different orders");
    }

    #[test]
    fn test_iter_visits_all_samples() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 3, 4);
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();

        let count = ds.iter().count();
        assert_eq!(count, 12);
    }

    #[test]
    fn test_iter_shuffled_visits_all_samples() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 3, 4);
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();

        let count = ds.iter_shuffled(42).count();
        assert_eq!(count, 12);
    }

    #[test]
    fn test_dataset_not_found() {
        let result = ImageNetDataset::open("/nonexistent/path", DatasetConfig::default());
        assert!(matches!(result.unwrap_err(), Error::DatasetNotFound { .. }));
    }

    #[test]
    fn test_index_out_of_bounds() {
        let dir = TempDir::new().unwrap();
        let root = make_test_dataset(&dir, 2, 3);
        let ds = ImageNetDataset::open(&root, DatasetConfig::default()).unwrap();

        let result = ds.get(999);
        assert!(matches!(result.unwrap_err(), Error::IndexOutOfBounds { .. }));
    }

    #[test]
    fn test_empty_split_is_error() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        // train/ exists but has no synset subdirectories
        std::fs::create_dir_all(root.join("train")).unwrap();

        let result = ImageNetDataset::open(&root, DatasetConfig::default());
        assert!(result.is_err());
    }
}
