// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! ImageNet normalization transforms.
//!
//! Standard ImageNet preprocessing:
//! - mean = [0.485, 0.456, 0.406]  (per channel, 0–1 range)
//! - std  = [0.229, 0.224, 0.225]  (per channel)
//!
//! Input: `u8` RGB bytes packed as H×W×C (row-major).
//! Output: `f32` bytes packed as H×W×C or C×H×W (after `ToChannelFirst`).
//!
//! The normalised float value for each pixel channel:
//!   `out = (pixel / 255.0 - mean) / std`

use super::Transform;
use crate::{Error, Result, Sample};

/// ImageNet channel statistics (mean, std) for R, G, B.
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Standard ImageNet normalization.
///
/// Converts u8 RGB (H×W×C) → f32 RGB (H×W×C) with per-channel
/// mean subtraction and std division.
///
/// Output `data` contains 4× the bytes of the input
/// (f32 = 4 bytes per value): 224×224×3×4 = 602,112 bytes.
pub struct NormalizeImageNet;

impl Transform for NormalizeImageNet {
    fn apply(&self, mut sample: Sample) -> Result<Sample> {
        let n = sample.data.len();
        if !n.is_multiple_of(3) {
            return Err(Error::InvalidFormat {
                reason: format!("NormalizeImageNet: data length {n} is not divisible by 3"),
            });
        }

        let num_pixels = n / 3;
        let mut out = Vec::<u8>::with_capacity(num_pixels * 3 * 4);

        for chunk in sample.data.chunks_exact(3) {
            for (c, &byte) in chunk.iter().enumerate() {
                let norm = (byte as f32 / 255.0 - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
                out.extend_from_slice(&norm.to_le_bytes());
            }
        }

        sample.data = out;
        Ok(sample)
    }
}

/// Reorder normalized data from H×W×C to C×H×W (PyTorch tensor layout).
///
/// Input:  `f32` bytes in H×W×C layout (from `NormalizeImageNet`).
/// Output: `f32` bytes in C×H×W layout.
///
/// `height` and `width` must be provided at construction time.
pub struct ToChannelFirst {
    height: usize,
    width: usize,
    channels: usize,
}

impl ToChannelFirst {
    /// Create the transform.
    ///
    /// For standard ImageNet: `ToChannelFirst::new(224, 224, 3)`.
    pub fn new(height: usize, width: usize, channels: usize) -> Self {
        Self {
            height,
            width,
            channels,
        }
    }
}

impl Transform for ToChannelFirst {
    fn apply(&self, mut sample: Sample) -> Result<Sample> {
        let h = self.height;
        let w = self.width;
        let c = self.channels;
        let floats_per_elem = 4usize; // f32 = 4 bytes
        let expected = h * w * c * floats_per_elem;

        if sample.data.len() != expected {
            return Err(Error::InvalidFormat {
                reason: format!(
                    "ToChannelFirst: expected {expected} bytes ({}×{}×{}×4), got {}",
                    h,
                    w,
                    c,
                    sample.data.len()
                ),
            });
        }

        // Reinterpret as f32 slice.
        let num_floats = h * w * c;
        let mut floats = vec![0f32; num_floats];
        for (i, chunk) in sample.data.chunks_exact(4).enumerate() {
            floats[i] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        // Transpose H×W×C → C×H×W
        let mut transposed = vec![0f32; num_floats];
        for row in 0..h {
            for col in 0..w {
                for chan in 0..c {
                    let src = row * w * c + col * c + chan;
                    let dst = chan * h * w + row * w + col;
                    transposed[dst] = floats[src];
                }
            }
        }

        // Pack back into bytes.
        let mut out = Vec::<u8>::with_capacity(expected);
        for f in transposed {
            out.extend_from_slice(&f.to_le_bytes());
        }

        sample.data = out;
        Ok(sample)
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sample;

    fn make_sample(data: Vec<u8>) -> Sample {
        Sample {
            data,
            label: 0,
            metadata: None,
        }
    }

    fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn test_normalize_output_length() {
        // 4 pixels × 3 channels = 12 u8 input → 48 bytes f32 output
        let s = make_sample(vec![128u8; 12]);
        let out = NormalizeImageNet.apply(s).unwrap();
        assert_eq!(out.data.len(), 12 * 4);
    }

    #[test]
    fn test_normalize_zero_pixel_gives_negative_mean_over_std() {
        // pixel=0 → (0/255 - mean) / std = -mean/std
        let s = make_sample(vec![0u8; 3]); // single RGB pixel
        let out = NormalizeImageNet.apply(s).unwrap();
        let floats = bytes_to_f32s(&out.data);

        for c in 0..3 {
            let expected = -IMAGENET_MEAN[c] / IMAGENET_STD[c];
            let diff = (floats[c] - expected).abs();
            assert!(
                diff < 1e-5,
                "channel {c}: got {}, expected {expected}",
                floats[c]
            );
        }
    }

    #[test]
    fn test_normalize_255_pixel_gives_1_minus_mean_over_std() {
        let s = make_sample(vec![255u8; 3]);
        let out = NormalizeImageNet.apply(s).unwrap();
        let floats = bytes_to_f32s(&out.data);

        for c in 0..3 {
            let expected = (1.0 - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
            let diff = (floats[c] - expected).abs();
            assert!(
                diff < 1e-5,
                "channel {c}: got {}, expected {expected}",
                floats[c]
            );
        }
    }

    #[test]
    fn test_to_channel_first_shape() {
        // 2×2×3 HWC → 3×2×2 CHW (same number of elements)
        let h = 2usize;
        let w = 2usize;
        let c = 3usize;
        let num_floats = h * w * c;
        // Build f32 bytes sequentially 0.0, 1.0, 2.0, ...
        let mut bytes = Vec::with_capacity(num_floats * 4);
        for i in 0..num_floats {
            bytes.extend_from_slice(&(i as f32).to_le_bytes());
        }
        let s = make_sample(bytes);
        let out = ToChannelFirst::new(h, w, c).apply(s).unwrap();
        assert_eq!(out.data.len(), num_floats * 4);
    }

    #[test]
    fn test_to_channel_first_values() {
        // Single pixel HWC → CHW: values stay the same (just reordered).
        // HWC for 1×1×3: [R, G, B] at (0,0)
        // CHW: R_plane[0,0]=R, G_plane[0,0]=G, B_plane[0,0]=B → same order.
        let input_f32s: Vec<f32> = vec![0.1, 0.2, 0.3];
        let mut bytes = Vec::with_capacity(12);
        for f in &input_f32s {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        let s = make_sample(bytes);
        let out = ToChannelFirst::new(1, 1, 3).apply(s).unwrap();
        let floats = bytes_to_f32s(&out.data);
        for (a, b) in floats.iter().zip(input_f32s.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_normalize_invalid_length() {
        let s = make_sample(vec![0u8; 5]); // not divisible by 3
        assert!(NormalizeImageNet.apply(s).is_err());
    }

    #[test]
    fn test_to_channel_first_wrong_size() {
        let s = make_sample(vec![0u8; 100]);
        assert!(ToChannelFirst::new(224, 224, 3).apply(s).is_err());
    }
}
