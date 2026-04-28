// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Object detection dataset (COCO format and YOLO format).
//!
//! **Use cases:**
//! - Object detection (YOLO, Faster R-CNN)
//! - Instance segmentation (Mask R-CNN)
//! - Keypoint detection
//! - Document analysis (bounding boxes)
//!
//! **Expected formats:**
//! - COCO JSON: annotations with bounding boxes
//! - YOLO format: image + txt file with normalized bbox coordinates
//! - Pascal VOC XML: annotations in XML format
//!
//! **Output:**
//! - `Sample.data`: Image bytes (same as ImageNet)
//! - `Sample.label`: Primary class (first bbox class)
//! - `Sample.metadata`: Full bounding box annotations as JSON

use crate::{
    readers::{FileReader, MmapReader},
    Dataset, DatasetConfig, Error, Result, Sample,
};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(target_os = "linux")]
use crate::readers::IoUringReader;

/// Standard input size for detection models (YOLO default).
const INPUT_SIZE: u32 = 640;

/// Bounding box in different coordinate formats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Class label
    pub class_id: u32,
    /// Coordinates (format depends on coordinate_format)
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Optional confidence score (for predictions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

impl BoundingBox {
    /// Convert from YOLO format (x_center, y_center, width, height, all normalized 0-1)
    /// to absolute pixel coordinates.
    pub fn from_yolo_normalized(
        class_id: u32,
        x_center_norm: f32,
        y_center_norm: f32,
        width_norm: f32,
        height_norm: f32,
        img_width: u32,
        img_height: u32,
    ) -> Self {
        let x = x_center_norm * img_width as f32;
        let y = y_center_norm * img_height as f32;
        let width = width_norm * img_width as f32;
        let height = height_norm * img_height as f32;

        Self {
            class_id,
            x,
            y,
            width,
            height,
            confidence: None,
        }
    }

    /// Convert to YOLO normalized format.
    pub fn to_yolo_normalized(&self, img_width: u32, img_height: u32) -> (f32, f32, f32, f32) {
        let x_center = self.x / img_width as f32;
        let y_center = self.y / img_height as f32;
        let width = self.width / img_width as f32;
        let height = self.height / img_height as f32;
        (x_center, y_center, width, height)
    }

    /// Convert from COCO format (x_min, y_min, width, height in pixels)
    pub fn from_coco(class_id: u32, x_min: f32, y_min: f32, width: f32, height: f32) -> Self {
        Self {
            class_id,
            x: x_min + width / 2.0, // Convert to center
            y: y_min + height / 2.0,
            width,
            height,
            confidence: None,
        }
    }
}

/// Annotation for one image (multiple bounding boxes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnnotation {
    pub image_path: PathBuf,
    pub bboxes: Vec<BoundingBox>,
}

/// Object detection dataset.
///
/// Supports multiple annotation formats (YOLO, COCO, VOC).
/// For YOLO format, each image has a corresponding .txt file with bboxes.
pub struct DetectionDataset {
    config: DatasetConfig,
    /// List of image annotations for this shard
    annotations: Vec<ImageAnnotation>,
    /// Number of classes
    num_classes: usize,
    /// Class names (optional)
    class_names: Option<Vec<String>>,
    /// File reader (io_uring on Linux, mmap elsewhere)
    reader: Arc<dyn FileReader>,
}

impl std::fmt::Debug for DetectionDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetectionDataset")
            .field("num_annotations", &self.annotations.len())
            .field("num_classes", &self.num_classes)
            .field("has_class_names", &self.class_names.is_some())
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DetectionDataset {
    /// Load detection dataset in YOLO format.
    ///
    /// **Directory structure:**
    /// ```text
    /// <root>/
    ///   images/
    ///     img1.jpg
    ///     img2.jpg
    ///   labels/
    ///     img1.txt
    ///     img2.txt
    ///   classes.txt (optional)
    /// ```
    ///
    /// **Label file format (YOLO):**
    /// ```text
    /// class_id x_center y_center width height
    /// 0 0.5 0.5 0.2 0.3
    /// 1 0.7 0.8 0.1 0.15
    /// ```
    /// All coordinates are normalized to [0, 1].
    pub fn from_yolo_directory(root: impl AsRef<Path>, config: DatasetConfig) -> Result<Self> {
        config.validate()?;
        let root = root.as_ref();

        if !root.exists() {
            return Err(Error::DatasetNotFound {
                path: root.to_path_buf(),
            });
        }

        let images_dir = root.join("images");
        let labels_dir = root.join("labels");
        let classes_file = root.join("classes.txt");

        if !images_dir.exists() || !labels_dir.exists() {
            return Err(Error::InvalidFormat {
                reason: format!(
                    "YOLO format requires images/ and labels/ subdirectories in {}",
                    root.display()
                ),
            });
        }

        // Load class names if available
        let class_names = if classes_file.exists() {
            let content = fs::read_to_string(&classes_file).ok();
            content.map(|c| c.lines().map(|l| l.trim().to_string()).collect())
        } else {
            None
        };

        let mut all_annotations = Vec::new();
        let mut max_class_id = 0u32;

        // Scan images directory
        for entry in fs::read_dir(&images_dir)
            .map_err(|_| Error::InvalidFormat {
                reason: format!("cannot read images dir: {}", images_dir.display()),
            })?
            .filter_map(|e| e.ok())
        {
            let image_path = entry.path();
            if !image_path.is_file() {
                continue;
            }

            // Check if it's an image file
            let is_image = image_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| {
                    ["jpg", "jpeg", "png", "bmp"]
                        .iter()
                        .any(|&e| e.eq_ignore_ascii_case(ext))
                })
                .unwrap_or(false);

            if !is_image {
                continue;
            }

            // Find corresponding label file
            let stem = image_path.file_stem().and_then(|s| s.to_str());
            if stem.is_none() {
                continue;
            }

            let label_path = labels_dir.join(format!("{}.txt", stem.unwrap()));
            if !label_path.exists() {
                // Image without labels → skip (or treat as no objects)
                continue;
            }

            // Parse label file
            let label_content =
                fs::read_to_string(&label_path).map_err(|e| Error::CorruptedSample {
                    index: all_annotations.len(),
                    reason: format!("read label '{}': {e}", label_path.display()),
                })?;

            let mut bboxes = Vec::new();
            for line in label_content.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() != 5 {
                    continue; // Skip malformed lines
                }

                let class_id: u32 = parts[0].parse().map_err(|_| Error::InvalidFormat {
                    reason: format!("invalid class_id in {}: {}", label_path.display(), parts[0]),
                })?;

                let x: f32 = parts[1].parse().map_err(|_| Error::InvalidFormat {
                    reason: format!("invalid x in {}: {}", label_path.display(), parts[1]),
                })?;

                let y: f32 = parts[2].parse().map_err(|_| Error::InvalidFormat {
                    reason: format!("invalid y in {}: {}", label_path.display(), parts[2]),
                })?;

                let width: f32 = parts[3].parse().map_err(|_| Error::InvalidFormat {
                    reason: format!("invalid width in {}: {}", label_path.display(), parts[3]),
                })?;

                let height: f32 = parts[4].parse().map_err(|_| Error::InvalidFormat {
                    reason: format!("invalid height in {}: {}", label_path.display(), parts[4]),
                })?;

                bboxes.push(BoundingBox {
                    class_id,
                    x,
                    y,
                    width,
                    height,
                    confidence: None,
                });

                max_class_id = max_class_id.max(class_id);
            }

            if !bboxes.is_empty() {
                all_annotations.push(ImageAnnotation { image_path, bboxes });
            }
        }

        if all_annotations.is_empty() {
            return Err(Error::InvalidFormat {
                reason: format!("no valid annotations found in {}", root.display()),
            });
        }

        let num_classes = (max_class_id + 1) as usize;

        // Shard
        let annotations = if let Some(shard_id) = config.shard_id {
            let n = config.num_shards;
            all_annotations
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % n == shard_id)
                .map(|(_, e)| e)
                .collect()
        } else {
            all_annotations
        };

        // Select file reader
        #[cfg(target_os = "linux")]
        let reader: Arc<dyn FileReader> = match IoUringReader::new() {
            Ok(r) => {
                tracing::debug!("DetectionDataset: using IoUringReader");
                Arc::new(r)
            }
            Err(e) => {
                tracing::warn!("io_uring unavailable ({e}), falling back to MmapReader");
                Arc::new(MmapReader)
            }
        };
        #[cfg(not(target_os = "linux"))]
        let reader: Arc<dyn FileReader> = Arc::new(MmapReader);

        tracing::info!(
            total_annotations = annotations.len(),
            num_classes = num_classes,
            has_class_names = class_names.is_some(),
            shard = ?config.shard_id,
            "DetectionDataset loaded",
        );

        Ok(Self {
            config,
            annotations,
            num_classes,
            class_names,
            reader,
        })
    }
}

impl Dataset for DetectionDataset {
    fn len(&self) -> usize {
        self.annotations.len()
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.annotations.len() {
            return Err(Error::IndexOutOfBounds {
                index: idx,
                len: self.annotations.len(),
            });
        }

        let annotation = &self.annotations[idx];

        // Read image bytes
        let bytes =
            self.reader
                .read(&annotation.image_path)
                .map_err(|e| Error::CorruptedSample {
                    index: idx,
                    reason: format!("read '{}': {e}", annotation.image_path.display()),
                })?;

        // Decode and resize image (same as ImageNet)
        let img = image::load_from_memory(&bytes).map_err(|e| Error::CorruptedSample {
            index: idx,
            reason: format!("decode '{}': {e}", annotation.image_path.display()),
        })?;

        let img = img.resize_exact(
            INPUT_SIZE,
            INPUT_SIZE,
            image::imageops::FilterType::Triangle,
        );

        let data = img.into_rgb8().into_raw();

        // Primary label is the first bbox's class (or 0 if no bboxes)
        let label = annotation.bboxes.first().map(|b| b.class_id).unwrap_or(0);

        // Store full annotations in metadata
        let metadata = Some(serde_json::json!({
            "filename": annotation.image_path.file_name().and_then(|n| n.to_str()),
            "num_bboxes": annotation.bboxes.len(),
            "bboxes": annotation.bboxes,
        }));

        Ok(Sample {
            data,
            label,
            metadata,
        })
    }

    fn iter_shuffled(&self, seed: u64) -> Box<dyn Iterator<Item = Result<Sample>> + '_> {
        if seed == 0 || !self.config.shuffle {
            return Box::new((0..self.len()).map(move |idx| self.get(idx)));
        }

        let mut indices: Vec<usize> = (0..self.len()).collect();
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        indices.shuffle(&mut rng);

        Box::new(indices.into_iter().map(move |idx| self.get(idx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_bounding_box_conversions() {
        // YOLO normalized → pixel coordinates
        let bbox = BoundingBox::from_yolo_normalized(0, 0.5, 0.5, 0.2, 0.3, 640, 480);
        assert_eq!(bbox.x, 320.0); // 0.5 * 640
        assert_eq!(bbox.y, 240.0); // 0.5 * 480
        assert_eq!(bbox.width, 128.0); // 0.2 * 640
        assert_eq!(bbox.height, 144.0); // 0.3 * 480

        // COCO format
        let bbox2 = BoundingBox::from_coco(1, 100.0, 100.0, 50.0, 50.0);
        assert_eq!(bbox2.x, 125.0); // 100 + 50/2
        assert_eq!(bbox2.y, 125.0); // 100 + 50/2
    }

    #[test]
    fn test_detection_dataset_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create empty subdirectories
        fs::create_dir_all(root.join("images")).unwrap();
        fs::create_dir_all(root.join("labels")).unwrap();

        let config = DatasetConfig::default();
        let result = DetectionDataset::from_yolo_directory(root, config);

        assert!(result.is_err()); // No annotations
    }

    // TODO: Add integration test with actual images and label files
}
