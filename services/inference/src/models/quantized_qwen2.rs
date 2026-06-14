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
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

/// KV cache snapshot for prefix-only incremental prefill.
/// Stores frozen KV cache after processing fixed system prefix to accelerate subsequent prefills.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct KVCacheSnapshot {
    /// Frozen KV cache per layer after system prefix completion
    pub layer_kvs: Vec<(Tensor, Tensor)>,
    /// System prefix token count (matched tokens can skip computation)
    pub prefix_len: usize,
    /// Hash/signature of system prompt to detect changes
    pub prefix_hash: u64,
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

fn uses_bitnet_weights(layer_idx: usize, config: &HybridMatrixConfig) -> bool {
    matches!(
        QuantizationStage::for_layer(layer_idx, config),
        QuantizationStage::BitNet
    )
}

/// Hybrid-quantized weight matrix for extreme compression.
#[derive(Debug, Clone)]
struct HybridWeights {
    stage: QuantizationStage,
    rows: usize,
    cols: usize,
    bitnet_packed: Option<Vec<u64>>,
    bitnet_row_scales: Option<Vec<f32>>,
    int8_weight_tensor: Option<Tensor>,
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

        tracing::debug!(
            "🔍 from_f32_tensor: layer={}, stage={}, shape=[{}, {}], dtype={:?}",
            layer_idx,
            stage.name(),
            rows,
            cols,
            tensor.dtype()
        );

        let weights_f32 = tensor.to_vec2::<f32>()?;

        // Log first few values to verify dequantization
        tracing::debug!(
            "   First row samples (before quantization): [{:.6}, {:.6}, {:.6}, {:.6}]",
            weights_f32.first().and_then(|r| r.first()).unwrap_or(&0.0),
            weights_f32.first().and_then(|r| r.get(1)).unwrap_or(&0.0),
            weights_f32.first().and_then(|r| r.get(2)).unwrap_or(&0.0),
            weights_f32.first().and_then(|r| r.get(3)).unwrap_or(&0.0)
        );

        let flattened: Vec<f32> = weights_f32.into_iter().flatten().collect();

        match stage {
            QuantizationStage::BitNet => {
                let mut row_scales = Vec::with_capacity(rows);
                for row in 0..rows {
                    let row_start = row * cols;
                    let row_end = row_start + cols;
                    let row_slice = &flattened[row_start..row_end];
                    let mean_abs = row_slice.iter().map(|v| v.abs()).sum::<f32>() / cols as f32;
                    row_scales.push(mean_abs.max(1e-8));
                }

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
                    bitnet_row_scales: Some(row_scales),
                    int8_weight_tensor: None,
                })
            }
            QuantizationStage::Int8Stage1 | QuantizationStage::Int8Stage3 => {
                let (quantized, scale) = blazil_inference::quantize_int8(&flattened);
                let dequantized = blazil_inference::dequantize_int8(&quantized, scale);
                let weight_tensor = Tensor::from_vec(dequantized, (rows, cols), &Device::Cpu)?
                    .to_dtype(candle_core::DType::F32)?;
                Ok(Self {
                    stage,
                    rows,
                    cols,
                    bitnet_packed: None,
                    bitnet_row_scales: None,
                    int8_weight_tensor: Some(weight_tensor),
                })
            }
        }
    }

    /// Forward pass with hybrid quantization.
    /// Handles both rank-2 [batch, in_features] and rank-3 [batch, seq_len, in_features] inputs.
    /// Uses Candle's optimized matmul for performance.
    fn forward(&self, input: &Tensor) -> Result<Tensor> {
        let dims = input.dims();
        let forward_start = Instant::now();

        tracing::debug!(
            "🔧 HybridWeights::forward: stage={}, input_shape={:?}",
            self.stage.name(),
            dims
        );

        // Handle both rank-2 and rank-3 inputs
        let (input_2d, original_shape) = match dims.len() {
            2 => {
                // Already rank-2: [batch, in_features]
                (input.clone(), None)
            }
            3 => {
                // Rank-3: [batch, seq_len, in_features] -> flatten to [batch * seq_len, in_features]
                let (batch, seq_len, in_features) = (dims[0], dims[1], dims[2]);
                let flattened = input.reshape(&[batch * seq_len, in_features])?;
                (flattened, Some((batch, seq_len)))
            }
            _ => {
                candle_core::bail!(
                    "HybridWeights::forward: expected rank-2 or rank-3 input, got rank-{} {:?}",
                    dims.len(),
                    dims
                );
            }
        };

        // Stage-specific projection.
        let output_2d = match self.stage {
            QuantizationStage::BitNet => {
                let stage_start = Instant::now();
                let packed = self.bitnet_packed.as_ref().unwrap();
                let row_scales = self.bitnet_row_scales.as_ref().unwrap();
                let extract_start = Instant::now();
                let inputs = input_2d.to_vec2::<f32>()?;
                let extract_ms = extract_start.elapsed().as_secs_f64() * 1000.0;
                let batch = inputs.len();
                let kernel_start = Instant::now();
                let mut output_flat = vec![0f32; batch * self.rows];

                if batch > 1 {
                    let input_flat: Vec<f32> = inputs.into_iter().flatten().collect();
                    blazil_inference::bitnet_linear_1bit_batched_parallel(
                        &input_flat,
                        batch,
                        packed,
                        self.rows,
                        self.cols,
                        &mut output_flat,
                    )
                    .map_err(|err| {
                        candle_core::Error::Msg(format!("BitNet kernel failed: {err}"))
                    })?;

                    for row_out in output_flat.chunks_mut(self.rows) {
                        for (v, scale) in row_out.iter_mut().zip(row_scales.iter()) {
                            *v *= *scale;
                        }
                    }
                } else {
                    for input_row in &inputs {
                        let mut row_out = vec![0f32; self.rows];
                        let kernel_result = if self.rows >= 1024 {
                            blazil_inference::bitnet_linear_1bit_parallel(
                                input_row,
                                packed,
                                self.rows,
                                self.cols,
                                &mut row_out,
                            )
                        } else {
                            blazil_inference::bitnet_linear_1bit(
                                input_row,
                                packed,
                                self.rows,
                                self.cols,
                                &mut row_out,
                            )
                        };
                        if let Err(err) = kernel_result {
                            candle_core::bail!("BitNet kernel failed: {err}");
                        }
                        for (v, scale) in row_out.iter_mut().zip(row_scales.iter()) {
                            *v *= *scale;
                        }
                        output_flat.copy_from_slice(&row_out);
                    }
                }

                let kernel_ms = kernel_start.elapsed().as_secs_f64() * 1000.0;
                let materialize_start = Instant::now();
                let output = Tensor::from_vec(output_flat, (batch, self.rows), input_2d.device())?;
                let materialize_ms = materialize_start.elapsed().as_secs_f64() * 1000.0;
                let total_ms = stage_start.elapsed().as_secs_f64() * 1000.0;

                if total_ms >= 5.0 {
                    tracing::info!(
                        "⏱️ Stage2(BitNet) total_ms={:.3} extract_ms={:.3} kernel_ms={:.3} materialize_ms={:.3} batch={} rows={} cols={} input_shape={:?}",
                        total_ms,
                        extract_ms,
                        kernel_ms,
                        materialize_ms,
                        batch,
                        self.rows,
                        self.cols,
                        dims
                    );
                } else {
                    tracing::debug!(
                        "⏱️ Stage2(BitNet) total_ms={:.3} extract_ms={:.3} kernel_ms={:.3} materialize_ms={:.3} batch={} rows={} cols={} input_shape={:?}",
                        total_ms,
                        extract_ms,
                        kernel_ms,
                        materialize_ms,
                        batch,
                        self.rows,
                        self.cols,
                        dims
                    );
                }
                output
            }
            QuantizationStage::Int8Stage1 | QuantizationStage::Int8Stage3 => {
                let stage_start = Instant::now();
                let weight_tensor = self.int8_weight_tensor.as_ref().unwrap();
                let out = input_2d.matmul(&weight_tensor.t()?)?;
                tracing::debug!(
                    "⏱️ {} core_time_ms={:.3} input_shape={:?}",
                    self.stage.name(),
                    stage_start.elapsed().as_secs_f64() * 1000.0,
                    dims
                );
                out
            }
        };

        tracing::debug!("   Output shape: {:?}", output_2d.dims());

        // Reshape back to rank-3 if original input was rank-3
        if let Some((batch, seq_len)) = original_shape {
            let out = output_2d.reshape(&[batch, seq_len, self.rows])?;
            tracing::debug!(
                "⏱️ HybridWeights::forward total_ms={:.3} stage={} input_shape={:?}",
                forward_start.elapsed().as_secs_f64() * 1000.0,
                self.stage.name(),
                dims
            );
            Ok(out)
        } else {
            tracing::debug!(
                "⏱️ HybridWeights::forward total_ms={:.3} stage={} input_shape={:?}",
                forward_start.elapsed().as_secs_f64() * 1000.0,
                self.stage.name(),
                dims
            );
            Ok(output_2d)
        }
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

        tracing::debug!(
            "🔍 forward_attn START: x={:?}, index_pos={}",
            x.dims(),
            index_pos
        );
        if let Some(m) = mask {
            tracing::debug!("   mask: {:?}", m.dims());
        } else {
            tracing::debug!("   mask: None");
        }

        let (q, k, v) = if let (Some(hwq), Some(hwk), Some(hwv)) =
            (&self.hybrid_wq, &self.hybrid_wk, &self.hybrid_wv)
        {
            tracing::debug!("   Using Hybrid Matrix weights");
            (hwq.forward(x)?, hwk.forward(x)?, hwv.forward(x)?)
        } else {
            tracing::debug!("   Using Q4_K_M weights");
            (
                self.attention_wq.forward(x)?,
                self.attention_wk.forward(x)?,
                self.attention_wv.forward(x)?,
            )
        };

        tracing::debug!(
            "   q/k/v after projection: q={:?}, k={:?}, v={:?}",
            q.dims(),
            k.dims(),
            v.dims()
        );

        let q = q.broadcast_add(&self.attention_bq)?;
        let k = k.broadcast_add(&self.attention_bk)?;
        let v = v.broadcast_add(&self.attention_bv)?;

        tracing::debug!(
            "   After bias add: q={:?}, k={:?}, v={:?}",
            q.dims(),
            k.dims(),
            v.dims()
        );

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

        tracing::debug!(
            "   After reshape+transpose: q={:?}, k={:?}, v={:?}",
            q.dims(),
            k.dims(),
            v.dims()
        );

        let q = self.apply_rotary_emb(&q, index_pos)?;
        let k = self.apply_rotary_emb(&k, index_pos)?;

        tracing::debug!("   After rotary: q={:?}, k={:?}", q.dims(), k.dims());

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

        tracing::debug!("   After KV cache: k={:?}, v={:?}", k.dims(), v.dims());

        let k = repeat_kv(k, self.n_head / self.n_kv_head)?;
        let v = repeat_kv(v, self.n_head / self.n_kv_head)?;

        tracing::debug!("   After repeat_kv: k={:?}, v={:?}", k.dims(), v.dims());

        let att = (q.matmul(&k.t()?)? / (self.head_dim as f64).sqrt())?;
        tracing::debug!("   Attention scores: att={:?}", att.dims());

        let att = match mask {
            None => att,
            Some(mask) => {
                tracing::debug!(
                    "   Applying mask: mask={:?}, att={:?}",
                    mask.dims(),
                    att.dims()
                );
                let mask = mask.broadcast_as(att.shape())?;
                tracing::debug!("   Mask after broadcast: {:?}", mask.dims());
                masked_fill(&att, &mask, &self.neg_inf)?
            }
        };
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let y = att.matmul(&v.contiguous()?)?;
        tracing::debug!("   After attention matmul: y={:?}", y.dims());

        let y = y.transpose(1, 2)?.reshape(&[b_sz, seq_len, n_embd])?;
        tracing::debug!("   After final reshape: y={:?}", y.dims());

        let y = if let Some(hwo) = &self.hybrid_wo {
            hwo.forward(&y)?
        } else {
            self.attention_wo.forward(&y)?
        };

        tracing::debug!("   After output projection: y={:?}", y.dims());
        tracing::debug!("✅ forward_attn END");

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
    /// Prefix-KV snapshot for delta-only prefill acceleration.
    /// After system prefix is fully processed once, frozen KV cache enables skipping prefix recomputation.
    prefix_kv_snapshot: Arc<Mutex<Option<KVCacheSnapshot>>>,
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

            let (hybrid_wq, hybrid_wk, hybrid_wv, hybrid_wo) = if hybrid_matrix_enabled
                && uses_bitnet_weights(layer_idx, hybrid_matrix_config.as_ref().unwrap())
            {
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

                let (hybrid_w1, hybrid_w2, hybrid_w3) = if hybrid_matrix_enabled
                    && uses_bitnet_weights(layer_idx, hybrid_matrix_config.as_ref().unwrap())
                {
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
            prefix_kv_snapshot: Arc::new(Mutex::new(None)),
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

    /// Capture current KV cache state as frozen snapshot for system prefix.
    /// Call after system prompt prefill completes to enable delta-prefill on subsequent user queries.
    #[allow(dead_code)]
    pub fn capture_kv_snapshot(&self, prefix_len: usize, prefix_hash: u64) {
        let layer_kvs: Vec<(Tensor, Tensor)> = self
            .layers
            .iter()
            .filter_map(|layer| layer.kv_cache.clone())
            .collect();

        if layer_kvs.len() == self.layers.len() {
            let snapshot = KVCacheSnapshot {
                layer_kvs,
                prefix_len,
                prefix_hash,
            };
            if let Ok(mut snap) = self.prefix_kv_snapshot.lock() {
                *snap = Some(snapshot);
                tracing::info!(
                    "📸 KV snapshot captured: prefix_len={}, hash={:x}, layers={}",
                    prefix_len,
                    prefix_hash,
                    self.layers.len()
                );
            }
        } else {
            tracing::warn!(
                "⚠️ KV snapshot capture failed: only {}/{} layers have cache",
                layer_kvs.len(),
                self.layers.len()
            );
        }
    }

    /// Restore frozen KV snapshot into layers (resets KV cache to system prefix endpoint).
    #[allow(dead_code)]
    pub fn restore_kv_snapshot(&mut self) -> Result<bool> {
        if let Ok(snap_lock) = self.prefix_kv_snapshot.lock() {
            if let Some(snapshot) = snap_lock.as_ref() {
                for (layer, (k, v)) in self.layers.iter_mut().zip(snapshot.layer_kvs.iter()) {
                    layer.kv_cache = Some((k.clone(), v.clone()));
                }
                tracing::debug!(
                    "🔄 KV snapshot restored: prefix_len={}",
                    snapshot.prefix_len
                );
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Restore snapshot only when prefix hash/length match.
    /// Returns `Some(prefix_len)` when restored, otherwise `None`.
    #[allow(dead_code)]
    pub fn restore_kv_snapshot_for_prefix(
        &mut self,
        prefix_tokens: &[u32],
    ) -> Result<Option<usize>> {
        let expected_hash = Self::compute_prefix_hash(prefix_tokens);
        if let Ok(snap_lock) = self.prefix_kv_snapshot.lock() {
            if let Some(snapshot) = snap_lock.as_ref() {
                if snapshot.prefix_hash != expected_hash
                    || snapshot.prefix_len != prefix_tokens.len()
                {
                    tracing::info!(
                        "🧪 KV snapshot mismatch: expected_len={}, expected_hash={:x}, snapshot_len={}, snapshot_hash={:x}",
                        prefix_tokens.len(),
                        expected_hash,
                        snapshot.prefix_len,
                        snapshot.prefix_hash
                    );
                    return Ok(None);
                }

                let prefix_len = snapshot.prefix_len;
                for (layer, (k, v)) in self.layers.iter_mut().zip(snapshot.layer_kvs.iter()) {
                    layer.kv_cache = Some((k.clone(), v.clone()));
                }
                tracing::info!(
                    "✨ KV snapshot matched and restored: prefix_len={}, hash={:x}",
                    prefix_len,
                    expected_hash
                );
                return Ok(Some(prefix_len));
            }

            tracing::info!(
                "🧪 KV snapshot unavailable: expected_len={}, expected_hash={:x}",
                prefix_tokens.len(),
                expected_hash
            );
        } else {
            tracing::warn!("⚠️ Failed to lock KV snapshot mutex during restore");
        }
        Ok(None)
    }

    /// Compute simple hash for system prefix tokens (for change detection).
    #[allow(dead_code)]
    pub fn compute_prefix_hash(tokens: &[u32]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        tokens.hash(&mut hasher);
        hasher.finish()
    }

    /// Finalize prefill phase and capture KV snapshot for system prefix (after full chat setup).
    /// Call after processing system prompt tokens to enable delta-only prefill on subsequent user queries.
    ///
    /// # Arguments
    /// - `prefix_tokens` — System prompt tokens (used for hash/change detection)
    ///
    /// # Example
    /// ```rust,ignore
    /// // After system prompt fully processed through all pipeline stages
    /// model.finalize_prefill_and_capture_snapshot(&system_tokens)?;
    /// ```
    #[allow(dead_code)]
    pub fn finalize_prefill_and_capture_snapshot(&self, prefix_tokens: &[u32]) -> Result<()> {
        let prefix_len = prefix_tokens.len();
        let prefix_hash = Self::compute_prefix_hash(prefix_tokens);
        self.capture_kv_snapshot(prefix_len, prefix_hash);
        Ok(())
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
            tracing::debug!("📋 Using cached mask: shape={:?}", mask.dims());
            Ok(mask.clone())
        } else {
            let mask: Vec<_> = (0..t)
                .flat_map(|i| (0..t).map(move |j| u8::from(j > i)))
                .collect();
            let mask = Tensor::from_slice(&mask, (t, t), device)?;
            tracing::debug!("📋 Created new mask: shape={:?}", mask.dims());
            self.masks.insert(t, mask.clone());
            Ok(mask)
        }
    }

    fn mask_with_past(&self, t: usize, past_len: usize, device: &Device) -> Result<Tensor> {
        let total_k_len = past_len + t;
        let mask: Vec<_> = (0..t)
            .flat_map(|i| (0..total_k_len).map(move |j| u8::from(j > past_len + i)))
            .collect();
        Tensor::from_slice(&mask, (t, total_k_len), device)
    }

    pub fn forward(&mut self, x: &Tensor, index_pos: usize) -> Result<Tensor> {
        let forward_start = Instant::now();
        let (_b_sz, seq_len) = x.dims2()?;
        let hmq_enabled = self
            .hybrid_matrix_config
            .as_ref()
            .map(|cfg| cfg.enabled)
            .unwrap_or(false);
        let mask = if seq_len == 1 {
            None
        } else if index_pos > 0 {
            Some(self.mask_with_past(seq_len, index_pos, x.device())?)
        } else {
            Some(self.mask(seq_len, x.device())?)
        };
        let _enter = self.span.enter();
        let mut layer_in = self.tok_embeddings.forward(x)?;
        for (layer_idx, layer) in self.layers.iter_mut().enumerate() {
            let layer_start = Instant::now();
            let x = layer_in;
            let residual = &x;
            let x = layer.attention_norm.forward(&x)?;
            let attn_start = Instant::now();
            let attn = layer.forward_attn(&x, mask.as_ref(), index_pos)?;
            let attn_ms = attn_start.elapsed().as_secs_f64() * 1000.0;
            let x = (attn + residual)?;

            // MLP
            let _enter = layer.span_mlp.enter();
            let residual = &x;
            let x = layer.ffn_norm.forward(&x)?;
            let mlp_start = Instant::now();
            let x = layer.mlp.forward(&x)?;
            let mlp_ms = mlp_start.elapsed().as_secs_f64() * 1000.0;
            let x = (x + residual)?;
            let layer_ms = layer_start.elapsed().as_secs_f64() * 1000.0;
            if hmq_enabled && seq_len > 1 && layer_ms >= 5.0 {
                tracing::info!(
                    "⏱️ HMQ layer={} seq_len={} index_pos={} attn_ms={:.3} mlp_ms={:.3} total_ms={:.3}",
                    layer_idx,
                    seq_len,
                    index_pos,
                    attn_ms,
                    mlp_ms,
                    layer_ms
                );
            }
            layer_in = x
        }
        let x = self.norm.forward(&layer_in)?;
        let x = x.i((.., seq_len - 1, ..))?;
        let _enter = self.span_output.enter();
        let logits = self.output.forward(&x)?;
        if hmq_enabled && seq_len > 1 {
            tracing::info!(
                "⏱️ HMQ forward total_ms={:.3} seq_len={} index_pos={}",
                forward_start.elapsed().as_secs_f64() * 1000.0,
                seq_len,
                index_pos
            );
        }
        Ok(logits)
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
            tracing::debug!("⚡ Decode mode: seq_len=1, no mask needed");
            None
        } else {
            tracing::debug!("🔍 Prefill mode: seq_len={}, creating mask", seq_len);
            let m = self.mask(seq_len, x.device())?;
            tracing::debug!("✅ Mask created: {:?}", m.dims());
            Some(m)
        };

        // 📸 Prefix-KV snapshot: attempt delta-only prefill acceleration
        // If snapshot restored, KV cache pre-populated with system prefix state
        let _snapshot_active = if apply_embeddings && seq_len > 1 && layer_start == 0 {
            // Prefill at stage 1: try restore snapshot for delta mode
            match self.restore_kv_snapshot() {
                Ok(true) => {
                    tracing::info!("✨ KV snapshot RESTORED: delta-only prefill enabled");
                    true
                }
                _ => false,
            }
        } else {
            false
        };

        let _enter = self.span.enter();

        // Stage 1: Apply token embeddings
        let mut layer_in = if apply_embeddings {
            let emb = self.tok_embeddings.forward(x)?;
            tracing::debug!(
                "🎯 Embeddings applied: x.dims={:?} -> emb.dims={:?}",
                x.dims(),
                emb.dims()
            );
            emb
        } else {
            tracing::debug!("⏭️  Skipping embeddings (not first stage)");
            x.clone()
        };

        tracing::info!(
            "✅ layer_in initialized: dims={:?}, about to execute {} layers",
            layer_in.dims(),
            layer_end - layer_start
        );

        // Execute layers in specified range
        for layer_idx in layer_start..layer_end {
            tracing::debug!("\n━━━ Layer {} START ━━━", layer_idx);
            tracing::debug!("   Input: {:?}", layer_in.dims());

            // Log quantization stage if hybrid matrix is enabled
            if let Some(ref config) = self.hybrid_matrix_config {
                let stage = QuantizationStage::for_layer(layer_idx, config);
                tracing::debug!("   Quantization: {}", stage.name());
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
