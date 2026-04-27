// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Core inference model trait and prediction types.

use crate::{config::InferenceConfig, Result};
use blazil_dataloader::Sample;
use serde::{Deserialize, Serialize};

/// A single prediction output from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    /// Predicted class index (for classification tasks).
    pub class_id: Option<u32>,

    /// Class probabilities (softmax output for classification).
    pub probabilities: Option<Vec<f32>>,

    /// Raw model output (for regression or custom tasks).
    pub raw_output: Vec<f32>,

    /// Confidence score (0.0–1.0, highest probability for classification).
    pub confidence: f32,

    /// Optional metadata (e.g., input sample index, timing).
    pub metadata: Option<serde_json::Value>,
}

impl Prediction {
    /// Create a prediction from raw logits (classification).
    /// Applies softmax and extracts the top-1 class.
    pub fn from_logits(logits: Vec<f32>) -> Self {
        let probabilities = softmax(&logits);
        let (class_id, confidence) = probabilities
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(i, &p)| (i as u32, p))
            .unwrap_or((0, 0.0));

        Self {
            class_id: Some(class_id),
            probabilities: Some(probabilities.clone()),
            raw_output: logits,
            confidence,
            metadata: None,
        }
    }

    /// Create a prediction from raw regression output.
    pub fn from_regression(output: Vec<f32>) -> Self {
        Self {
            class_id: None,
            probabilities: None,
            raw_output: output.clone(),
            confidence: 1.0, // regression has no inherent confidence
            metadata: None,
        }
    }
}

/// Apply softmax to a vector of logits.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|&e| e / sum).collect()
}

/// Inference model trait — abstraction over different backends.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// async tasks and threads.
pub trait InferenceModel: Send + Sync {
    /// Load a model from the given configuration.
    fn load(config: InferenceConfig) -> Result<Self>
    where
        Self: Sized;

    /// Run inference on a batch of preprocessed samples.
    ///
    /// The input `samples` must have been preprocessed to match the model's
    /// expected input shape (e.g., normalized, resized to 224×224 for ImageNet).
    ///
    /// Returns predictions in the same order as the input samples.
    fn run_batch(&self, samples: &[Sample]) -> Result<Vec<Prediction>>;

    /// Get the model's expected input shape (batch, channels, height, width).
    fn input_shape(&self) -> (usize, usize, usize, usize);

    /// Get the number of output classes (for classification models).
    fn num_classes(&self) -> Option<usize>;

    /// Get the model configuration.
    fn config(&self) -> &InferenceConfig;
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_softmax() {
        let logits = vec![1.0, 2.0, 3.0];
        let probs = softmax(&logits);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "softmax must sum to 1.0");
        assert!(
            probs[2] > probs[1] && probs[1] > probs[0],
            "softmax preserves order"
        );
    }

    #[test]
    fn test_prediction_from_logits() {
        let logits = vec![0.1, 2.5, 0.8];
        let pred = Prediction::from_logits(logits);
        assert_eq!(pred.class_id, Some(1));
        assert!(pred.confidence > 0.7); // class 1 should dominate
        assert!(pred.probabilities.is_some());
    }

    #[test]
    fn test_prediction_from_regression() {
        let output = vec![2.5, 1.8];
        let pred = Prediction::from_regression(output.clone());
        assert_eq!(pred.class_id, None);
        assert_eq!(pred.probabilities, None);
        assert_eq!(pred.raw_output, output);
        assert_eq!(pred.confidence, 1.0);
    }
}
