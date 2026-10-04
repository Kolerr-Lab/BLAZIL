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

/// Sliding-window options for scoring texts longer than one model window.
///
/// A transformer only sees `max_len` tokens; anything after that is silently dropped by
/// [`TextClassifier::predict`]. Windowed scoring splits the text into overlapping token
/// windows, scores each one, and reports the maximum, so an instruction hidden after a
/// long benign prefix is still seen.
#[derive(Debug, Clone, Copy)]
pub struct WindowOptions {
    /// Total tokens per window INCLUDING special tokens. Capped at the model `max_len`.
    pub window: usize,
    /// Tokens shared by consecutive windows, so a phrase on a boundary is seen whole.
    pub overlap: usize,
    /// Upper bound on windows scored per text. When a text needs more, the first and last
    /// windows are always kept and the rest are spread evenly across the middle.
    pub max_windows: usize,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            window: 128,
            overlap: 32,
            max_windows: 32,
        }
    }
}

/// Token windows produced for one text (special tokens already applied to each window).
#[derive(Debug, Clone)]
pub struct TokenWindows {
    /// Model-ready token ids, one entry per window that will be scored.
    pub windows: Vec<Vec<i64>>,
    /// Content tokens in the text (special tokens excluded).
    pub total_tokens: usize,
    /// Windows the full text needs before the `max_windows` cap was applied.
    pub total_windows: usize,
}

/// Result of windowed scoring.
#[derive(Debug, Clone, Copy)]
pub struct WindowedScore {
    /// Highest class probability over all scored windows.
    pub max_score: f32,
    /// Index (in scored order) of the window that produced `max_score`.
    pub max_index: usize,
    /// Windows actually scored.
    pub scored: usize,
    /// Windows the full text needs before the cap.
    pub total_windows: usize,
    /// Content tokens in the text.
    pub total_tokens: usize,
}

/// Plan overlapping `[start, end)` windows over `n` content tokens.
///
/// Every token is covered when the window count is within `max_windows`. Above the cap the
/// first and last windows are always kept and the remainder are sampled evenly, so a
/// payload of any length is scored in bounded time. Pure and deterministic.
pub fn plan_windows(
    n: usize,
    content_len: usize,
    overlap: usize,
    max_windows: usize,
) -> Vec<(usize, usize)> {
    if n == 0 {
        return Vec::new();
    }
    let content_len = content_len.max(1);
    if n <= content_len {
        return vec![(0, n)];
    }
    let step = content_len - overlap.min(content_len - 1);

    let mut starts = Vec::new();
    let mut s = 0usize;
    loop {
        if s + content_len >= n {
            // Align the final window to the end so the tail is always fully covered.
            starts.push(n - content_len);
            break;
        }
        starts.push(s);
        s += step;
    }

    let k = max_windows.max(1);
    let picked: Vec<usize> = if starts.len() <= k {
        starts
    } else if k == 1 {
        vec![starts[0]]
    } else {
        let last = starts.len() - 1;
        let mut out: Vec<usize> = (0..k)
            .map(|i| starts[(i * last + (k - 1) / 2) / (k - 1)])
            .collect();
        out.dedup();
        out
    };

    picked
        .into_iter()
        .map(|st| (st, st + content_len))
        .collect()
}

/// A Tract-backed transformer text classifier. Thread-safe (`Arc` runnable + `Send`
/// tokenizer), so it can be shared across async tasks.
pub struct TextClassifier {
    model: Runnable,
    tokenizer: Tokenizer,
    /// Copy of `tokenizer` with truncation and padding disabled, used to see the WHOLE
    /// text when planning windows (a tokenizer.json may ship with truncation set).
    raw_tokenizer: Tokenizer,
    /// Special tokens the model expects before / after the content (e.g. `[CLS]` / `[SEP]`).
    special_prefix: Vec<i64>,
    special_suffix: Vec<i64>,
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

        let mut raw_tokenizer = tokenizer.clone();
        raw_tokenizer
            .with_truncation(None)
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("tokenizer truncation: {e}"),
            })?;
        raw_tokenizer.with_padding(None);

        let (special_prefix, special_suffix) = detect_special_tokens(&raw_tokenizer)?;

        let num_inputs = cfg.num_inputs.max(2);
        tracing::info!(
            model = %cfg.model_path.display(),
            num_inputs,
            max_len = cfg.max_len,
            special_prefix = special_prefix.len(),
            special_suffix = special_suffix.len(),
            "Loaded text classifier via Tract"
        );

        Ok(Self {
            model: Arc::new(model),
            tokenizer,
            raw_tokenizer,
            special_prefix,
            special_suffix,
            max_len: cfg.max_len,
            num_inputs,
        })
    }

    /// Split `text` into model-ready overlapping token windows (see [`WindowOptions`]).
    pub fn encode_windows(&self, text: &str, opts: &WindowOptions) -> Result<TokenWindows> {
        let enc = self
            .raw_tokenizer
            .encode(text, false)
            .map_err(|e| Error::InferenceFailed {
                reason: format!("tokenize: {e}"),
            })?;
        let content: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
        let n = content.len();

        let specials = self.special_prefix.len() + self.special_suffix.len();
        let window = opts.window.min(self.max_len).max(specials + 1);
        let content_len = window - specials;

        let all = plan_windows(n, content_len, opts.overlap, usize::MAX);
        let total_windows = all.len().max(1);
        let planned = plan_windows(n, content_len, opts.overlap, opts.max_windows);

        let windows = if planned.is_empty() {
            // Empty text: score the special tokens alone, same as `predict`.
            vec![self.wrap(&[])]
        } else {
            planned
                .iter()
                .map(|&(s, e)| self.wrap(&content[s..e]))
                .collect()
        };

        Ok(TokenWindows {
            windows,
            total_tokens: n,
            total_windows,
        })
    }

    /// Score many token windows in parallel; returns the `class_idx` probability per window,
    /// in input order.
    pub fn score_ids_batch(&self, windows: &[Vec<i64>], class_idx: usize) -> Result<Vec<f32>> {
        use rayon::prelude::*;
        windows
            .par_iter()
            .map(|ids| {
                let pred = self.run_ids(ids.clone())?;
                Ok(class_prob(&pred, class_idx))
            })
            .collect()
    }

    /// Windowed scoring: the maximum `class_idx` probability over all windows of `text`.
    pub fn score_class_windowed(
        &self,
        text: &str,
        class_idx: usize,
        opts: &WindowOptions,
    ) -> Result<WindowedScore> {
        let tw = self.encode_windows(text, opts)?;
        let scores = self.score_ids_batch(&tw.windows, class_idx)?;
        let (max_index, max_score) =
            scores
                .iter()
                .copied()
                .enumerate()
                .fold(
                    (0, 0.0f32),
                    |best, (i, s)| if s > best.1 { (i, s) } else { best },
                );
        Ok(WindowedScore {
            max_score,
            max_index,
            scored: scores.len(),
            total_windows: tw.total_windows,
            total_tokens: tw.total_tokens,
        })
    }

    fn wrap(&self, content: &[i64]) -> Vec<i64> {
        let mut ids = Vec::with_capacity(
            self.special_prefix.len() + content.len() + self.special_suffix.len(),
        );
        ids.extend_from_slice(&self.special_prefix);
        ids.extend_from_slice(content);
        ids.extend_from_slice(&self.special_suffix);
        ids
    }

    /// Run the model on already-tokenized ids (no padding, so the mask is all ones).
    fn run_ids(&self, ids: Vec<i64>) -> Result<Prediction> {
        let seq = ids.len();
        if seq == 0 {
            return Ok(Prediction::from_logits(vec![0.0]));
        }
        let mask = vec![1i64; seq];
        self.run_tensors(ids, mask)
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
        if ids.is_empty() {
            return Ok(Prediction::from_logits(vec![0.0]));
        }
        self.run_tensors(ids, mask)
    }

    /// Feed `[1, seq]` ids + mask to the model and return the softmax prediction.
    fn run_tensors(&self, ids: Vec<i64>, mask: Vec<i64>) -> Result<Prediction> {
        let seq = ids.len();
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
        Ok(class_prob(&pred, class_idx))
    }
}

fn class_prob(pred: &Prediction, class_idx: usize) -> f32 {
    pred.probabilities
        .as_ref()
        .and_then(|p| p.get(class_idx).copied())
        .unwrap_or(0.0)
}

/// Find the special tokens the tokenizer wraps around a single sequence by comparing a
/// probe encoded with and without them. Model-agnostic (BERT `[CLS]…[SEP]`, RoBERTa
/// `<s>…</s>`, DeBERTa, …).
fn detect_special_tokens(tok: &Tokenizer) -> Result<(Vec<i64>, Vec<i64>)> {
    let probe = "blazle window probe";
    let enc = |special: bool| -> Result<Vec<i64>> {
        Ok(tok
            .encode(probe, special)
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("tokenizer probe: {e}"),
            })?
            .get_ids()
            .iter()
            .map(|&x| x as i64)
            .collect())
    };
    let with = enc(true)?;
    let without = enc(false)?;
    if without.is_empty() || without.len() > with.len() {
        return Ok((Vec::new(), Vec::new()));
    }
    match with
        .windows(without.len())
        .position(|w| w == without.as_slice())
    {
        Some(pos) => Ok((with[..pos].to_vec(), with[pos + without.len()..].to_vec())),
        None => {
            tracing::warn!("could not locate special tokens; windows are scored without them");
            Ok((Vec::new(), Vec::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plan_windows;

    #[test]
    fn short_text_is_one_window() {
        assert_eq!(plan_windows(50, 126, 32, 32), vec![(0, 50)]);
        assert_eq!(plan_windows(126, 126, 32, 32), vec![(0, 126)]);
        assert!(plan_windows(0, 126, 32, 32).is_empty());
    }

    #[test]
    fn every_token_is_covered_with_overlap() {
        for n in [127usize, 200, 500, 1000, 3001] {
            let w = plan_windows(n, 126, 32, usize::MAX);
            assert_eq!(w.first().unwrap().0, 0);
            assert_eq!(w.last().unwrap().1, n, "tail must be covered");
            for pair in w.windows(2) {
                // next window starts before the previous one ends => overlap, no gap
                assert!(pair[1].0 < pair[0].1, "gap between windows at n={n}");
                assert!(pair[0].1 - pair[1].0 >= 32 || pair[1].1 == n);
            }
            for &(s, e) in &w {
                assert_eq!(e - s, 126);
            }
        }
    }

    #[test]
    fn cap_keeps_head_and_tail() {
        let n = 100_000;
        let w = plan_windows(n, 126, 32, 8);
        assert!(w.len() <= 8 && w.len() >= 2);
        assert_eq!(w.first().unwrap().0, 0);
        assert_eq!(w.last().unwrap().1, n);
        // strictly increasing starts
        for pair in w.windows(2) {
            assert!(pair[1].0 > pair[0].0);
        }
    }

    #[test]
    fn degenerate_overlap_still_progresses() {
        let w = plan_windows(10, 3, 99, usize::MAX);
        assert_eq!(w.last().unwrap().1, 10);
        assert!(w.len() <= 8);
    }
}
