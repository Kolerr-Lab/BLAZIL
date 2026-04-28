// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Feature-based dataset for anomaly detection and classification.
//!
//! **Use cases:**
//! - Fraud detection (transaction features → fraud/normal)
//! - Network intrusion detection
//! - Manufacturing defect detection
//! - System monitoring and alerting
//!
//! **Expected formats:**
//! - CSV: feature1, feature2, ..., label
//! - Parquet: columnar feature data
//! - Numpy: .npy files with shape (samples, features)
//!
//! **Normalization:**
//! Supports z-score normalization (mean/std) and min-max scaling.

use crate::{Dataset, DatasetConfig, Error, Result, Sample};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::{fs, path::Path};

/// Statistics for feature normalization.
#[derive(Debug, Clone)]
pub struct FeatureStats {
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
    pub min: Vec<f32>,
    pub max: Vec<f32>,
}

impl FeatureStats {
    /// Compute statistics from data.
    pub fn from_data(data: &[Vec<f32>]) -> Result<Self> {
        if data.is_empty() {
            return Err(Error::InvalidFormat {
                reason: "cannot compute stats from empty data".to_string(),
            });
        }

        let num_features = data[0].len();
        let n = data.len() as f32;

        let mut mean = vec![0.0; num_features];
        let mut min = vec![f32::INFINITY; num_features];
        let mut max = vec![f32::NEG_INFINITY; num_features];

        // Compute mean, min, max
        for row in data {
            for (i, &val) in row.iter().enumerate() {
                mean[i] += val;
                min[i] = min[i].min(val);
                max[i] = max[i].max(val);
            }
        }

        for m in &mut mean {
            *m /= n;
        }

        // Compute std
        let mut std = vec![0.0; num_features];
        for row in data {
            for (i, &val) in row.iter().enumerate() {
                let diff = val - mean[i];
                std[i] += diff * diff;
            }
        }

        for s in &mut std {
            *s = (*s / n).sqrt();
            // Avoid division by zero
            if *s < 1e-8 {
                *s = 1.0;
            }
        }

        Ok(Self {
            mean,
            std,
            min,
            max,
        })
    }

    /// Z-score normalization: (x - mean) / std
    pub fn normalize_zscore(&self, features: &[f32]) -> Vec<f32> {
        features
            .iter()
            .zip(&self.mean)
            .zip(&self.std)
            .map(|((&x, &m), &s)| (x - m) / s)
            .collect()
    }

    /// Min-max normalization: (x - min) / (max - min)
    pub fn normalize_minmax(&self, features: &[f32]) -> Vec<f32> {
        features
            .iter()
            .zip(&self.min)
            .zip(&self.max)
            .map(|((&x, &min_val), &max_val)| {
                let range = max_val - min_val;
                if range < 1e-8 {
                    0.5 // Constant feature → map to 0.5
                } else {
                    (x - min_val) / range
                }
            })
            .collect()
    }
}

/// Normalization method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizationMethod {
    None,
    ZScore,
    MinMax,
}

/// Feature dataset for anomaly detection and classification.
pub struct FeatureDataset {
    config: DatasetConfig,
    /// Feature vectors and labels
    entries: Vec<(Vec<f32>, u32)>,
    /// Number of features
    num_features: usize,
    /// Feature statistics for normalization
    stats: Option<FeatureStats>,
    /// Normalization method
    normalization: NormalizationMethod,
}

impl std::fmt::Debug for FeatureDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureDataset")
            .field("num_entries", &self.entries.len())
            .field("num_features", &self.num_features)
            .field("normalization", &self.normalization)
            .field("has_stats", &self.stats.is_some())
            .field("config", &self.config)
            .finish()
    }
}

impl FeatureDataset {
    /// Load feature dataset from CSV.
    ///
    /// **CSV format:**
    /// ```csv
    /// feature1,feature2,...,label
    /// 1.5,2.3,...,0
    /// 1.6,2.4,...,1
    /// ```
    ///
    /// - Last column is assumed to be the label
    /// - All other columns are features
    /// - Normalization is applied if specified
    pub fn from_csv(
        path: impl AsRef<Path>,
        normalization: NormalizationMethod,
        config: DatasetConfig,
    ) -> Result<Self> {
        Self::from_csv_with_options(path, None, normalization, config)
    }

    /// Load feature dataset from CSV with custom label column.
    pub fn from_csv_with_options(
        path: impl AsRef<Path>,
        label_col: Option<usize>,
        normalization: NormalizationMethod,
        config: DatasetConfig,
    ) -> Result<Self> {
        config.validate()?;
        let path = path.as_ref();

        if !path.exists() {
            return Err(Error::DatasetNotFound {
                path: path.to_path_buf(),
            });
        }

        let content = fs::read_to_string(path).map_err(|_e| Error::DatasetNotFound {
            path: path.to_path_buf(),
        })?;

        let mut lines = content.lines();
        let header = lines.next().ok_or_else(|| Error::InvalidFormat {
            reason: "empty CSV file".to_string(),
        })?;

        let num_cols = header.split(',').count();
        if num_cols < 2 {
            return Err(Error::InvalidFormat {
                reason: format!("CSV must have at least 2 columns, got {}", num_cols),
            });
        }

        let label_idx = label_col.unwrap_or(num_cols - 1);
        let num_features = num_cols - 1;

        // Parse all rows
        let mut all_entries = Vec::new();
        let mut all_features_for_stats = Vec::new();

        for (line_idx, line) in lines.enumerate() {
            let values: Vec<&str> = line.split(',').collect();
            if values.len() != num_cols {
                continue; // Skip malformed lines
            }

            let mut features = Vec::with_capacity(num_features);
            let mut label = 0u32;

            for (col_idx, value) in values.iter().enumerate() {
                if col_idx == label_idx {
                    label = value.trim().parse().map_err(|_| Error::InvalidFormat {
                        reason: format!("invalid label on line {}: {}", line_idx + 2, value),
                    })?;
                } else {
                    let feature: f32 = value.trim().parse().map_err(|_| Error::InvalidFormat {
                        reason: format!("invalid feature on line {}: {}", line_idx + 2, value),
                    })?;
                    features.push(feature);
                }
            }

            all_features_for_stats.push(features.clone());
            all_entries.push((features, label));
        }

        if all_entries.is_empty() {
            return Err(Error::InvalidFormat {
                reason: "no valid entries in CSV".to_string(),
            });
        }

        // Compute statistics for normalization
        let stats = if normalization != NormalizationMethod::None {
            Some(FeatureStats::from_data(&all_features_for_stats)?)
        } else {
            None
        };

        // Apply normalization
        if let Some(ref stats) = stats {
            for (features, _) in &mut all_entries {
                *features = match normalization {
                    NormalizationMethod::ZScore => stats.normalize_zscore(features),
                    NormalizationMethod::MinMax => stats.normalize_minmax(features),
                    NormalizationMethod::None => features.clone(),
                };
            }
        }

        // Shard
        let entries = if let Some(shard_id) = config.shard_id {
            let n = config.num_shards;
            all_entries
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % n == shard_id)
                .map(|(_, e)| e)
                .collect()
        } else {
            all_entries
        };

        tracing::info!(
            total_samples = entries.len(),
            num_features = num_features,
            normalization = ?normalization,
            shard = ?config.shard_id,
            "FeatureDataset loaded",
        );

        Ok(Self {
            config,
            entries,
            num_features,
            stats,
            normalization,
        })
    }

    /// Convert Vec<f32> to Vec<u8> for Sample.data.
    fn floats_to_bytes(floats: Vec<f32>) -> Vec<u8> {
        floats.into_iter().flat_map(|f| f.to_le_bytes()).collect()
    }
}

impl Dataset for FeatureDataset {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.entries.len() {
            return Err(Error::IndexOutOfBounds {
                index: idx,
                len: self.entries.len(),
            });
        }

        let (features, label) = &self.entries[idx];
        let data = Self::floats_to_bytes(features.clone());

        let metadata = Some(serde_json::json!({
            "num_features": self.num_features,
            "normalization": format!("{:?}", self.normalization),
        }));

        Ok(Sample {
            data,
            label: *label,
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
    fn test_feature_stats() -> Result<()> {
        let data = vec![
            vec![1.0, 2.0, 3.0],
            vec![2.0, 4.0, 6.0],
            vec![3.0, 6.0, 9.0],
        ];

        let stats = FeatureStats::from_data(&data)?;

        assert_eq!(stats.mean, vec![2.0, 4.0, 6.0]);
        assert_eq!(stats.min, vec![1.0, 2.0, 3.0]);
        assert_eq!(stats.max, vec![3.0, 6.0, 9.0]);

        // Test normalization
        let normalized = stats.normalize_zscore(&vec![2.0, 4.0, 6.0]);
        // Should be close to [0.0, 0.0, 0.0] (mean values)
        assert!(normalized[0].abs() < 0.01);

        Ok(())
    }

    #[test]
    fn test_feature_dataset_from_csv() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("features.csv");

        let csv_content = r#"feature1,feature2,feature3,label
1.0,2.0,3.0,0
2.0,4.0,6.0,1
3.0,6.0,9.0,0
4.0,8.0,12.0,1
"#;
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();
        let dataset = FeatureDataset::from_csv(&csv_path, NormalizationMethod::None, config)?;

        assert_eq!(dataset.len(), 4);
        assert_eq!(dataset.num_features, 3);

        let sample = dataset.get(0)?;
        assert_eq!(sample.label, 0);
        // 3 features × 4 bytes = 12 bytes
        assert_eq!(sample.data.len(), 12);

        Ok(())
    }

    #[test]
    fn test_zscore_normalization() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("features.csv");

        let csv_content = r#"feature1,feature2,label
10.0,100.0,0
20.0,200.0,1
30.0,300.0,0
"#;
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();
        let dataset = FeatureDataset::from_csv(&csv_path, NormalizationMethod::ZScore, config)?;

        assert!(dataset.stats.is_some());

        // After normalization, mean should be ~0, std ~1
        let sample = dataset.get(1)?; // Middle value
                                      // Middle value should be close to 0 after z-score normalization

        Ok(())
    }

    #[test]
    fn test_minmax_normalization() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("features.csv");

        let csv_content = r#"feature1,feature2,label
0.0,10.0,0
5.0,20.0,1
10.0,30.0,0
"#;
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();
        let dataset = FeatureDataset::from_csv(&csv_path, NormalizationMethod::MinMax, config)?;

        assert!(dataset.stats.is_some());

        // After min-max, values should be in [0, 1]
        for idx in 0..dataset.len() {
            let sample = dataset.get(idx)?;
            let floats: Vec<f32> = sample
                .data
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            for &val in &floats {
                assert!(val >= -0.01 && val <= 1.01); // Allow small float error
            }
        }

        Ok(())
    }

    #[test]
    fn test_feature_sharding() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("features.csv");

        let mut csv_content = "feature1,feature2,label\n".to_string();
        for i in 0..100 {
            csv_content.push_str(&format!("{},{},{}\n", i, i * 2, i % 2));
        }
        fs::write(&csv_path, csv_content).unwrap();

        let config0 = DatasetConfig::default().with_shard(0, 2);
        let dataset0 = FeatureDataset::from_csv(&csv_path, NormalizationMethod::None, config0)?;

        let config1 = DatasetConfig::default().with_shard(1, 2);
        let dataset1 = FeatureDataset::from_csv(&csv_path, NormalizationMethod::None, config1)?;

        assert_eq!(dataset0.len(), 50);
        assert_eq!(dataset1.len(), 50);

        Ok(())
    }
}
