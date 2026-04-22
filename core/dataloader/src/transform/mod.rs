// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Data transformation and augmentation pipelines.
//!
//! Transforms are composable, stateless, and `Send + Sync`.
//! Apply them to a [`crate::Sample`] before feeding into the GPU.
//!
//! ## Standard ImageNet preprocessing
//!
//! ```text
//! Raw JPEG
//!   → Decode (224×224 RGB bytes)          [done in ImageNetDataset]
//!   → Normalize(mean=[0.485,0.456,0.406],
//!               std =[0.229,0.224,0.225])  [NormalizeImageNet]
//!   → f32 tensor layout: C×H×W            [ToChannelFirst]
//! ```

pub mod normalize;

pub use normalize::{NormalizeImageNet, ToChannelFirst};

use crate::{Error, Result, Sample};

/// A stateless transformation applied to a single [`Sample`].
pub trait Transform: Send + Sync {
    fn apply(&self, sample: Sample) -> Result<Sample>;
}

/// A composed chain of transforms applied in order.
pub struct TransformChain {
    transforms: Vec<Box<dyn Transform>>,
}

impl TransformChain {
    pub fn new(transforms: Vec<Box<dyn Transform>>) -> Self {
        Self { transforms }
    }

    /// Apply all transforms in order, short-circuiting on error.
    pub fn apply(&self, mut sample: Sample) -> Result<Sample> {
        for t in &self.transforms {
            sample = t.apply(sample)?;
        }
        Ok(sample)
    }
}

/// Identity transform — passes sample through unchanged.
/// Useful as a placeholder or in tests.
pub struct Identity;

impl Transform for Identity {
    fn apply(&self, sample: Sample) -> Result<Sample> {
        Ok(sample)
    }
}

/// Validate that sample data has the expected byte length.
pub struct ValidateSize {
    expected_bytes: usize,
}

impl ValidateSize {
    /// `expected_bytes` = H × W × C (e.g. 224 × 224 × 3 = 150_528)
    pub fn new(expected_bytes: usize) -> Self {
        Self { expected_bytes }
    }
}

impl Transform for ValidateSize {
    fn apply(&self, sample: Sample) -> Result<Sample> {
        if sample.data.len() != self.expected_bytes {
            return Err(Error::InvalidFormat {
                reason: format!(
                    "Expected {} bytes, got {}",
                    self.expected_bytes,
                    sample.data.len()
                ),
            });
        }
        Ok(sample)
    }
}

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

    #[test]
    fn test_identity_passthrough() {
        let s = make_sample(vec![1, 2, 3]);
        let t = Identity;
        let out = t.apply(s).unwrap();
        assert_eq!(out.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_validate_size_ok() {
        let s = make_sample(vec![0u8; 150_528]);
        let t = ValidateSize::new(150_528);
        assert!(t.apply(s).is_ok());
    }

    #[test]
    fn test_validate_size_fail() {
        let s = make_sample(vec![0u8; 100]);
        let t = ValidateSize::new(150_528);
        assert!(t.apply(s).is_err());
    }

    #[test]
    fn test_chain_applies_in_order() {
        // ValidateSize(3) → Identity: ok
        let chain = TransformChain::new(vec![Box::new(ValidateSize::new(3)), Box::new(Identity)]);
        let s = make_sample(vec![1, 2, 3]);
        let out = chain.apply(s).unwrap();
        assert_eq!(out.data.len(), 3);
    }

    #[test]
    fn test_chain_short_circuits_on_error() {
        // ValidateSize(5) will fail on 3-byte input → Identity never runs.
        let chain = TransformChain::new(vec![Box::new(ValidateSize::new(5)), Box::new(Identity)]);
        let s = make_sample(vec![1, 2, 3]);
        assert!(chain.apply(s).is_err());
    }
}
