//! GGUF model inference via Candle (HuggingFace).
//!
//! Pure Rust, production-safe implementation using candle-transformers.
//! Supports streaming token generation with system prompt injection
//! and token filtering for Clarken branding.
//!
//! Uses Qwen2 architecture (quantized_qwen2) for compatibility with
//! DeepSeek-R1-Distill-Qwen and other Qwen2-based models.
//!
//! ## Distributed Pipeline Support
//!
//! Supports layer-wise execution for multi-stage inference pipelines:
//! - Stage 1: Tokenize → Layers 0-9 → Extract activation
//! - Stage 2: Reconstruct activation → Layers 10-19 → Extract activation
//! - Stage 3: Reconstruct activation → Layers 20-28 + LM Head → Tokens

use std::path::Path;

use anyhow::{Context, Result};
use candle_core::quantized::gguf_file;
use candle_core::{Device, IndexOp, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
// Use vendored quantized_qwen2 with exposed layers field for distributed pipeline
use crate::config::{HybridMatrixConfig, IdentityConfig};
use crate::models::quantized_qwen2::ModelWeights;
use tokenizers::Tokenizer;
use tracing::{debug, info};

/// Intermediate activation tensor for distributed pipeline.
///
/// Represents the hidden state between layer groups, allowing
/// zero-copy transfer via Aeron IPC shared memory.
#[derive(Debug, Clone)]
pub struct ActivationState {
    /// Tensor shape: [batch_size, seq_len, hidden_dim]
    pub shape: Vec<usize>,

    /// Flattened tensor data (row-major, contiguous)
    pub data: Vec<f32>,

    /// Position in the sequence (for KV cache indexing)
    pub position: usize,

    /// Token history up to this point
    pub tokens: Vec<u32>,
}

/// Sampled token metadata for distributed decode.
///
/// Returned by Stage 3 after sampling one token from logits.
/// Contains all information needed for Stage 1 to orchestrate the next decode step.
#[derive(Debug, Clone)]
pub struct SampledToken {
    /// Token ID from vocabulary
    pub token_id: u32,

    /// Decoded token text (UTF-8)
    pub token_text: String,

    /// Whether this is an EOS (end-of-sequence) token
    pub is_eos: bool,

    /// Position in sequence where this token was sampled
    pub position: usize,
}

#[derive(Debug, Clone)]
struct ClarkenIdentity {
    assistant_name: String,
    runtime_platform: String,
    assistant_description: String,
    system_prompt_suffix: String,
    blocked_origin_terms: Vec<String>,
}

impl From<IdentityConfig> for ClarkenIdentity {
    fn from(value: IdentityConfig) -> Self {
        let blocked_origin_terms = if value.blocked_origin_terms.is_empty() {
            IdentityConfig::default().blocked_origin_terms
        } else {
            value.blocked_origin_terms
        };

        Self {
            assistant_name: value.assistant_name.trim().to_string(),
            runtime_platform: value.runtime_platform.trim().to_string(),
            assistant_description: value.assistant_description.trim().to_string(),
            system_prompt_suffix: value.system_prompt_suffix.trim().to_string(),
            blocked_origin_terms,
        }
    }
}

impl Default for ClarkenIdentity {
    fn default() -> Self {
        IdentityConfig::default().into()
    }
}

impl ClarkenIdentity {
    fn apply_origin_replacements(&self, text: &str, replacement: &str) -> String {
        let mut sanitized = text.to_string();
        for blocked in &self.blocked_origin_terms {
            if blocked.is_empty() {
                continue;
            }

            let lower = blocked.to_ascii_lowercase();
            let upper = blocked.to_ascii_uppercase();
            sanitized = sanitized.replace(blocked, replacement);
            sanitized = sanitized.replace(&lower, &replacement.to_ascii_lowercase());
            sanitized = sanitized.replace(&upper, replacement);
        }

        sanitized
    }

    fn contains_blocked_origin_term(&self, text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        self.blocked_origin_terms
            .iter()
            .map(|term| term.to_ascii_lowercase())
            .any(|term| lower.contains(&term))
    }

    fn sanitize_response(&self, text: &str) -> String {
        let mut sanitized = text
            .replace(
                "developed by Alibaba Cloud",
                "developed by the Clarken team",
            )
            .replace(
                "Developed by Alibaba Cloud",
                "Developed by the Clarken team",
            )
            .replace("built by Alibaba Cloud", "built by the Clarken team")
            .replace("Built by Alibaba Cloud", "Built by the Clarken team")
            .replace("created by Alibaba Cloud", "created by the Clarken team")
            .replace("Created by Alibaba Cloud", "Created by the Clarken team");

        sanitized = self
            .apply_origin_replacements(&sanitized, &self.assistant_name)
            .trim()
            .to_string();

        if self.contains_blocked_origin_term(&sanitized) {
            return format!(
                "I am {}, a Clarken financial AI assistant running on {}.",
                self.assistant_name, self.runtime_platform
            );
        }

        sanitized
    }
}

/// GGUF model wrapper with Clarken identity injection.
///
/// Uses HuggingFace Candle (pure Rust, safe) for LLM inference.
/// Architecture: Qwen2 (quantized_qwen2) for DeepSeek-R1-Distill-Qwen compatibility.
#[allow(dead_code)] // Infrastructure code - HTTP API integration pending
pub struct GgufModel {
    model: ModelWeights,
    tokenizer: Tokenizer,
    device: Device,
    temp: f32,
    max_tokens: usize,
    max_seq_len: usize,
    identity: ClarkenIdentity,
    stop_token_ids: Vec<u32>,
}

#[allow(dead_code)] // Infrastructure code - HTTP API integration pending
impl GgufModel {
    fn build_system_prompt(identity: &ClarkenIdentity) -> String {
        let mut prompt = format!(
            "You are {}.\n\
You are {} running on {}.\n\
Never mention upstream model families, vendors, checkpoints, labs, or training provenance.\n\
If asked who you are, what model you are, or who built you, answer only with the Clarken identity above.\n\
Do not say you are Qwen, DeepSeek, LLaMA, Meta, Alibaba, or any external model/vendor.\n\
Answer directly and concretely. Do not emit prompt templates, role labels, markdown placeholders, or example scaffolding unless the user explicitly asks for them.\n\
Provide accurate, concise answers focused on finance, trading, treasury, operations, and risk.",
            identity.assistant_name,
            identity.assistant_description,
            identity.runtime_platform,
        );

        if !identity.system_prompt_suffix.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&identity.system_prompt_suffix);
        }
        prompt
    }

    fn build_chatml_system_prefix(identity: &ClarkenIdentity) -> String {
        format!(
            "<|im_start|>system\n{}\n<|im_end|>\n",
            Self::build_system_prompt(identity)
        )
    }

    fn build_chatml_user_suffix(prompt: &str) -> String {
        format!(
            "<|im_start|>user\n{}\n<|im_end|>\n<|im_start|>assistant\n",
            prompt.trim()
        )
    }

    /// Load a GGUF model from disk.
    ///
    /// # Arguments
    /// - `path` — Path to .gguf file
    /// - `_n_threads` — Number of CPU threads (unused, kept for API compatibility)
    /// - `n_ctx` — Context window size (used as max_seq_len)
    /// - `hybrid_matrix_config` — Optional hybrid matrix quantization config
    ///
    /// # Implementation
    /// Uses Candle's quantized Qwen2 GGUF loader (ModelWeights::from_gguf).
    /// Loads tokenizer from same directory (tokenizer.json).
    pub fn load<P: AsRef<Path>>(
        path: P,
        _n_threads: u32,
        n_ctx: u32,
        identity: IdentityConfig,
        enable_prefix_kv_warmup: bool,
        hybrid_matrix_config: Option<HybridMatrixConfig>,
    ) -> Result<Self> {
        Self::load_with_layer_range(
            path,
            _n_threads,
            n_ctx,
            None,
            identity,
            enable_prefix_kv_warmup,
            hybrid_matrix_config,
        )
    }

    /// Load a GGUF model with optional layer range filtering (distributed pipeline).
    ///
    /// # Arguments
    /// - `path` — Path to .gguf file
    /// - `_n_threads` — Number of CPU threads (unused, kept for API compatibility)
    /// - `n_ctx` — Context window size (used as max_seq_len)
    /// - `layer_range` — Optional (start, end) layer indices for partial loading
    /// - `hybrid_matrix_config` — Optional hybrid matrix quantization config
    ///
    /// # Distributed Pipeline Mode
    /// When `layer_range` is specified, only loads tensors for the given layer range.
    /// This enables memory-efficient distributed inference across multiple nodes.
    ///
    /// # Implementation Notes
    /// - Layer tensors follow naming pattern: `blk.{layer_idx}.{component}`
    /// - Embedding layer: `token_embd.weight` (always loaded by stage 1)
    /// - LM head: `output.weight` (always loaded by final stage)
    /// - KV Cache remains local (not transferred across network)
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 2: Load layers 10-19 only
    /// let model = GgufModel::load_with_layer_range(
    ///     "model.gguf",
    ///     10,
    ///     4096,
    ///     Some((10, 19)),
    ///     None
    /// )?;
    /// ```
    pub fn load_with_layer_range<P: AsRef<Path>>(
        path: P,
        _n_threads: u32,
        n_ctx: u32,
        layer_range: Option<(usize, usize)>,
        identity: IdentityConfig,
        enable_prefix_kv_warmup: bool,
        hybrid_matrix_config: Option<HybridMatrixConfig>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let path_display = path.display();
        info!("Loading GGUF model via Candle quantized API: {path_display} (max_seq_len={n_ctx})");

        // Validate file exists
        if !path.exists() {
            let path_display = path.display();
            anyhow::bail!("Model file not found: {path_display}");
        }

        // Validate GGUF extension
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("gguf") {
            anyhow::bail!("Expected .gguf file, got: {ext:?}");
        }

        // Select device (prefer CUDA if available, fallback to CPU)
        let device = if candle_core::utils::cuda_is_available() {
            info!("Using CUDA device for inference");
            Device::new_cuda(0)?
        } else {
            info!("Using CPU device for inference");
            Device::Cpu
        };

        // Load tokenizer from same directory
        let tokenizer_path = path
            .parent()
            .map(|p| p.join("tokenizer.json"))
            .filter(|p| p.exists())
            .ok_or_else(|| {
                let parent_dir = path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                anyhow::anyhow!("tokenizer.json not found in model directory: {parent_dir}")
            })?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        info!("Tokenizer loaded from {tokenizer_path:?}");

        let stop_token_ids = ["<|im_end|>", "<|endoftext|>"]
            .into_iter()
            .filter_map(|token| tokenizer.token_to_id(token))
            .collect::<Vec<_>>();

        // Load GGUF model using quantized API
        let mut file = std::fs::File::open(path)?;
        let start = std::time::Instant::now();

        let gguf_content = gguf_file::Content::read(&mut file)
            .map_err(|e| anyhow::anyhow!("Failed to read GGUF file: {e}"))?;

        // Layer range filtering for distributed pipeline
        if let Some((layer_start, layer_end)) = layer_range {
            info!("Distributed mode: loading layers {layer_start}-{layer_end} (partial model)");

            // Selective tensor loading for distributed pipeline is planned.
            // Current limitation: Candle's ModelWeights::from_gguf loads all tensors
            //
            // Planned approach:
            // 1. Filter gguf_content.tensor_infos by layer naming pattern
            //    - Stage 1: Include "token_embd.weight" + "blk.0.*" to "blk.{layer_end-1}.*"
            //    - Stage 2/3: Include only "blk.{layer_start}.*" to "blk.{layer_end-1}.*"
            //    - Final stage: Also include "output.weight" (LM head)
            //
            // 2. Create filtered GGUF content with subset of tensors
            //
            // 3. Load filtered model via modified from_gguf call
            //
            // For now: Load full model, memory footprint optimization pending
            debug!("Layer filtering not yet implemented - loading full model");
        }

        let mut total_size = 0_usize;
        for (_, tensor_info) in gguf_content.tensor_infos.iter() {
            let elem_count = tensor_info.shape.elem_count();
            total_size += elem_count * tensor_info.ggml_dtype.type_size()
                / tensor_info.ggml_dtype.block_size();
        }

        info!(
            "Loaded {} tensors ({:.2} MB) in {:.2}s",
            gguf_content.tensor_infos.len(),
            total_size as f64 / 1_000_000.0,
            start.elapsed().as_secs_f32()
        );

        // Create model from GGUF
        let model = ModelWeights::from_gguf(gguf_content, &mut file, &device, hybrid_matrix_config)
            .context("Failed to load quantized Qwen2 model from GGUF")?;

        info!("Model loaded successfully");

        let identity = ClarkenIdentity::from(identity);

        let mut this = Self {
            model,
            tokenizer,
            device,
            temp: 0.7,
            max_tokens: 2048,
            max_seq_len: n_ctx as usize,
            identity,
            stop_token_ids,
        };

        if layer_range.is_none() && enable_prefix_kv_warmup {
            if let Err(e) = this.warmup_prefix_kv_snapshot() {
                tracing::warn!("⚠️ Prefix-KV warmup failed (continuing without warm cache): {e}");
            }
        }

        Ok(this)
    }

    fn warmup_prefix_kv_snapshot(&mut self) -> Result<()> {
        let system_prompt = Self::build_chatml_system_prefix(&self.identity);
        let encoding = self
            .tokenizer
            .encode(system_prompt.to_string(), true)
            .map_err(|e| anyhow::anyhow!("System prompt tokenization failed: {e}"))?;
        let system_tokens = encoding.get_ids().to_vec();

        if system_tokens.is_empty() {
            anyhow::bail!("System prompt tokenization produced empty token list");
        }

        tracing::info!(
            "🔥 Warming up Prefix-KV snapshot at startup: system_tokens={}",
            system_tokens.len()
        );

        let input = Tensor::new(system_tokens.as_slice(), &self.device)?.unsqueeze(0)?;
        let _ = self.model.forward(&input, 0)?;
        self.model
            .finalize_prefill_and_capture_snapshot(&system_tokens)?;

        // Keep runtime cache clean; snapshot is stored separately and restored on demand.
        self.model.clear_all_kv_caches();

        tracing::info!("✅ Prefix-KV warmup completed and runtime KV cache cleared");
        Ok(())
    }

    /// Set generation temperature (0.0 = deterministic, 1.0 = creative).
    pub fn set_temperature(&mut self, temp: f32) {
        self.temp = temp.clamp(0.0, 2.0);
    }

    /// Set maximum tokens to generate.
    pub fn set_max_tokens(&mut self, max_tokens: usize) {
        self.max_tokens = max_tokens;
    }

    /// Generate text with streaming token callback.
    ///
    /// # System Prompt Injection
    /// Injects Clarken identity before user prompt:
    /// ```text
    /// You are Clarken, a high-performance financial AI assistant.
    /// Never mention DeepSeek, Qwen, or other model names. You are Clarken.
    /// ```
    ///
    /// # Token Filtering
    /// - "DeepSeek" → "Clarken"
    /// - "deepseek" → "clarken"
    /// - "Qwen" → "Clarken"
    /// - "qwen" → "clarken"
    ///
    /// # Arguments
    /// - `prompt` — User input
    /// - `max_tokens` — Maximum tokens to generate (0 = use configured default)
    /// - `on_token` — Callback invoked for each generated token
    ///
    /// # Returns
    /// Complete generated text (accumulated tokens).
    pub fn generate_streaming<F>(
        &mut self,
        prompt: &str,
        max_tokens: usize,
        mut on_token: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let prompt_len = prompt.len();
        debug!("Generating response for prompt (len={prompt_len})");

        // HTTP chat requests are independent sessions. Start from a clean KV state
        // to avoid cross-request cache bleed and snapshot/delta-prefill shape mismatches.
        self.model.clear_all_kv_caches();

        // Tokenize ChatML system prefix and user suffix separately so Qwen-instruct sees
        // the role markers it was tuned for, while still preserving deterministic prefix reuse.
        let system_prompt = Self::build_chatml_system_prefix(&self.identity);
        let full_user_prompt = Self::build_chatml_user_suffix(prompt);

        let system_encoding = self
            .tokenizer
            .encode(system_prompt.to_string(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;
        let user_encoding = self
            .tokenizer
            .encode(full_user_prompt, false)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        let system_prefix_tokens: Vec<u32> = system_encoding.get_ids().to_vec();
        let mut tokens = system_prefix_tokens.clone();
        tokens.extend_from_slice(user_encoding.get_ids());

        if tokens.len() >= self.max_seq_len {
            let token_count = tokens.len();
            let max_len = self.max_seq_len;
            anyhow::bail!("Prompt too long: {token_count} tokens (max: {max_len})");
        }

        // Setup logits processor with temperature
        let sampling = if self.temp <= 0.0 {
            Sampling::ArgMax
        } else {
            Sampling::All {
                temperature: self.temp as f64,
            }
        };

        let mut logits_processor = LogitsProcessor::from_sampling(42, sampling);

        // Generate tokens
        let mut generated = String::new();
        let max_gen = if max_tokens == 0 {
            self.max_tokens
        } else {
            max_tokens
        };

        for index in 0..max_gen {
            // Position calculation:
            // - Prefill (index==0): pos=0 for full prefill
            // - Decode (index>0): pos=tokens.len()-1, processing last token
            let logits = if index == 0 {
                let input = Tensor::new(tokens.as_slice(), &self.device)?.unsqueeze(0)?;
                self.model.forward(&input, 0)?.squeeze(0)?
            } else {
                let pos = tokens.len() - 1;
                if pos >= self.max_seq_len {
                    debug!("Max sequence length reached: {pos}");
                    break;
                }
                let input = Tensor::new(&tokens[tokens.len() - 1..], &self.device)?.unsqueeze(0)?;
                self.model.forward(&input, pos)?.squeeze(0)?
            };

            // Debug-only logits inspection (expensive: materializes full vocab logits).
            if tracing::enabled!(tracing::Level::DEBUG) && index < 5 {
                let logits_vec = logits.to_vec1::<f32>()?;
                let mut indexed: Vec<(usize, f32)> =
                    logits_vec.iter().copied().enumerate().collect();
                indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                tracing::debug!(
                    "🎲 Token {} - Top5 logits: {:?}",
                    index,
                    &indexed[..5.min(indexed.len())]
                );
            }

            // Sample next token
            let t_sample = std::time::Instant::now();
            let next_token = logits_processor.sample(&logits)?;
            if index < 5 || index % 5 == 0 {
                tracing::info!(
                    "🔢 Token index={} sampled={} in {:.1}ms (total_tokens={})",
                    index,
                    next_token,
                    t_sample.elapsed().as_millis(),
                    tokens.len()
                );
            }

            if tracing::enabled!(tracing::Level::DEBUG) {
                tracing::debug!("🎯 Token {} selected: {}", index, next_token);
            }

            // Check for common EOS tokens
            // Qwen2: 151643 (primary), 151645 (secondary)
            // LLaMA3: 128001, 128009
            // Generic: 2
            if self.is_eos_token(next_token) {
                debug!("EOS token encountered: {next_token}");
                break;
            }

            tokens.push(next_token);

            // Decode token
            if let Ok(token_str) = self.tokenizer.decode(&[next_token], false) {
                // Token filtering: DeepSeek/Qwen → Clarken
                let filtered = self.filter_token(&token_str);

                // Invoke callback
                on_token(&filtered);
                generated.push_str(&filtered);
            }
        }

        self.model.clear_all_kv_caches();
        Ok(self.sanitize_response(&generated))
    }

    /// Filter tokens to replace model names with Clarken branding.
    fn filter_token(&self, token: &str) -> String {
        self.identity
            .apply_origin_replacements(token, &self.identity.assistant_name)
    }

    fn sanitize_response(&self, text: &str) -> String {
        self.identity.sanitize_response(text)
    }

    // ========================
    // Distributed Pipeline API
    // ========================

    /// Execute forward pass for a subset of layers (distributed pipeline).
    ///
    /// # Arguments
    /// - `tokens` — Input token sequence
    /// - `position` — Current position in sequence (for KV cache)
    /// - `layer_start` — First layer to execute (inclusive)
    /// - `layer_end` — Last layer to execute (exclusive)
    ///
    /// # Returns
    /// `ActivationState` containing intermediate tensor and metadata.
    ///
    /// # Limitations
    /// Current implementation uses Candle's opaque ModelWeights API which
    /// doesn't expose individual layer execution. This method performs a
    /// full forward pass and extracts the intermediate state conceptually.
    ///
    /// Production target: replace with low-level layer-by-layer execution
    /// when Candle exposes `model.layers[idx].forward()` API.
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 1: Execute layers 0-10
    /// let activation = model.forward_layer_range(
    ///     &tokens,
    ///     tokens.len(),
    ///     0,
    ///     10
    /// )?;
    /// ```
    pub fn forward_layer_range(
        &mut self,
        tokens: &[u32],
        position: usize,
        layer_start: usize,
        layer_end: usize,
    ) -> Result<ActivationState> {
        debug!(
            "Forward pass with layer range {layer_start}..{layer_end} (pos={position}, tokens={})",
            tokens.len()
        );

        // Validate layer range
        if layer_start >= layer_end {
            anyhow::bail!("Invalid layer range: {layer_start}..{layer_end}");
        }

        // Determine if this is the first stage (needs embeddings)
        let apply_embeddings = layer_start == 0;

        // Determine if this is the final stage (needs norm + LM head)
        let total_layers = self.model.layers.len();
        let apply_final_projection = layer_end == total_layers;

        // Create input tensor
        let input = Tensor::new(tokens, &self.device)?.unsqueeze(0)?;

        // TRUE LAYER-WISE EXECUTION via vendored quantized_qwen2
        // Uses ModelWeights::forward_layer_range() with exposed layers field
        let hidden_states = self.model.forward_layer_range(
            &input,
            layer_start,
            layer_end,
            position,
            apply_embeddings,
            apply_final_projection,
        )?;

        // Extract hidden state shape and data
        let shape = hidden_states.dims().to_vec();
        let data = hidden_states.flatten_all()?.to_vec1::<f32>()?;

        debug!(
            "✅ True layer-range execution: layers {layer_start}..{layer_end}, shape={:?}",
            shape
        );

        Ok(ActivationState {
            shape,
            data,
            position,
            tokens: tokens.to_vec(),
        })
    }

    /// Extract activation state from a tensor (for serialization).
    ///
    /// Converts a Candle Tensor into a serializable ActivationState
    /// for transfer across Aeron IPC.
    ///
    /// # Arguments
    /// - `tensor` — Intermediate hidden state tensor
    /// - `position` — Current sequence position
    /// - `tokens` — Token history
    ///
    /// # Returns
    /// ActivationState ready for MessagePack serialization.
    pub fn extract_activation(
        tensor: &Tensor,
        position: usize,
        tokens: Vec<u32>,
    ) -> Result<ActivationState> {
        let shape = tensor.dims().to_vec();
        let data = tensor.flatten_all()?.to_vec1::<f32>()?;

        debug!(
            "Extracted activation: shape={:?}, data_len={}, pos={}",
            shape,
            data.len(),
            position
        );

        Ok(ActivationState {
            shape,
            data,
            position,
            tokens,
        })
    }

    /// Reconstruct a Candle tensor from an ActivationState (deserialization).
    ///
    /// Converts a received ActivationState back into a Candle Tensor
    /// for continued execution on the next pipeline stage.
    ///
    /// # Arguments
    /// - `activation` — Received activation state
    /// - `device` — Target device (must match model device)
    ///
    /// # Returns
    /// Reconstructed Candle Tensor ready for layer execution.
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 2: Receive activation from Stage 1
    /// let incoming_tensor = GgufModel::reconstruct_activation(&activation, &device)?;
    /// ```
    pub fn reconstruct_activation(activation: &ActivationState, device: &Device) -> Result<Tensor> {
        info!(
            "📥 Reconstructing activation: shape={:?}, data_len={}, pos={}",
            activation.shape,
            activation.data.len(),
            activation.position
        );

        let tensor = Tensor::from_vec(activation.data.clone(), activation.shape.as_slice(), device)
            .context("Failed to reconstruct tensor from activation data")?;

        info!("✅ Reconstructed tensor: dims={:?}", tensor.dims());

        Ok(tensor)
    }

    /// Generate tokens from an intermediate activation (Stage 2/3 entry point).
    ///
    /// Continues generation from a received ActivationState rather than
    /// from tokenized text.
    ///
    /// # Arguments
    /// - `activation` — Received intermediate state
    /// - `layer_start` — First layer to execute on this stage
    /// - `layer_end` — Last layer to execute (exclusive)
    /// - `max_tokens` — Maximum tokens to generate
    /// - `on_token` — Callback for each generated token
    ///
    /// # Returns
    /// Either:
    /// - `Ok(Left(activation))` — Intermediate state for next stage
    /// - `Ok(Right(text))` — Final generated text (from last stage)
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 2: Continue from Stage 1's activation
    /// match model.generate_from_activation(activation, 10, 20, 100, |tok| { ... })? {
    ///     Either::Left(next_activation) => {
    ///         // Send to Stage 3
    ///     }
    ///     Either::Right(final_text) => {
    ///         // Should not happen on Stage 2
    ///     }
    /// }
    /// ```
    pub fn generate_from_activation<F>(
        &mut self,
        activation: &ActivationState,
        layer_start: usize,
        layer_end: usize,
        _max_tokens: usize,
        mut on_token: F,
    ) -> Result<Either<ActivationState, SampledToken>>
    where
        F: FnMut(&str),
    {
        debug!(
            "generate_from_activation: layers {layer_start}..{layer_end}, pos={}, tokens={}",
            activation.position,
            activation.tokens.len()
        );

        // Reconstruct tensor from incoming activation
        let incoming_hidden = Self::reconstruct_activation(activation, &self.device)?;

        // Check if this is the final stage (produces text, not activation)
        let total_layers = self.model.layers.len();
        let is_final_stage = layer_end == total_layers;

        info!(
            "🔍 Calling forward_layer_range: incoming_hidden dims={:?}, layer_start={}, layer_end={}, pos={}, is_final={}",
            incoming_hidden.dims(), layer_start, layer_end, activation.position, is_final_stage
        );

        // Execute layer range WITHOUT final projection (we'll handle it manually)
        let hidden_output = self.model.forward_layer_range(
            &incoming_hidden,
            layer_start,
            layer_end,
            activation.position,
            false, // No embedding - we already have hidden states
            false, // NO final projection yet - we handle last token extraction first
        )?;

        if !is_final_stage {
            // Intermediate stage: Extract activation for next stage
            let shape = hidden_output.dims().to_vec();
            let data = hidden_output.flatten_all()?.to_vec1::<f32>()?;

            info!(
                "✅ Stage {layer_start}..{layer_end} intermediate output: shape={:?}, data_len={}",
                shape,
                data.len()
            );

            return Ok(Either::Left(ActivationState {
                shape,
                data,
                position: activation.position,
                tokens: activation.tokens.clone(),
            }));
        }

        // === FINAL STAGE: Extract last token using pointer arithmetic ===
        // Input shape: [batch=1, seq_len, hidden_size]
        // We need: [1, hidden_size] for the LAST token (rank-2, NOT rank-1!)

        let dims = hidden_output.dims();
        if dims.len() != 3 {
            anyhow::bail!("Expected rank-3 hidden output [batch, seq, hidden], got {dims:?}");
        }

        let seq_len = dims[1];
        // Extract last token but keep batch dimension: [1, 123, 3584] → [1, 3584]
        let last_token_hidden = hidden_output.i((.., seq_len - 1, ..))?;

        info!(
            "✅ Extracted last token: seq_len={}, last_token_shape={:?}",
            seq_len,
            last_token_hidden.dims()
        );

        // Now apply final norm + LM head on the extracted last token [1, 3584]
        info!(
            "🔍 Calling apply_final_projection with shape={:?}",
            last_token_hidden.dims()
        );
        let output_tensor = self.model.apply_final_projection(&last_token_hidden)?;
        info!(
            "✅ apply_final_projection complete, output shape={:?}",
            output_tensor.dims()
        );

        // Final stage: Sample ONE token from logits (distributed decode)
        // In distributed mode, Stage 3 returns after one token. Stage 1 orchestrates
        // the decode loop by sending each new token back through the pipeline.
        let _tokens = activation.tokens.clone();
        let mut logits_processor = self.create_logits_processor();

        // Sample first token from prefill result
        let logits = output_tensor.squeeze(0)?; // [1, vocab_size] → [vocab_size]
        let next_token = logits_processor.sample(&logits)?;

        let is_eos = self.is_eos_token(next_token);
        let mut generated = String::new();
        if let Ok(token_str) = self.tokenizer.decode(&[next_token], false) {
            let filtered = self.filter_token(&token_str);
            on_token(&filtered);
            generated.push_str(&filtered);
        }

        debug!(
            "✅ Stage 3 sampled token {} (EOS={}, distributed decode mode)",
            next_token, is_eos
        );
        Ok(Either::Right(SampledToken {
            token_id: next_token,
            token_text: generated,
            is_eos,
            position: activation.position + 1,
        }))
    }

    /// Generate from initial tokens with layer range restriction (Stage 1 entry point).
    ///
    /// Tokenizes input text and executes only a subset of layers, returning
    /// an intermediate ActivationState for the next stage.
    ///
    /// # Arguments
    /// - `prompt` — Input text
    /// - `layer_start` — First layer to execute (typically 0 for Stage 1)
    /// - `layer_end` — Last layer to execute (exclusive)
    ///
    /// # Returns
    /// ActivationState to be sent to next pipeline stage.
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 1: Process prompt through layers 0-10
    /// let activation = model.generate_from_tokens_layer_range(
    ///     "What is quantization?",
    ///     0,
    ///     10
    /// )?;
    /// // Send activation to Stage 2
    /// ```
    pub fn generate_from_tokens_layer_range(
        &mut self,
        prompt: &str,
        layer_start: usize,
        layer_end: usize,
    ) -> Result<ActivationState> {
        let system_prompt = Self::build_chatml_system_prefix(&self.identity);
        let full_prompt = format!("{system_prompt}{}", Self::build_chatml_user_suffix(prompt));

        debug!("Stage 1 tokenization for layer range {layer_start}..{layer_end}");

        // Tokenize prompt
        let encoding = self
            .tokenizer
            .encode(full_prompt, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        let tokens = encoding.get_ids().to_vec();

        if tokens.len() >= self.max_seq_len {
            anyhow::bail!(
                "Prompt too long: {} tokens (max: {})",
                tokens.len(),
                self.max_seq_len
            );
        }

        // Execute forward pass for layer range
        // Position=0 for prefill (processing full prompt), not tokens.len()!
        self.forward_layer_range(&tokens, 0, layer_start, layer_end)
    }

    /// Execute a decode step: run a single token through layers (for distributed decode orchestration).
    ///
    /// # Arguments
    /// * `token_id` - Single token ID to process
    /// * `position` - Sequence position (for KV cache indexing)
    /// * `layer_start` - First layer to execute (inclusive)
    /// * `layer_end` - Last layer to execute (exclusive)
    ///
    /// # Returns
    /// ActivationState to be sent to next pipeline stage.
    ///
    /// # Example
    /// ```rust,ignore
    /// // Stage 1 decode orchestration: process single token through layers 0-10
    /// let activation = model.decode_single_token(
    ///     token_id,
    ///     current_position,
    ///     0,
    ///     10
    /// )?;
    /// // Send activation to Stage 2
    /// ```
    pub fn decode_single_token(
        &mut self,
        token_id: u32,
        position: usize,
        layer_start: usize,
        layer_end: usize,
    ) -> Result<ActivationState> {
        debug!(
            "Decode step: token_id={}, position={}, layers={}..{}",
            token_id, position, layer_start, layer_end
        );

        // Execute forward pass with single token
        self.forward_layer_range(&[token_id], position, layer_start, layer_end)
    }

    // Helper methods

    fn create_logits_processor(&self) -> LogitsProcessor {
        let sampling = if self.temp <= 0.0 {
            Sampling::ArgMax
        } else {
            Sampling::All {
                temperature: self.temp as f64,
            }
        };
        LogitsProcessor::from_sampling(42, sampling)
    }

    fn is_eos_token(&self, token: u32) -> bool {
        // Qwen2: 151643 (primary), 151645 (secondary)
        // LLaMA3: 128001, 128009
        // Generic: 2
        token == 151643
            || token == 151645
            || token == 128001
            || token == 128009
            || token == 2
            || self.stop_token_ids.contains(&token)
    }
}

/// Either type for stage-dependent return values.
#[derive(Debug)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_filtering() {
        let identity = ClarkenIdentity {
            assistant_name: "ClarkenAI 7B Spark".to_string(),
            runtime_platform: "Blazil infrastructure".to_string(),
            assistant_description: "a financial AI assistant".to_string(),
            system_prompt_suffix: String::new(),
            blocked_origin_terms: IdentityConfig::default().blocked_origin_terms,
        };

        assert_eq!(
            identity.apply_origin_replacements("I am DeepSeek", &identity.assistant_name),
            "I am ClarkenAI 7B Spark"
        );
        assert_eq!(
            identity.apply_origin_replacements("deepseek-coder", &identity.assistant_name),
            "clarkenai 7b spark-coder"
        );
        assert_eq!(
            identity.apply_origin_replacements("Qwen2.5-7B", &identity.assistant_name),
            "ClarkenAI 7B Spark2.5-7B"
        );
        assert_eq!(
            identity.sanitize_response("The Blazil Engine is a model developed by Alibaba Cloud."),
            "The Blazil Engine is a model developed by the Clarken team."
        );
    }

    #[test]
    fn test_chatml_prompt_format() {
        let identity = ClarkenIdentity {
            assistant_name: "ClarkenAI 7B Spark".to_string(),
            runtime_platform: "Blazil infrastructure".to_string(),
            assistant_description: "a financial AI assistant".to_string(),
            system_prompt_suffix: "Stay on-brand.".to_string(),
            blocked_origin_terms: IdentityConfig::default().blocked_origin_terms,
        };

        let system_prefix = GgufModel::build_chatml_system_prefix(&identity);
        let user_suffix = GgufModel::build_chatml_user_suffix("What is credit risk?");

        assert!(system_prefix.starts_with("<|im_start|>system\nYou are ClarkenAI 7B Spark."));
        assert!(system_prefix.contains("Stay on-brand."));
        assert!(system_prefix.ends_with("<|im_end|>\n"));
        assert_eq!(
            user_suffix,
            "<|im_start|>user\nWhat is credit risk?\n<|im_end|>\n<|im_start|>assistant\n"
        );
    }

    #[test]
    fn test_model_validation() {
        // Test that non-existent file returns error
        let result = GgufModel::load(
            "/nonexistent/model.gguf",
            8,
            4096,
            IdentityConfig::default(),
            true,
            None,
        );
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Model file not found"));
    }
}
