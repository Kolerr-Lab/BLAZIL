// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Time series dataset for forecasting and sequence prediction.
//!
//! **Use cases:**
//! - Stock price prediction
//! - Demand forecasting
//! - Anomaly detection in temporal data
//! - Sensor data analysis
//!
//! **Expected formats:**
//! - CSV: timestamp, feature1, feature2, ..., target
//! - Parquet: columnar time series data
//! - Numpy: .npy files with shape (samples, timesteps, features)
//!
//! **Windowing:**
//! Creates overlapping or non-overlapping windows from continuous time series.
//! Example: [1,2,3,4,5,6,7,8,9,10] with window_size=5, stride=1:
//!   - Window 0: [1,2,3,4,5] → predict 6
//!   - Window 1: [2,3,4,5,6] → predict 7
//!   - ...

use crate::{Dataset, DatasetConfig, Error, Result, Sample};
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::{fs, path::Path};

/// Time series dataset with sliding window.
///
/// Creates fixed-length windows from continuous time series data.
/// Each window becomes one training sample.
pub struct TimeSeriesDataset {
    config: DatasetConfig,
    /// Windowed data: each entry is (window_features, target)
    /// window_features: Vec<f32> of length (window_size × num_features)
    /// target: f32 (for regression) or class label (for classification)
    windows: Vec<(Vec<f32>, f32)>,
    /// Number of features per timestep
    num_features: usize,
    /// Window size (number of timesteps in each window)
    window_size: usize,
    /// Whether this is classification (true) or regression (false)
    is_classification: bool,
}

impl std::fmt::Debug for TimeSeriesDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeSeriesDataset")
            .field("num_windows", &self.windows.len())
            .field("num_features", &self.num_features)
            .field("window_size", &self.window_size)
            .field("is_classification", &self.is_classification)
            .field("config", &self.config)
            .finish()
    }
}

impl TimeSeriesDataset {
    /// Load time series from CSV file.
    ///
    /// **CSV format:**
    /// ```csv
    /// timestamp,feature1,feature2,...,target
    /// 2024-01-01,1.5,2.3,...,100.0
    /// 2024-01-02,1.6,2.4,...,102.0
    /// ```
    ///
    /// - `window_size`: Number of timesteps in each window
    /// - `stride`: Step size for sliding window (1 = overlapping, window_size = non-overlapping)
    /// - `target_col`: Column index for target variable (last column by default)
    /// - `is_classification`: If true, target is converted to class label (u32)
    pub fn from_csv(
        path: impl AsRef<Path>,
        window_size: usize,
        stride: usize,
        config: DatasetConfig,
    ) -> Result<Self> {
        Self::from_csv_with_options(path, window_size, stride, None, false, config)
    }

    /// Load time series from CSV with custom options.
    pub fn from_csv_with_options(
        path: impl AsRef<Path>,
        window_size: usize,
        stride: usize,
        target_col: Option<usize>,
        is_classification: bool,
        config: DatasetConfig,
    ) -> Result<Self> {
        config.validate()?;
        let path = path.as_ref();

        if !path.exists() {
            return Err(Error::DatasetNotFound {
                path: path.to_path_buf(),
            });
        }

        if window_size == 0 {
            return Err(Error::InvalidFormat {
                reason: "window_size must be > 0".to_string(),
            });
        }

        if stride == 0 {
            return Err(Error::InvalidFormat {
                reason: "stride must be > 0".to_string(),
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

        // Note: We skip column 0 (timestamp), so num_features = num_cols - 1
        // and target_col indices need to be adjusted
        let num_features = num_cols - 1; // All columns except timestamp
        let target_col_original = target_col.unwrap_or(num_cols - 1);

        // Adjust target_col for skipped timestamp (col 0)
        if target_col_original == 0 {
            return Err(Error::InvalidFormat {
                reason: "target_col cannot be timestamp column (col 0)".to_string(),
            });
        }
        let target_idx = target_col_original - 1; // Adjust for skipped timestamp

        // Parse all rows
        let mut all_data: Vec<Vec<f32>> = Vec::new();
        for (line_idx, line) in lines.enumerate() {
            let values: Result<Vec<f32>> = line
                .split(',')
                .enumerate()
                .filter(|(idx, _)| *idx != 0) // Skip timestamp column
                .map(|(_, v)| {
                    v.trim().parse::<f32>().map_err(|_| Error::InvalidFormat {
                        reason: format!("invalid float on line {}: {}", line_idx + 2, v),
                    })
                })
                .collect();

            all_data.push(values?);
        }

        if all_data.len() < window_size + 1 {
            return Err(Error::InvalidFormat {
                reason: format!(
                    "not enough data for windowing: {} rows, need at least {}",
                    all_data.len(),
                    window_size + 1
                ),
            });
        }

        // Create sliding windows
        let mut all_windows = Vec::new();
        let mut idx = 0;

        while idx + window_size < all_data.len() {
            // Collect window
            let mut window_features = Vec::with_capacity(window_size * num_features);
            for t in 0..window_size {
                window_features.extend_from_slice(&all_data[idx + t]);
            }

            // Target is the value at the next timestep
            let target_row = &all_data[idx + window_size];
            let target = if target_idx >= target_row.len() {
                return Err(Error::InvalidFormat {
                    reason: format!("target_col {} out of bounds", target_idx),
                });
            } else {
                target_row[target_idx]
            };

            all_windows.push((window_features, target));
            idx += stride;
        }

        if all_windows.is_empty() {
            return Err(Error::InvalidFormat {
                reason: "no windows created (data too short?)".to_string(),
            });
        }

        // Shard
        let windows = if let Some(shard_id) = config.shard_id {
            let n = config.num_shards;
            all_windows
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % n == shard_id)
                .map(|(_, e)| e)
                .collect()
        } else {
            all_windows
        };

        tracing::info!(
            total_windows = windows.len(),
            window_size = window_size,
            num_features = num_features,
            stride = stride,
            is_classification = is_classification,
            shard = ?config.shard_id,
            "TimeSeriesDataset loaded",
        );

        Ok(Self {
            config,
            windows,
            num_features,
            window_size,
            is_classification,
        })
    }

    /// Convert Vec<f32> to Vec<u8> for Sample.data.
    fn floats_to_bytes(floats: Vec<f32>) -> Vec<u8> {
        floats.into_iter().flat_map(|f| f.to_le_bytes()).collect()
    }
}

impl Dataset for TimeSeriesDataset {
    fn len(&self) -> usize {
        self.windows.len()
    }

    fn get(&self, idx: usize) -> Result<Sample> {
        if idx >= self.windows.len() {
            return Err(Error::IndexOutOfBounds {
                index: idx,
                len: self.windows.len(),
            });
        }

        let (window_features, target) = &self.windows[idx];

        let data = Self::floats_to_bytes(window_features.clone());

        let label = if self.is_classification {
            *target as u32
        } else {
            // For regression, store target in metadata
            0
        };

        let metadata = Some(serde_json::json!({
            "target": target,
            "window_size": self.window_size,
            "num_features": self.num_features,
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
    fn test_timeseries_from_csv() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("timeseries.csv");

        // Create simple time series: trend + noise
        let mut csv_content = "timestamp,value\n".to_string();
        for i in 0..20 {
            csv_content.push_str(&format!("{},{}\n", i, i as f32 * 1.5));
        }
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();
        let dataset = TimeSeriesDataset::from_csv(&csv_path, 5, 1, config)?;

        // 20 rows, window_size=5, stride=1
        // Valid windows: 0-4→5, 1-5→6, ..., 14-18→19
        // Total: 15 windows
        assert_eq!(dataset.len(), 15);

        let sample = dataset.get(0)?;
        // 5 timesteps × 1 feature × 4 bytes = 20 bytes
        assert_eq!(sample.data.len(), 5 * 1 * 4);

        Ok(())
    }

    #[test]
    fn test_timeseries_stride() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("timeseries.csv");

        let mut csv_content = "timestamp,value\n".to_string();
        for i in 0..100 {
            csv_content.push_str(&format!("{},{}\n", i, i));
        }
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();

        // stride=1 (overlapping)
        let dataset1 = TimeSeriesDataset::from_csv(&csv_path, 10, 1, config.clone())?;
        assert_eq!(dataset1.len(), 90); // 100 - 10

        // stride=10 (non-overlapping)
        let dataset2 = TimeSeriesDataset::from_csv(&csv_path, 10, 10, config)?;
        assert_eq!(dataset2.len(), 9); // floor((100 - 10) / 10)

        Ok(())
    }

    #[test]
    fn test_timeseries_multivariate() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("multivariate.csv");

        let mut csv_content = "timestamp,feature1,feature2,target\n".to_string();
        for i in 0..50 {
            csv_content.push_str(&format!("{},{},{},{}\n", i, i, i * 2, i + 1));
        }
        fs::write(&csv_path, csv_content).unwrap();

        let config = DatasetConfig::default();
        let dataset = TimeSeriesDataset::from_csv(&csv_path, 5, 1, config)?;

        // 3 features (feature1, feature2, target)
        assert_eq!(dataset.num_features, 3);

        let sample = dataset.get(0)?;
        // 5 timesteps × 3 features × 4 bytes = 60 bytes
        assert_eq!(sample.data.len(), 5 * 3 * 4);

        Ok(())
    }

    #[test]
    fn test_timeseries_sharding() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("timeseries.csv");

        let mut csv_content = "timestamp,value\n".to_string();
        for i in 0..100 {
            csv_content.push_str(&format!("{},{}\n", i, i));
        }
        fs::write(&csv_path, csv_content).unwrap();

        let config0 = DatasetConfig::default().with_shard(0, 2);
        let dataset0 = TimeSeriesDataset::from_csv(&csv_path, 10, 1, config0)?;

        let config1 = DatasetConfig::default().with_shard(1, 2);
        let dataset1 = TimeSeriesDataset::from_csv(&csv_path, 10, 1, config1)?;

        assert_eq!(dataset0.len() + dataset1.len(), 90);

        Ok(())
    }
}
