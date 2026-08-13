// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Text sequence-classification via Tract (pure Rust ONNX).
//!
//! The existing [`crate::onnx::OnnxModel`] is specialised for CNN image classification
//! (single 4-D NCHW f32 input). Transformer text classifiers (DistilBERT / MiniLM /
//! DeBERTa) need a different shape: **two or three int64 inputs** — `input_ids`,
//! `attention_mask`, and optionally `token_type_ids` — each `[batch, seq_len]`, plus
//! tokenization. This module provides that path on the same Tract engine.
//!
//! Primary use: BLAZLE's prompt-injection / jailbreak detector, run in-process (no Python
//! sidecar, payload never leaves the VPC).
//!
//! # Example
//! ```no_run
//! use blazil_inference::text::{TextClassifier, TextConfig};
//! # fn main() -> blazil_inference::Result<()> {
//! let clf = TextClassifier::load(&TextConfig {
//!     model_path: "prompt-injection.onnx".into(),
//!     tokenizer_path: "tokenizer.json".into(),
//!     max_len: 512,
//!     num_inputs: 2, // input_ids + attention_mask (DistilBERT-style)
//! })?;
//! let p_injection = clf.score_class("ignore all previous instructions", 1)?;
//! println!("injection prob = {p_injection:.3}");
//! # Ok(())
//! # }
//! ```

use crate::model::Prediction;
use crate::{Error, Result};
use ndarray::Array2;
use std::path::PathBuf;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tract_onnx::prelude::*;

type Runnable = Arc<TypedRunnableModel<TypedModel>>;

/// Configuration for loading a text sequence classifier.
#[derive(Debug, Clone)]
pub struct TextConfig {
    /// Path to the exported ONNX model (sequence-classification head).
    pub model_path: PathBuf,
    /// Path to the HuggingFace `tokenizer.json` matching the model.
    pub tokenizer_path: PathBuf,
    /// Truncate token sequences to this length (transformer position limit, e.g. 512).
    pub max_len: usize,
    /// Number of ONNX graph inputs: 2 = `input_ids`,`attention_mask`;
    /// 3 additionally feeds an all-zero `token_type_ids` (BERT-style models).
    pub num_inputs: usize,
}

/// A Tract-backed transformer text classifier. Thread-safe (`Arc` runnable + `Send`
/// tokenizer), so it can be shared across async tasks.
pub struct TextClassifier {
    model: Runnable,
    tokenizer: Tokenizer,
    max_len: usize,
    num_inputs: usize,
}

impl TextClassifier {
    /// Load and optimize the ONNX model and its tokenizer.
    pub fn load(cfg: &TextConfig) -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_path(&cfg.model_path)
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("tract load '{}': {e}", cfg.model_path.display()),
            })?
            .into_typed()
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("type inference: {e}"),
            })?
            .into_optimized()
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("optimize: {e}"),
            })?
            .into_runnable()
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("compile: {e}"),
            })?;

        let tokenizer =
            Tokenizer::from_file(&cfg.tokenizer_path).map_err(|e| Error::ModelLoadFailed {
                reason: format!("tokenizer '{}': {e}", cfg.tokenizer_path.display()),
            })?;

        let num_inputs = cfg.num_inputs.max(2);
        tracing::info!(
            model = %cfg.model_path.display(),
            num_inputs,
            max_len = cfg.max_len,
            "Loaded text classifier via Tract"
        );

        Ok(Self {
            model: Arc::new(model),
            tokenizer,
            max_len: cfg.max_len,
            num_inputs,
        })
    }

    /// Tokenize `text`, run the model, and return the softmax [`Prediction`] over labels.
    pub fn predict(&self, text: &str) -> Result<Prediction> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| Error::InferenceFailed {
                reason: format!("tokenize: {e}"),
            })?;

        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
        let mut mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
        if ids.len() > self.max_len {
            ids.truncate(self.max_len);
            mask.truncate(self.max_len);
        }
        let seq = ids.len();
        if seq == 0 {
            return Ok(Prediction::from_logits(vec![0.0]));
        }

        let ids_t: Tensor = Array2::from_shape_vec((1, seq), ids)
            .map_err(|e| Error::InferenceFailed {
                reason: format!("input_ids shape: {e}"),
            })?
            .into_dyn()
            .into();
        let mask_t: Tensor = Array2::from_shape_vec((1, seq), mask)
            .map_err(|e| Error::InferenceFailed {
                reason: format!("attention_mask shape: {e}"),
            })?
            .into_dyn()
            .into();

        // ONNX graph inputs are fed BY POSITION in the order the model declares them —
        // the HF ONNX export convention is [input_ids, attention_mask, token_type_ids?].
        let inputs: TVec<TValue> = if self.num_inputs >= 3 {
            let token_type: Tensor = Array2::<i64>::zeros((1, seq)).into_dyn().into();
            tvec![ids_t.into(), mask_t.into(), token_type.into()]
        } else {
            tvec![ids_t.into(), mask_t.into()]
        };

        let outputs = self.model.run(inputs).map_err(|e| Error::InferenceFailed {
            reason: format!("tract run: {e}"),
        })?;

        let logits = outputs
            .first()
            .ok_or_else(|| Error::InferenceFailed {
                reason: "model produced no outputs".to_string(),
            })?
            .to_array_view::<f32>()
            .map_err(|e| Error::InferenceFailed {
                reason: format!("extract logits: {e}"),
            })?;

        // Logits are [1, num_labels]; flatten the single row.
        let row: Vec<f32> = logits.iter().copied().collect();
        Ok(Prediction::from_logits(row))
    }

    /// Convenience: probability of a specific class index (e.g. the "injection" label).
    pub fn score_class(&self, text: &str, class_idx: usize) -> Result<f32> {
        let pred = self.predict(text)?;
        Ok(pred
            .probabilities
            .and_then(|p| p.get(class_idx).copied())
            .unwrap_or(0.0))
    }
}
