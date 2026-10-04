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

// ── Sentence embeddings ─────────────────────────────────────────────────────────

/// Configuration for loading a sentence-embedding encoder (E5 / MiniLM / BGE family).
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// ONNX export with a `last_hidden_state` (`[batch, seq, dim]`) or already pooled
    /// (`[batch, dim]`) first output (`optimum-cli export onnx --task feature-extraction`).
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    /// Model position limit (E5: 512).
    pub max_len: usize,
    /// 2 = `input_ids`,`attention_mask`; 3 additionally feeds zero `token_type_ids` (BERT).
    pub num_inputs: usize,
    /// Text prepended to queries (E5 expects `"query: "`). Empty for none.
    pub query_prefix: String,
    /// Text prepended to indexed passages (E5 expects `"passage: "`). Empty for none.
    pub passage_prefix: String,
}

/// Which instruction prefix an input gets (E5-style asymmetric retrieval).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    Query,
    Passage,
}

/// A Tract-backed sentence encoder: mean-pooled, L2-normalised embeddings, so the cosine
/// similarity of two embeddings is their dot product. Thread-safe; batches run in parallel.
pub struct TextEncoder {
    model: Runnable,
    raw_tokenizer: Tokenizer,
    special_prefix: Vec<i64>,
    special_suffix: Vec<i64>,
    query_prefix: Vec<i64>,
    passage_prefix: Vec<i64>,
    max_len: usize,
    num_inputs: usize,
    dim: usize,
}

impl TextEncoder {
    /// Load the encoder, then probe it once to learn the embedding dimension.
    pub fn load(cfg: &EncoderConfig) -> Result<Self> {
        let model = load_runnable(&cfg.model_path)?;
        let mut raw_tokenizer =
            Tokenizer::from_file(&cfg.tokenizer_path).map_err(|e| Error::ModelLoadFailed {
                reason: format!("tokenizer '{}': {e}", cfg.tokenizer_path.display()),
            })?;
        raw_tokenizer
            .with_truncation(None)
            .map_err(|e| Error::ModelLoadFailed {
                reason: format!("tokenizer truncation: {e}"),
            })?;
        raw_tokenizer.with_padding(None);

        let (special_prefix, special_suffix) = detect_special_tokens(&raw_tokenizer)?;
        let prefix_ids = |p: &str| -> Result<Vec<i64>> {
            if p.trim().is_empty() {
                return Ok(Vec::new());
            }
            Ok(raw_tokenizer
                .encode(p.trim_end(), false)
                .map_err(|e| Error::ModelLoadFailed {
                    reason: format!("prefix tokenize: {e}"),
                })?
                .get_ids()
                .iter()
                .map(|&x| x as i64)
                .collect())
        };
        let query_prefix = prefix_ids(&cfg.query_prefix)?;
        let passage_prefix = prefix_ids(&cfg.passage_prefix)?;

        let mut enc = Self {
            model,
            raw_tokenizer,
            special_prefix,
            special_suffix,
            query_prefix,
            passage_prefix,
            max_len: cfg.max_len.max(8),
            num_inputs: cfg.num_inputs.max(2),
            dim: 0,
        };
        let probe = enc.embed("dimension probe", EmbedKind::Query)?;
        enc.dim = probe.len();
        if enc.dim == 0 {
            return Err(Error::ModelLoadFailed {
                reason: "encoder produced an empty embedding".to_string(),
            });
        }
        tracing::info!(
            model = %cfg.model_path.display(),
            dim = enc.dim,
            max_len = enc.max_len,
            num_inputs = enc.num_inputs,
            "Loaded text encoder via Tract"
        );
        Ok(enc)
    }

    /// Embedding dimension.
    pub fn dim(&self) -> usize {
        self.dim
    }

    fn kind_prefix(&self, kind: EmbedKind) -> &[i64] {
        match kind {
            EmbedKind::Query => &self.query_prefix,
            EmbedKind::Passage => &self.passage_prefix,
        }
    }

    fn wrap(&self, kind: EmbedKind, content: &[i64]) -> Vec<i64> {
        let p = self.kind_prefix(kind);
        let mut ids = Vec::with_capacity(
            self.special_prefix.len() + p.len() + content.len() + self.special_suffix.len(),
        );
        ids.extend_from_slice(&self.special_prefix);
        ids.extend_from_slice(p);
        ids.extend_from_slice(content);
        ids.extend_from_slice(&self.special_suffix);
        ids
    }

    fn content_ids(&self, text: &str) -> Result<Vec<i64>> {
        Ok(self
            .raw_tokenizer
            .encode(text, false)
            .map_err(|e| Error::InferenceFailed {
                reason: format!("tokenize: {e}"),
            })?
            .get_ids()
            .iter()
            .map(|&x| x as i64)
            .collect())
    }

    /// Embed one text, truncated to the model limit (use for short passages / exemplars).
    pub fn embed(&self, text: &str, kind: EmbedKind) -> Result<Vec<f32>> {
        let mut content = self.content_ids(text)?;
        let room = self.max_len.saturating_sub(
            self.special_prefix.len() + self.special_suffix.len() + self.kind_prefix(kind).len(),
        );
        content.truncate(room.max(1));
        self.run_pooled(self.wrap(kind, &content))
    }

    /// Split `text` into overlapping model-ready windows (same planner as the classifier).
    /// The instruction prefix is repeated in every window.
    pub fn encode_windows(
        &self,
        text: &str,
        kind: EmbedKind,
        opts: &WindowOptions,
    ) -> Result<TokenWindows> {
        let content = self.content_ids(text)?;
        let n = content.len();
        let fixed =
            self.special_prefix.len() + self.special_suffix.len() + self.kind_prefix(kind).len();
        let window = opts.window.min(self.max_len).max(fixed + 1);
        let content_len = window - fixed;
        let total_windows = plan_windows(n, content_len, opts.overlap, usize::MAX)
            .len()
            .max(1);
        let planned = plan_windows(n, content_len, opts.overlap, opts.max_windows);
        let windows = if planned.is_empty() {
            vec![self.wrap(kind, &[])]
        } else {
            planned
                .iter()
                .map(|&(s, e)| self.wrap(kind, &content[s..e]))
                .collect()
        };
        Ok(TokenWindows {
            windows,
            total_tokens: n,
            total_windows,
        })
    }

    /// Embed many prepared windows in parallel (input order preserved).
    pub fn embed_ids_batch(&self, windows: &[Vec<i64>]) -> Result<Vec<Vec<f32>>> {
        use rayon::prelude::*;
        windows
            .par_iter()
            .map(|ids| self.run_pooled(ids.clone()))
            .collect()
    }

    /// Embed many short texts in parallel (each truncated to the model limit).
    pub fn embed_batch(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
        use rayon::prelude::*;
        texts.par_iter().map(|t| self.embed(t, kind)).collect()
    }

    fn run_pooled(&self, ids: Vec<i64>) -> Result<Vec<f32>> {
        let seq = ids.len();
        if seq == 0 {
            return Err(Error::InferenceFailed {
                reason: "empty token sequence".to_string(),
            });
        }
        let mask = vec![1i64; seq];
        let inputs = build_inputs(self.num_inputs, ids, mask)?;
        let outputs = self.model.run(inputs).map_err(|e| Error::InferenceFailed {
            reason: format!("tract run: {e}"),
        })?;
        let out = outputs
            .first()
            .ok_or_else(|| Error::InferenceFailed {
                reason: "model produced no outputs".to_string(),
            })?
            .to_array_view::<f32>()
            .map_err(|e| Error::InferenceFailed {
                reason: format!("extract hidden states: {e}"),
            })?;

        let shape = out.shape().to_vec();
        let mut pooled = match shape.len() {
            // [1, seq, dim]: mean over tokens (no padding, so every position counts).
            3 => {
                let (tokens, dim) = (shape[1].max(1), shape[2]);
                let mut acc = vec![0f32; dim];
                for (i, v) in out.iter().enumerate() {
                    acc[i % dim] += *v;
                }
                for a in &mut acc {
                    *a /= tokens as f32;
                }
                acc
            }
            // [1, dim]: already pooled by the export.
            2 => out.iter().copied().collect(),
            _ => {
                return Err(Error::InferenceFailed {
                    reason: format!("unexpected encoder output shape {shape:?}"),
                })
            }
        };
        l2_normalize(&mut pooled);
        Ok(pooled)
    }
}

/// Scale `v` to unit length (no-op for the zero vector).
pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Dot product of two equal-length vectors (cosine similarity for normalised embeddings).
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn load_runnable(path: &std::path::Path) -> Result<Runnable> {
    let model = tract_onnx::onnx()
        .model_for_path(path)
        .map_err(|e| Error::ModelLoadFailed {
            reason: format!("tract load '{}': {e}", path.display()),
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
    Ok(Arc::new(model))
}

/// `[1, seq]` int64 inputs fed by position: input_ids, attention_mask, token_type_ids?.
fn build_inputs(num_inputs: usize, ids: Vec<i64>, mask: Vec<i64>) -> Result<TVec<TValue>> {
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
    Ok(if num_inputs >= 3 {
        let token_type: Tensor = Array2::<i64>::zeros((1, seq)).into_dyn().into();
        tvec![ids_t.into(), mask_t.into(), token_type.into()]
    } else {
        tvec![ids_t.into(), mask_t.into()]
    })
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
    use super::{dot, l2_normalize, plan_windows};

    #[test]
    fn normalised_vectors_dot_is_cosine() {
        let mut a = vec![3.0, 4.0];
        l2_normalize(&mut a);
        assert!((a[0] - 0.6).abs() < 1e-6 && (a[1] - 0.8).abs() < 1e-6);
        assert!((dot(&a, &a) - 1.0).abs() < 1e-6);
        let mut z = vec![0.0, 0.0];
        l2_normalize(&mut z);
        assert_eq!(z, vec![0.0, 0.0]);
    }

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
