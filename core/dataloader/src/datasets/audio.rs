// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Audio dataset for speech recognition, classification, and event detection.
//!
//! **Use cases:**
//! - Voice command recognition
//! - Speaker identification
//! - Audio event detection (door knock, glass break, etc.)
//! - Speech emotion recognition
//!
//! **Expected formats:**
//! - WAV files (mono or stereo, various sample rates)
//! - Directory structure: `class_name/*.wav`
//! - CSV manifest: `path,label,duration`
//!
//! **Preprocessing:**
//! - Resampling to target sample rate (16kHz default for speech)
//! - Mono conversion (stereo → mono via averaging)
//! - Duration normalization (pad/truncate to fixed length)
//!
//! **Note:** Requires `audio` feature flag. Add to Cargo.toml:
//! ```toml
//! blazil-dataloader = { path = "...", features = ["audio"] }
//! ```

#[cfg(feature = "audio")]
use crate::{
    readers::{FileReader, MmapReader},
    Dataset, DatasetConfig, Error, Result, Sample,
};

#[cfg(feature = "audio")]
use rand::{seq::SliceRandom, SeedableRng};
#[cfg(feature = "audio")]
use rand_chacha::ChaCha8Rng;
#[cfg(feature = "audio")]
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(all(feature = "audio", target_os = "linux"))]
use crate::readers::IoUringReader;

/// Audio file extensions recognized.
#[cfg(feature = "audio")]
const AUDIO_EXTENSIONS: &[&str] = &["wav", "wave"];

/// Default target sample rate for speech (16kHz).
#[cfg(feature = "audio")]
const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Default audio duration in seconds (10s for speech, 1-2s for commands).
#[cfg(feature = "audio")]
const DEFAULT_DURATION_SECS: f32 = 10.0;

/// Audio dataset for classification tasks.
///
/// Loads WAV files, resamples to target sample rate, converts to mono,
/// and pads/truncates to fixed duration.
#[cfg(feature = "audio")]
pub struct AudioDataset {
    config: DatasetConfig,
    /// List of (audio_path, label) for this shard
    entries: Vec<(PathBuf, u32)>,
    /// Target sample rate (Hz)
    target_sample_rate: u32,
    /// Target duration (seconds)
    target_duration: f32,
    /// Number of samples per audio clip
    num_samples: usize,
    /// File reader (io_uring on Linux, mmap elsewhere)
    #[allow(dead_code)]
    reader: Arc<dyn FileReader>,
}

#[cfg(feature = "audio")]
impl std::fmt::Debug for AudioDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDataset")
            .field("num_entries", &self.entries.len())
            .field("target_sample_rate", &self.target_sample_rate)
            .field("target_duration", &self.target_duration)
            .field("num_samples", &self.num_samples)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "audio")]
impl AudioDataset {
    /// Open audio dataset from directory (class folders).
    ///
    /// **Directory structure:**
    /// ```text
    /// <root>/
    ///   class_0/
    ///     audio1.wav
    ///     audio2.wav
    ///   class_1/
    ///     audio3.wav
    /// ```
    pub fn from_directory(root: impl AsRef<Path>, config: DatasetConfig) -> Result<Self> {
        Self::from_directory_with_options(root, DEFAULT_SAMPLE_RATE, DEFAULT_DURATION_SECS, config)
    }

    /// Open audio dataset with custom sample rate and duration.
    pub fn from_directory_with_options(
        root: impl AsRef<Path>,
        target_sample_rate: u32,
        target_duration: f32,
        config: DatasetConfig,
    ) -> Result<Self> {
        config.validate()?;
        let root = root.as_ref();

        if !root.exists() {
            return Err(Error::DatasetNotFound {
                path: root.to_path_buf(),
            });
        }

        let mut all_entries = Vec::new();

        // Scan class directories
        let mut class_dirs: Vec<_> = fs::read_dir(root)
            .map_err(|_e| Error::DatasetNotFound {
                path: root.to_path_buf(),
            })?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        class_dirs.sort_by_key(|e| e.file_name());

        for (class_idx, class_dir) in class_dirs.iter().enumerate() {
            let class_path = class_dir.path();

            for entry in fs::read_dir(&class_path)
                .map_err(|_| Error::InvalidFormat {
                    reason: format!("cannot read class dir: {}", class_path.display()),
                })?
                .filter_map(|e| e.ok())
            {
                let file_path = entry.path();
                if !file_path.is_file() {
                    continue;
                }

                // Check audio extension
                if !Self::is_audio_file(&file_path) {
                    continue;
                }

                all_entries.push((file_path, class_idx as u32));
            }
        }

        if all_entries.is_empty() {
            return Err(Error::InvalidFormat {
                reason: format!("no audio files found under {}", root.display()),
            });
        }

        Self::from_entries(all_entries, target_sample_rate, target_duration, config)
    }

    /// Create dataset from entries.
    fn from_entries(
        all_entries: Vec<(PathBuf, u32)>,
        target_sample_rate: u32,
        target_duration: f32,
        config: DatasetConfig,
    ) -> Result<Self> {
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

        let num_samples = (target_sample_rate as f32 * target_duration) as usize;

        // Select file reader
        #[cfg(target_os = "linux")]
        let reader: Arc<dyn FileReader> = match IoUringReader::new() {
            Ok(r) => {
                tracing::debug!("AudioDataset: using IoUringReader");
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
            total_samples = entries.len(),
            sample_rate = target_sample_rate,
            duration = target_duration,
            num_samples = num_samples,
            shard = ?config.shard_id,
            "AudioDataset loaded",
        );

        Ok(Self {
            config,
            entries,
            target_sample_rate,
            target_duration,
            num_samples,
            reader,
        })
    }

    /// Check if file is an audio file.
    fn is_audio_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                AUDIO_EXTENSIONS
                    .iter()
                    .any(|&v| v.eq_ignore_ascii_case(ext))
            })
            .unwrap_or(false)
    }

    /// Load and preprocess audio: read WAV → resample → mono → pad/truncate.
    fn load_and_preprocess(&self, path: &Path) -> Result<Vec<f32>> {
        // Read WAV file using hound
        let mut reader = hound::WavReader::open(path).map_err(|e| Error::CorruptedSample {
            index: 0,
            reason: format!("hound read '{}': {e}", path.display()),
        })?;

        let spec = reader.spec();
        let source_sample_rate = spec.sample_rate;
        let channels = spec.channels as usize;

        // Read samples
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|s| s.unwrap_or(0) as f32 / 32768.0)
                .collect(),
            hound::SampleFormat::Float => {
                reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect()
            }
        };

        // Convert stereo to mono (average channels)
        let mono: Vec<f32> = if channels == 1 {
            samples
        } else {
            samples
                .chunks_exact(channels)
                .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
                .collect()
        };

        // Simple resampling (nearest neighbor - for production use rubato or similar)
        let resampled = if source_sample_rate != self.target_sample_rate {
            let ratio = self.target_sample_rate as f32 / source_sample_rate as f32;
            let new_len = (mono.len() as f32 * ratio) as usize;
            (0..new_len)
                .map(|i| {
                    let src_idx = (i as f32 / ratio) as usize;
                    mono.get(src_idx).copied().unwrap_or(0.0)
                })
                .collect()
        } else {
            mono
        };

        // Pad or truncate to target length
        let mut result = vec![0.0; self.num_samples];
        let copy_len = resampled.len().min(self.num_samples);
        result[..copy_len].copy_from_slice(&resampled[..copy_len]);

        Ok(result)
    }

    /// Convert Vec<f32> to Vec<u8>.
    fn floats_to_bytes(floats: Vec<f32>) -> Vec<u8> {
        floats.into_iter().flat_map(|f| f.to_le_bytes()).collect()
    }
}

#[cfg(feature = "audio")]
impl Dataset for AudioDataset {
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

        let (path, label) = &self.entries[idx];

        // Load and preprocess audio
        let audio_samples = self.load_and_preprocess(path)?;
        let data = Self::floats_to_bytes(audio_samples);

        let metadata = Some(serde_json::json!({
            "filename": path.file_name().and_then(|n| n.to_str()),
            "sample_rate": self.target_sample_rate,
            "duration": self.target_duration,
            "num_samples": self.num_samples,
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

// Stub implementation when audio feature is disabled
#[cfg(not(feature = "audio"))]
pub struct AudioDataset;

#[cfg(not(feature = "audio"))]
impl AudioDataset {
    pub fn from_directory<P: AsRef<std::path::Path>>(
        _root: P,
        _config: crate::DatasetConfig,
    ) -> crate::Result<Self> {
        Err(crate::Error::InvalidFormat {
            reason: "AudioDataset requires 'audio' feature flag. Add features = [\"audio\"] to Cargo.toml".to_string(),
        })
    }
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Note: Creating actual WAV files in tests requires hound
    // For now, we test the structure and error handling

    #[test]
    fn test_audio_extensions() {
        assert!(AudioDataset::is_audio_file(Path::new("test.wav")));
        assert!(AudioDataset::is_audio_file(Path::new("test.WAVE")));
        assert!(!AudioDataset::is_audio_file(Path::new("test.mp3")));
        assert!(!AudioDataset::is_audio_file(Path::new("test.txt")));
    }

    #[test]
    fn test_audio_dataset_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let config = DatasetConfig::default();
        let result = AudioDataset::from_directory(root, config);

        assert!(result.is_err());
    }

    // TODO: Add integration tests with real WAV files
    // This requires generating WAV files using hound in test setup
}
