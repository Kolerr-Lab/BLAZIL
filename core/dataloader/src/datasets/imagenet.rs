// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! ImageNet dataset implementation.
//!
//! **Format:**
//! ```text
//! /data/imagenet/
//!   train/
//!     n01440764/
//!       n01440764_10026.JPEG
//!       ...
//!   val/
//!     ILSVRC2012_val_00000001.JPEG
//!     ...
//!   labels.txt
//! ```

use crate::{Dataset, DatasetConfig, Result, Sample};
use std::path::{Path, PathBuf};

/// ImageNet dataset (ILSVRC 2012).
#[allow(dead_code)] // TODO: Remove when fully implemented
pub struct ImageNetDataset {
    root: PathBuf,
    config: DatasetConfig,
    #[allow(dead_code)]
    image_paths: Vec<PathBuf>,
    #[allow(dead_code)]
    labels: Vec<u32>,
}

impl ImageNetDataset {
    /// Open ImageNet dataset from directory.
    pub fn open(root: impl AsRef<Path>, config: DatasetConfig) -> Result<Self> {
        let root = root.as_ref().to_path_buf();

        // Validate config
        config.validate()?;

        // Validate dataset exists
        if !root.exists() {
            return Err(crate::Error::DatasetNotFound { path: root });
        }

        // TODO: Load image paths and labels
        // For now, return empty dataset
        Ok(Self {
            root,
            config,
            image_paths: Vec::new(),
            labels: Vec::new(),
        })
    }
}

impl Dataset for ImageNetDataset {
    fn len(&self) -> usize {
        self.image_paths.len()
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.len() {
            return Err(crate::Error::IndexOutOfBounds {
                index: idx,
                len: self.len(),
            });
        }

        // TODO: Implement actual image loading
        Ok(Sample {
            data: Vec::new(),
            label: self.labels[idx],
            metadata: None,
        })
    }

    fn iter_shuffled(&self, _seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        // TODO: Implement shuffled iterator
        Box::new(std::iter::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dataset_not_found() {
        let config = DatasetConfig::default();
        let result = ImageNetDataset::open("/nonexistent/path", config);
        assert!(result.is_err());
    }
}
