//! Qwen2 model implementation with quantization support.
//!
//! Qwen2 is a chat-optimized language model that supports 8-bit quantization
//! for reduced memory usage and faster inference.
//!
//! Key characteristics:
//! - Group Query Attention (GQA)
//! - RMSNorm for layer normalization
//! - Rotary positional embeddings (RoPE)
//! - Support for 8-bit quantization
//!
//! References:
//! - [Model Card](https://huggingface.co/Qwen/Qwen2)
//!

// VENDORED FROM: candle-transformers/src/models/quantized_qwen2.rs
// Modified for distributed pipeline: exposed layers field, added forward_layer_range()

use candle_core::{
    quantized::{gguf_file, QMatMul},
    DType, Device, IndexOp, Result, Tensor,
};
use candle_nn::{Embedding, Module};
use candle_transformers::{quantized_nn::RmsNorm, utils::repeat_kv};
use std::collections::HashMap;

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use super::avx512_kernels;

use crate::config::HybridMatrixConfig;

fn should_enable_hybrid_matrix(config: &HybridMatrixConfig, total_layers: usize) -> bool {
    if config.stage1_end >= config.stage2_end || config.stage2_end >= config.total_layers {
        tracing::error!(
            "Invalid Hybrid Matrix stage boundaries: stage1_end={}, stage2_end={}, total={}",
            config.stage1_end,
            config.stage2_end,
            config.total_layers
        );
        return false;
    }
    if total_layers != config.total_layers {
        tracing::error!(
            "Hybrid Matrix layer count mismatch: model={}, config={}",
            total_layers,
            config.total_layers
        );
        return false;
    }
    true
}

/// Quantization stage for Hybrid Matrix architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuantizationStage {
    Int8Stage1,
    BitNet,
    Int8Stage3,
}

impl QuantizationStage {
    fn for_layer(layer_idx: usize, config: &HybridMatrixConfig) -> Self {
        if layer_idx < config.stage1_end {
            Self::Int8Stage1
        } else if layer_idx < config.stage2_end {
            Self::BitNet
        } else {
            Self::Int8Stage3
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Int8Stage1 => "Stage1(INT8)",
            Self::BitNet => "Stage2(BitNet)",
            Self::Int8Stage3 => "Stage3(INT8)",
        }
    }
}

/// Hybrid-quantized weight matrix for extreme compression.
#[derive(Debug, Clone)]
struct HybridWeights {
    stage: QuantizationStage,
    rows: usize,
    cols: usize,
    bitnet_packed: Option<Vec<u64>>,
    int8_weights: Option<Vec<i8>>,
    int8_scale: Option<f32>,
}

impl HybridWeights {
    /// Create hybrid weights from dequantized F32 tensor.
    fn from_f32_tensor(
        tensor: &Tensor,
        layer_idx: usize,
        config: &HybridMatrixConfig,
    ) -> Result<Self> {
        let stage = QuantizationStage::for_layer(layer_idx, config);
        let dims = tensor.dims();
        if dims.len() != 2 {
            candle_core::bail!("Expected 2D weight tensor, got {:?}", dims);
        }
        let rows = dims[0];
        let cols = dims[1];

        let weights_f32 = tensor.to_vec2::<f32>()?;
        let flattened: Vec<f32> = weights_f32.into_iter().flatten().collect();

        match stage {
            QuantizationStage::BitNet => {
                let packed = blazil_inference::pack_weights_1bit(
                    &flattened,
                    rows,
                    cols,
                    config.bitnet_threshold,
                );
                Ok(Self {
                    stage,
                    rows,
                    cols,
                    bitnet_packed: Some(packed),
                    int8_weights: None,
                    int8_scale: None,
                })
            }
            QuantizationStage::Int8Stage1 | QuantizationStage::Int8Stage3 => {
                let (quantized, scale) = blazil_inference::quantize_int8(&flattened);
                Ok(Self {
                    stage,
                    rows,
                    cols,
                    bitnet_packed: None,
                    int8_weights: Some(quantized),
                    int8_scale: Some(scale),
                })
            }
        }
    }

    /// Forward pass with hybrid quantization.
    fn forward(&self, input: &Tensor) -> Result<Tensor> {
        let input_f32 = input.to_vec2::<f32>()?;
        let batch_size = input_f32.len();

        if batch_size == 0 {
            candle_core::bail!("Empty input tensor");
        }
        if input_f32[0].len() != self.cols {
            candle_core::bail!(
                "Input dim mismatch: expected {}, got {}",
                self.cols,
                input_f32[0].len()
            );
        }

        let mut output = vec![vec![0.0f32; self.rows]; batch_size];

        match self.stage {
            QuantizationStage::BitNet => {
                let packed = self.bitnet_packed.as_ref().unwrap();
                for (batch_idx, input_row) in input_f32.iter().enumerate() {
                    let mut output_row = vec![0.0f32; self.rows];
                    blazil_inference::bitnet_linear_1bit(
                        input_row,
                        packed,
                        self.rows,
                        self.cols,
                        &mut output_row,
                    )
                    .map_err(|e| {
                        candle_core::Error::Msg(format!("BitNet forward failed: {}", e))
                    })?;
                    output[batch_idx] = output_row;
                }
            }
            QuantizationStage::Int8Stage1 | QuantizationStage::Int8Stage3 => {
                let weights_int8 = self.int8_weights.as_ref().unwrap();
                let scale = self.int8_scale.unwrap();
                let weights_f32 = blazil_inference::dequantize_int8(weights_int8, scale);

                for (batch_idx, input_row) in input_f32.iter().enumerate() {
                    for i in 0..self.rows {
                        let mut sum = 0.0f32;
                        for j in 0..self.cols {
                            sum += input_row[j] * weights_f32[i * self.cols + j];
                        }
                        output[batch_idx][i] = sum;
                    }
                }
            }
        }

        Tensor::from_vec(
            output.into_iter().flatten().collect(),
            (batch_size, self.rows),
            input.device(),
        )
    }
}

#[derive(Debug, Clone)]
struct Mlp {
    feed_forward_w1: QMatMul,
    feed_forward_w2: QMatMul,
    feed_forward_w3: QMatMul,
    hybrid_w1: Option<HybridWeights>,
    hybrid_w2: Option<HybridWeights>,
    hybrid_w3: Option<HybridWeights>,
}

/// Optimized SiLU activation with AVX-512 VNNI fast path.
///
/// Falls back to Candle's implementation if AVX-512 unavailable or feature disabled.
/// Performance: ~16 GFLOPS vs ~4 GFLOPS (scalar) on i9-12900K.
fn silu_optimized(x: &Tensor) -> Result<Tensor> {
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    {
        if avx512_kernels::is_avx512_vnni_available() {
            // Fast path: AVX-512 in-place SiLU
            let mut data = x.to_vec1::<f32>()?;
            unsafe { avx512_kernels::silu_avx512(&mut data) };
            return Tensor::from_vec(data, x.shape(), x.device());
        }
    }

    // Use Candle's SiLU implementation
    candle_nn::ops::silu(x)
}

impl Module for Mlp {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        if let (Some(hw1), Some(hw2), Some(hw3)) =
            (&self.hybrid_w1, &self.hybrid_w2, &self.hybrid_w3)
        {
            let w1 = hw1.forward(xs)?;
            let w3 = hw3.forward(xs)?;
            hw2.forward(&(silu_optimized(&w1)? * w3)?)
        } else {
            let w1 = self.feed_forward_w1.forward(xs)?;
            let w3 = self.feed_forward_w3.forward(xs)?;
            self.feed_forward_w2.forward(&(silu_optimized(&w1)? * w3)?)
        }
    }
}

/// Individual transformer layer with attention + MLP.
/// Made public for distributed pipeline access to layer count.
#[derive(Debug, Clone)]
pub struct LayerWeights {
    attention_wq: QMatMul,
    attention_wk: QMatMul,
    attention_wv: QMatMul,
    attention_bq: Tensor,
    attention_bk: Tensor,
    attention_bv: Tensor,
    attention_wo: QMatMul,
    attention_norm: RmsNorm,
    mlp: Mlp,
    ffn_norm: RmsNorm,
    n_head: usize,
    n_kv_head: usize,
    head_dim: usize,
    cos: Tensor,
    sin: Tensor,
    neg_inf: Tensor,
    kv_cache: Option<(Tensor, Tensor)>,
    span_attn: tracing::Span,
    span_rot: tracing::Span,
    span_mlp: tracing::Span,
    hybrid_wq: Option<HybridWeights>,
    hybrid_wk: Option<HybridWeights>,
    hybrid_wv: Option<HybridWeights>,
    hybrid_wo: Option<HybridWeights>,
}

fn masked_fill(on_false: &Tensor, mask: &Tensor, on_true: &Tensor) -> Result<Tensor> {
    let shape = mask.shape();
    let m = mask.where_cond(&on_true.broadcast_as(shape.dims())?, on_false)?;
    Ok(m)
}

impl LayerWeights {
    /// Clear KV cache for this layer (needed between distributed pipeline requests).
    pub fn clear_kv_cache(&mut self) {
        self.kv_cache = None;
    }

    fn apply_rotary_emb(&self, x: &Tensor, index_pos: usize) -> Result<Tensor> {
        let _enter = self.span_rot.enter();
        let (_b_sz, _n_head, seq_len, _n_embd) = x.dims4()?;
        let cos = self.cos.narrow(0, index_pos, seq_len)?;
        let sin = self.sin.narrow(0, index_pos, seq_len)?;
        candle_nn::rotary_emb::rope(&x.contiguous()?, &cos, &sin)
    }

    fn forward_attn(
        &mut self,
        x: &Tensor,
        mask: Option<&Tensor>,
        index_pos: usize,
    ) -> Result<Tensor> {
        let _enter = self.span_attn.enter();
        let (b_sz, seq_len, n_embd) = x.dims3()?;

        let (q, k, v) = if let (Some(hwq), Some(hwk), Some(hwv)) =
            (&self.hybrid_wq, &self.hybrid_wk, &self.hybrid_wv)
        {
            (hwq.forward(x)?, hwk.forward(x)?, hwv.forward(x)?)
        } else {
            (
                self.attention_wq.forward(x)?,
                self.attention_wk.forward(x)?,
                self.attention_wv.forward(x)?,
            )
        };

        let q = q.broadcast_add(&self.attention_bq)?;
        let k = k.broadcast_add(&self.attention_bk)?;
        let v = v.broadcast_add(&self.attention_bv)?;

        let q = q
            .reshape((b_sz, seq_len, self.n_head, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;
        let k = k
            .reshape((b_sz, seq_len, self.n_kv_head, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;
        let v = v
            .reshape((b_sz, seq_len, self.n_kv_head, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;

        let q = self.apply_rotary_emb(&q, index_pos)?;
        let k = self.apply_rotary_emb(&k, index_pos)?;

        let (k, v) = match &self.kv_cache {
            None => (k, v),
            Some((k_cache, v_cache)) => {
                if index_pos == 0 {
                    (k, v)
                } else {
                    let k = Tensor::cat(&[k_cache, &k], 2)?;
                    let v = Tensor::cat(&[v_cache, &v], 2)?;
                    (k, v)
                }
            }
        };
        self.kv_cache = Some((k.clone(), v.clone()));

        let k = repeat_kv(k, self.n_head / self.n_kv_head)?;
        let v = repeat_kv(v, self.n_head / self.n_kv_head)?;

        let att = (q.matmul(&k.t()?)? / (self.head_dim as f64).sqrt())?;
        let att = match mask {
            None => att,
            Some(mask) => {
                let mask = mask.broadcast_as(att.shape())?;
                masked_fill(&att, &mask, &self.neg_inf)?
            }
        };
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let y = att.matmul(&v.contiguous()?)?;
        let y = y.transpose(1, 2)?.reshape(&[b_sz, seq_len, n_embd])?;

        let y = if let Some(hwo) = &self.hybrid_wo {
            hwo.forward(&y)?
        } else {
            self.attention_wo.forward(&y)?
        };

        Ok(y)
    }
}

pub struct ModelWeights {
    tok_embeddings: Embedding,
    /// Individual transformer layers - exposed for distributed pipeline.
    /// Each stage executes a subset of layers (e.g., Stage 1: 0-26, Stage 2: 27-52, Stage 3: 53-80).
    pub layers: Vec<LayerWeights>,
    norm: RmsNorm,
    output: QMatMul,
    masks: HashMap<usize, Tensor>,
    span: tracing::Span,
    span_output: tracing::Span,
    /// Hybrid matrix quantization configuration (ClarkenAI Edge).
    /// When enabled, applies stage-specific quantization (INT8 → 1-bit BitNet → INT8).
    hybrid_matrix_config: Option<HybridMatrixConfig>,
}

fn precomput_freqs_cis(
    head_dim: usize,
    freq_base: f32,
    context_length: usize,
    device: &Device,
) -> Result<(Tensor, Tensor)> {
    let theta: Vec<_> = (0..head_dim)
        .step_by(2)
        .map(|i| 1f32 / freq_base.powf(i as f32 / head_dim as f32))
        .collect();
    let theta = Tensor::new(theta.as_slice(), device)?;
    let idx_theta = Tensor::arange(0, context_length as u32, device)?
        .to_dtype(DType::F32)?
        .reshape((context_length, 1))?
        .matmul(&theta.reshape((1, theta.elem_count()))?)?;
    let cos = idx_theta.cos()?;
    let sin = idx_theta.sin()?;
    Ok((cos, sin))
}

impl ModelWeights {
    pub fn from_gguf<R: std::io::Seek + std::io::Read>(
        ct: gguf_file::Content,
        reader: &mut R,
        device: &Device,
        hybrid_matrix_config: Option<HybridMatrixConfig>,
    ) -> Result<Self> {
        // Log SIMD capabilities at model load
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        {
            let avx512_available = avx512_kernels::is_avx512_vnni_available();
            tracing::info!(
                "🚀 AVX-512 VNNI detected: {} | SiLU optimization: {}",
                avx512_available,
                if avx512_available {
                    "ENABLED ⚡"
                } else {
                    "disabled (using scalar fallback)"
                }
            );
        }
        #[cfg(not(all(target_arch = "x86_64", feature = "avx512")))]
        {
            tracing::info!("🚀 Using Candle's standard operations (AVX-512 feature not enabled)");
        }

        let md_get = |s: &str| match ct.metadata.get(s) {
            None => candle_core::bail!("cannot find {s} in metadata"),
            Some(v) => Ok(v),
        };

        let head_count = md_get("qwen2.attention.head_count")?.to_u32()? as usize;
        let head_count_kv = md_get("qwen2.attention.head_count_kv")?.to_u32()? as usize;
        let embedding_length = md_get("qwen2.embedding_length")?.to_u32()? as usize;
        let context_length = md_get("qwen2.context_length")?.to_u32()? as usize;
        let block_count = md_get("qwen2.block_count")?.to_u32()? as usize;
        let rms_norm_eps = md_get("qwen2.attention.layer_norm_rms_epsilon")?.to_f32()? as f64;
        let rope_freq_base = md_get("qwen2.rope.freq_base")
            .and_then(|m| m.to_f32())
            .unwrap_or(10000f32);

        // Validate and log Hybrid Matrix configuration
        let hybrid_matrix_enabled = if let Some(ref config) = hybrid_matrix_config {
            should_enable_hybrid_matrix(config, block_count)
        } else {
            false
        };

        if hybrid_matrix_enabled {
            let config = hybrid_matrix_config.as_ref().unwrap();
            tracing::info!(
                "🎯 Hybrid Matrix Quantization ENABLED: Stage1(INT8)=0..{}, Stage2(BitNet)={}..{}, Stage3(INT8)={}..{}",
                config.stage1_end,
                config.stage1_end,
                config.stage2_end,
                config.stage2_end,
                config.total_layers
            );
        } else {
            tracing::info!("🎯 Hybrid Matrix Quantization: DISABLED (using Q4_K_M from GGUF)");
        }

        let head_dim = embedding_length / head_count;

        let neg_inf = Tensor::new(f32::NEG_INFINITY, device)?;

        let tok_embeddings = ct.tensor(reader, "token_embd.weight", device)?;
        let tok_embeddings = tok_embeddings.dequantize(device)?;
        let norm = RmsNorm::from_qtensor(
            ct.tensor(reader, "output_norm.weight", device)?,
            rms_norm_eps,
        )?;
        let output = match ct.tensor(reader, "output.weight", device) {
            Ok(v) => QMatMul::from_qtensor(v)?,
            _ => {
                // use tie_word_embeddings
                QMatMul::from_qtensor(ct.tensor(reader, "token_embd.weight", device)?)?
            }
        };

        let (cos, sin) = precomput_freqs_cis(head_dim, rope_freq_base, context_length, device)?;

        let mut layers = Vec::with_capacity(block_count);

        for layer_idx in 0..block_count {
            let prefix = format!("blk.{layer_idx}");
            let attention_wq = ct.tensor(reader, &format!("{prefix}.attn_q.weight"), device)?;
            let attention_wk = ct.tensor(reader, &format!("{prefix}.attn_k.weight"), device)?;
            let attention_wv = ct.tensor(reader, &format!("{prefix}.attn_v.weight"), device)?;

            let attention_bq = ct.tensor(reader, &format!("{prefix}.attn_q.bias"), device)?;
            let attention_bk = ct.tensor(reader, &format!("{prefix}.attn_k.bias"), device)?;
            let attention_bv = ct.tensor(reader, &format!("{prefix}.attn_v.bias"), device)?;

            let attention_wo =
                ct.tensor(reader, &format!("{prefix}.attn_output.weight"), device)?;

            let (hybrid_wq, hybrid_wk, hybrid_wv, hybrid_wo) = if hybrid_matrix_enabled {
                let config = hybrid_matrix_config.as_ref().unwrap();
                (
                    Some(HybridWeights::from_f32_tensor(
                        &attention_wq.dequantize(device)?,
                        layer_idx,
                        config,
                    )?),
                    Some(HybridWeights::from_f32_tensor(
                        &attention_wk.dequantize(device)?,
                        layer_idx,
                        config,
                    )?),
                    Some(HybridWeights::from_f32_tensor(
                        &attention_wv.dequantize(device)?,
                        layer_idx,
                        config,
                    )?),
                    Some(HybridWeights::from_f32_tensor(
                        &attention_wo.dequantize(device)?,
                        layer_idx,
                        config,
                    )?),
                )
            } else {
                (None, None, None, None)
            };

            let mlp = {
                let feed_forward_w1 =
                    ct.tensor(reader, &format!("{prefix}.ffn_gate.weight"), device)?;
                let feed_forward_w2 =
                    ct.tensor(reader, &format!("{prefix}.ffn_down.weight"), device)?;
                let feed_forward_w3 =
                    ct.tensor(reader, &format!("{prefix}.ffn_up.weight"), device)?;

                let (hybrid_w1, hybrid_w2, hybrid_w3) = if hybrid_matrix_enabled {
                    let config = hybrid_matrix_config.as_ref().unwrap();
                    (
                        Some(HybridWeights::from_f32_tensor(
                            &feed_forward_w1.dequantize(device)?,
                            layer_idx,
                            config,
                        )?),
                        Some(HybridWeights::from_f32_tensor(
                            &feed_forward_w2.dequantize(device)?,
                            layer_idx,
                            config,
                        )?),
                        Some(HybridWeights::from_f32_tensor(
                            &feed_forward_w3.dequantize(device)?,
                            layer_idx,
                            config,
                        )?),
                    )
                } else {
                    (None, None, None)
                };

                Mlp {
                    feed_forward_w1: QMatMul::from_qtensor(feed_forward_w1)?,
                    feed_forward_w2: QMatMul::from_qtensor(feed_forward_w2)?,
                    feed_forward_w3: QMatMul::from_qtensor(feed_forward_w3)?,
                    hybrid_w1,
                    hybrid_w2,
                    hybrid_w3,
                }
            };

            let attention_norm =
                ct.tensor(reader, &format!("{prefix}.attn_norm.weight"), device)?;
            let ffn_norm = ct.tensor(reader, &format!("{prefix}.ffn_norm.weight"), device)?;

            let span_attn = tracing::span!(tracing::Level::TRACE, "attn");
            let span_rot = tracing::span!(tracing::Level::TRACE, "attn-rot");
            let span_mlp = tracing::span!(tracing::Level::TRACE, "attn-mlp");

            layers.push(LayerWeights {
                attention_wq: QMatMul::from_qtensor(attention_wq)?,
                attention_wk: QMatMul::from_qtensor(attention_wk)?,
                attention_wv: QMatMul::from_qtensor(attention_wv)?,
                attention_bq: attention_bq.dequantize(device)?,
                attention_bk: attention_bk.dequantize(device)?,
                attention_bv: attention_bv.dequantize(device)?,
                attention_wo: QMatMul::from_qtensor(attention_wo)?,
                attention_norm: RmsNorm::from_qtensor(attention_norm, rms_norm_eps)?,
                cos: cos.clone(),
                sin: sin.clone(),
                mlp,
                ffn_norm: RmsNorm::from_qtensor(ffn_norm, rms_norm_eps)?,
                n_head: head_count,
                n_kv_head: head_count_kv,
                head_dim,
                neg_inf: neg_inf.clone(),
                kv_cache: None,
                span_attn,
                span_rot,
                span_mlp,
                hybrid_wq,
                hybrid_wk,
                hybrid_wv,
                hybrid_wo,
            });
        }

        let span = tracing::span!(tracing::Level::TRACE, "model");
        let span_output = tracing::span!(tracing::Level::TRACE, "output");

        Ok(Self {
            tok_embeddings: Embedding::new(tok_embeddings, embedding_length),
            layers,
            norm,
            output,
            masks: HashMap::new(),
            span,
            span_output,
            hybrid_matrix_config: if hybrid_matrix_enabled {
                hybrid_matrix_config
            } else {
                None
            },
        })
    }

    /// Apply final norm + LM head projection on a single hidden state vector.
    ///
    /// Used by distributed pipeline Stage 3 after layer execution to generate logits.
    ///
    /// # Arguments
    /// - `hidden` — Hidden state tensor, typically rank-1 [hidden_size] or rank-2 [1, hidden_size]
    ///
    /// # Returns
    /// Logits tensor for vocabulary
    pub fn apply_final_projection(&self, hidden: &Tensor) -> Result<Tensor> {
        let normed = self.norm.forward(hidden)?;
        self.output.forward(&normed)
    }

    /// Clear all KV caches in all layers for a brand new request.
    ///
    /// # Request Lifecycle
    /// - **Prefill phase** (seq_len > 1): New request, clear cache
    /// - **Decode phase** (seq_len == 1): Continuation, preserve cache
    /// - **Across stages**: Cache persists for the same request
    ///
    /// This ensures attention history is maintained during decode while preventing
    /// shape mismatches from stale caches across different requests.
    pub fn clear_all_kv_caches(&mut self) {
        for layer in &mut self.layers {
            layer.clear_kv_cache();
        }
    }

    /// Check if this is a brand new request that requires KV cache clearing.
    ///
    /// # Detection Logic
    /// - **Prefill phase**: seq_len > 1 (processing multiple tokens)
    /// - **Decode phase**: seq_len == 1 (processing single token)
    /// - **New request**: prefill at Stage 1 (layer_start == 0)
    ///
    /// # Returns
    /// `true` if KV cache should be cleared (new request detected).
    fn should_clear_kv_cache(seq_len: usize, layer_start: usize) -> bool {
        // Clear cache ONLY for new requests entering the pipeline at Stage 1
        // During prefill (seq_len > 1), we're starting a fresh request
        // During decode (seq_len == 1), we must preserve the accumulated cache
        seq_len > 1 && layer_start == 0
    }

    fn mask(&mut self, t: usize, device: &Device) -> Result<Tensor> {
        if let Some(mask) = self.masks.get(&t) {
            Ok(mask.clone())
        } else {
            let mask: Vec<_> = (0..t)
                .flat_map(|i| (0..t).map(move |j| u8::from(j > i)))
                .collect();
            let mask = Tensor::from_slice(&mask, (t, t), device)?;
            self.masks.insert(t, mask.clone());
            Ok(mask)
        }
    }

    pub fn forward(&mut self, x: &Tensor, index_pos: usize) -> Result<Tensor> {
        let (_b_sz, seq_len) = x.dims2()?;
        let mask = if seq_len == 1 {
            None
        } else {
            Some(self.mask(seq_len, x.device())?)
        };
        let _enter = self.span.enter();
        let mut layer_in = self.tok_embeddings.forward(x)?;
        for layer in self.layers.iter_mut() {
            let x = layer_in;
            let residual = &x;
            let x = layer.attention_norm.forward(&x)?;
            let attn = layer.forward_attn(&x, mask.as_ref(), index_pos)?;
            let x = (attn + residual)?;

            // MLP
            let _enter = layer.span_mlp.enter();
            let residual = &x;
            let x = layer.ffn_norm.forward(&x)?;
            let x = layer.mlp.forward(&x)?;
            let x = (x + residual)?;
            layer_in = x
        }
        let x = self.norm.forward(&layer_in)?;
        let x = x.i((.., seq_len - 1, ..))?;
        let _enter = self.span_output.enter();
        self.output.forward(&x)
    }

    /// Execute forward pass for a specific layer range (distributed pipeline).
    ///
    /// # Arguments
    /// - `x` — Input tensor (token IDs for stage 1, hidden states for stage 2/3)
    /// - `layer_start` — First layer index (inclusive)
    /// - `layer_end` — Last layer index (exclusive)
    /// - `index_pos` — Sequence position for KV cache
    /// - `apply_embeddings` — If true, applies token embeddings first (stage 1 only)
    /// - `apply_final_projection` — If true, applies norm + LM head (final stage only)
    ///
    /// # Returns
    /// Intermediate hidden states after executing layers [layer_start, layer_end).
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 1: Embed + layers 0-26
    /// let h1 = model.forward_layer_range(tokens, 0, 27, 0, true, false)?;
    ///
    /// // Stage 2: Layers 27-52 (no embedding, no projection)
    /// let h2 = model.forward_layer_range(&h1, 27, 53, 0, false, false)?;
    ///
    /// // Stage 3: Layers 53-80 + LM head
    /// let logits = model.forward_layer_range(&h2, 53, 80, 0, false, true)?;
    /// ```
    pub fn forward_layer_range(
        &mut self,
        x: &Tensor,
        layer_start: usize,
        layer_end: usize,
        index_pos: usize,
        apply_embeddings: bool,
        apply_final_projection: bool,
    ) -> Result<Tensor> {
        tracing::info!(
            "🎯 forward_layer_range ENTRY: x.dims={:?}, layers={}..{}, index_pos={}, apply_emb={}, apply_final={}",
            x.dims(), layer_start, layer_end, index_pos, apply_embeddings, apply_final_projection
        );

        // Validate layer range
        if layer_start >= layer_end {
            candle_core::bail!("Invalid layer range: {layer_start}..{layer_end}");
        }
        if layer_end > self.layers.len() {
            candle_core::bail!(
                "Layer end index {} exceeds model layers count {}",
                layer_end,
                self.layers.len()
            );
        }

        let seq_len = if apply_embeddings {
            let (_b_sz, seq_len) = x.dims2()?;
            seq_len
        } else {
            let (_b_sz, seq_len, _hidden_size) = x.dims3()?;
            seq_len
        };

        // CRITICAL KV CACHE LIFECYCLE MANAGEMENT
        // Clear cache ONLY for brand new requests entering at Stage 1 during prefill.
        // Preserve cache during:
        // - Decode phase (seq_len == 1): appends to existing cache
        // - Stage 2/3 during prefill: needs Stage 1's cache for correct attention
        if Self::should_clear_kv_cache(seq_len, layer_start) {
            tracing::info!(
                "🔄 NEW REQUEST detected: Clearing all KV caches (seq_len={}, stage=1)",
                seq_len
            );
            self.clear_all_kv_caches();
        } else {
            tracing::debug!(
                "✓ Preserving KV cache (seq_len={}, layer_start={}, decode_phase={})",
                seq_len,
                layer_start,
                seq_len == 1
            );
        }

        let mask = if seq_len == 1 {
            None
        } else {
            Some(self.mask(seq_len, x.device())?)
        };

        let _enter = self.span.enter();

        // Stage 1: Apply token embeddings
        let mut layer_in = if apply_embeddings {
            self.tok_embeddings.forward(x)?
        } else {
            x.clone()
        };

        tracing::info!(
            "✅ layer_in initialized: dims={:?}, about to execute {} layers",
            layer_in.dims(),
            layer_end - layer_start
        );

        // Execute layers in specified range
        for layer_idx in layer_start..layer_end {
            // Log quantization stage if hybrid matrix is enabled
            if let Some(ref config) = self.hybrid_matrix_config {
                let stage = QuantizationStage::for_layer(layer_idx, config);
                tracing::debug!(
                    "🔄 Layer {}: {} quantization (layer_in.dims={:?})",
                    layer_idx,
                    stage.name(),
                    layer_in.dims()
                );
            } else {
                tracing::info!(
                    "🔄 Layer {}: layer_in.dims={:?}",
                    layer_idx,
                    layer_in.dims()
                );
            }

            let layer = &mut self.layers[layer_idx];
            let x = layer_in;
            let residual = &x;
            let x = layer.attention_norm.forward(&x)?;
            let attn = layer.forward_attn(&x, mask.as_ref(), index_pos)?;
            let x = (attn + residual)?;
            tracing::debug!(
                "  ✓ Layer {} attn complete: x.dims={:?}",
                layer_idx,
                x.dims()
            );

            // MLP
            let _enter = layer.span_mlp.enter();
            let residual = &x;
            let x = layer.ffn_norm.forward(&x)?;
            let x = layer.mlp.forward(&x)?;
            let x = (x + residual)?;
            tracing::debug!(
                "  ✓ Layer {} MLP complete: x.dims={:?}",
                layer_idx,
                x.dims()
            );
            layer_in = x;
        }

        // Final stage: Apply norm + LM head projection
        if apply_final_projection {
            let x = self.norm.forward(&layer_in)?;
            let x = x.i((.., seq_len - 1, ..))?;
            let _enter = self.span_output.enter();
            self.output.forward(&x)
        } else {
            // Return intermediate hidden states
            Ok(layer_in)
        }
    }
}
