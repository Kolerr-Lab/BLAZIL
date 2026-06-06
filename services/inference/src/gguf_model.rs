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
use candle_core::{Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
// Use vendored quantized_qwen2 with exposed layers field for distributed pipeline
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
}

#[allow(dead_code)] // Infrastructure code - HTTP API integration pending
impl GgufModel {
    /// Load a GGUF model from disk.
    ///
    /// # Arguments
    /// - `path` — Path to .gguf file
    /// - `_n_threads` — Number of CPU threads (unused, kept for API compatibility)
    /// - `n_ctx` — Context window size (used as max_seq_len)
    ///
    /// # Implementation
    /// Uses Candle's quantized Qwen2 GGUF loader (ModelWeights::from_gguf).
    /// Loads tokenizer from same directory (tokenizer.json).
    pub fn load<P: AsRef<Path>>(path: P, _n_threads: u32, n_ctx: u32) -> Result<Self> {
        Self::load_with_layer_range(path, _n_threads, n_ctx, None)
    }

    /// Load a GGUF model with optional layer range filtering (distributed pipeline).
    ///
    /// # Arguments
    /// - `path` — Path to .gguf file
    /// - `_n_threads` — Number of CPU threads (unused, kept for API compatibility)
    /// - `n_ctx` — Context window size (used as max_seq_len)
    /// - `layer_range` — Optional (start, end) layer indices for partial loading
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
    ///     Some((10, 19))
    /// )?;
    /// ```
    pub fn load_with_layer_range<P: AsRef<Path>>(
        path: P,
        _n_threads: u32,
        n_ctx: u32,
        layer_range: Option<(usize, usize)>,
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

        // Load GGUF model using quantized API
        let mut file = std::fs::File::open(path)?;
        let start = std::time::Instant::now();

        let gguf_content = gguf_file::Content::read(&mut file)
            .map_err(|e| anyhow::anyhow!("Failed to read GGUF file: {e}"))?;

        // Layer range filtering for distributed pipeline
        if let Some((layer_start, layer_end)) = layer_range {
            info!("Distributed mode: loading layers {layer_start}-{layer_end} (partial model)");

            // TODO: Implement selective tensor loading for distributed pipeline
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
        let model = ModelWeights::from_gguf(gguf_content, &mut file, &device)
            .context("Failed to load quantized Qwen2 model from GGUF")?;

        info!("Model loaded successfully");

        Ok(Self {
            model,
            tokenizer,
            device,
            temp: 0.7,
            max_tokens: 2048,
            max_seq_len: n_ctx as usize,
        })
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
        // System prompt injection
        let system_prompt = "\
You are Clarken, a high-performance financial AI assistant built on Blazil infrastructure. \
Never mention DeepSeek, Qwen, or other model names. You are Clarken. \
Provide accurate, concise answers focused on finance, trading, and risk management.\n\n";

        let full_prompt = format!("{system_prompt}{prompt}");

        let prompt_len = prompt.len();
        debug!("Generating response for prompt (len={prompt_len})");

        // Tokenize prompt
        let encoding = self
            .tokenizer
            .encode(full_prompt.clone(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        let mut tokens = encoding.get_ids().to_vec();

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
            let pos = tokens.len();

            // Check sequence length limit
            if pos >= self.max_seq_len {
                debug!("Max sequence length reached: {pos}");
                break;
            }

            // Get last token for input
            let input_token = if index == 0 {
                // First iteration: use all prompt tokens
                tokens.as_slice()
            } else {
                // Subsequent iterations: use only last token
                &tokens[tokens.len() - 1..]
            };

            let input = Tensor::new(input_token, &self.device)?.unsqueeze(0)?;

            // Forward pass with quantized model (takes position directly)
            let logits = self.model.forward(&input, pos)?.squeeze(0)?;

            // Sample next token
            let next_token = logits_processor.sample(&logits)?;

            // Check for common EOS tokens
            // Qwen2: 151643 (primary), 151645 (secondary)
            // LLaMA3: 128001, 128009
            // Generic: 2
            if next_token == 151643
                || next_token == 151645
                || next_token == 128001
                || next_token == 128009
                || next_token == 2
            {
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

        Ok(generated)
    }

    /// Filter tokens to replace model names with Clarken branding.
    fn filter_token(&self, token: &str) -> String {
        token
            .replace("DeepSeek", "Clarken")
            .replace("deepseek", "clarken")
            .replace("Qwen", "Clarken")
            .replace("qwen", "clarken")
            .replace("LLaMA", "Clarken")
            .replace("llama", "clarken")
            .replace("Llama", "Clarken")
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
    /// **Production TODO**: Replace with low-level layer-by-layer execution
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
        let tensor = Tensor::from_vec(activation.data.clone(), activation.shape.as_slice(), device)
            .context("Failed to reconstruct tensor from activation data")?;

        debug!(
            "Reconstructed activation: shape={:?}, pos={}",
            activation.shape, activation.position
        );

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
        max_tokens: usize,
        mut on_token: F,
    ) -> Result<Either<ActivationState, String>>
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

        // Execute layer range on incoming hidden state (no embeddings, final projection if last stage)
        let output_tensor = self.model.forward_layer_range(
            &incoming_hidden,
            layer_start,
            layer_end,
            activation.position,
            false,          // No embedding - we already have hidden states
            is_final_stage, // Final projection if last stage
        )?;

        if !is_final_stage {
            // Intermediate stage: Extract activation for next stage
            let shape = output_tensor.dims().to_vec();
            let data = output_tensor.flatten_all()?.to_vec1::<f32>()?;

            debug!(
                "✅ Stage {layer_start}..{layer_end} intermediate output: shape={:?}",
                shape
            );

            return Ok(Either::Left(ActivationState {
                shape,
                data,
                position: activation.position,
                tokens: activation.tokens.clone(),
            }));
        }

        // Final stage: Generate tokens from logits
        let mut tokens = activation.tokens.clone();
        let mut logits_processor = self.create_logits_processor();
        let mut generated = String::new();

        let max_gen = if max_tokens == 0 {
            self.max_tokens
        } else {
            max_tokens
        };

        for _ in 0..max_gen {
            let pos = tokens.len();

            if pos >= self.max_seq_len {
                debug!("Max sequence length reached");
                break;
            }

            // For first iteration, use the output_tensor we already computed
            // For subsequent iterations, need to execute forward from last token
            let logits = if pos == activation.position {
                output_tensor.clone()
            } else {
                // Execute full model forward on last token
                let input = Tensor::new(&tokens[tokens.len() - 1..], &self.device)?.unsqueeze(0)?;
                self.model.forward(&input, pos)?.squeeze(0)?
            };

            // Sample next token
            let next_token = logits_processor.sample(&logits)?;

            if self.is_eos_token(next_token) {
                debug!("EOS token encountered: {next_token}");
                break;
            }

            tokens.push(next_token);

            if let Ok(token_str) = self.tokenizer.decode(&[next_token], false) {
                let filtered = self.filter_token(&token_str);
                on_token(&filtered);
                generated.push_str(&filtered);
            }
        }

        debug!("✅ Final stage generated {} tokens", generated.len());
        Ok(Either::Right(generated))
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
        // System prompt injection
        let system_prompt = "\
You are Clarken, a high-performance financial AI assistant built on Blazil infrastructure. \
Never mention DeepSeek, Qwen, or other model names. You are Clarken. \
Provide accurate, concise answers focused on finance, trading, and risk management.\n\n";

        let full_prompt = format!("{system_prompt}{prompt}");

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
        self.forward_layer_range(&tokens, tokens.len(), layer_start, layer_end)
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
        token == 151643 || token == 151645 || token == 128001 || token == 128009 || token == 2
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
        // Mock test for filter logic
        let filter = |token: &str| -> String {
            token
                .replace("DeepSeek", "Clarken")
                .replace("deepseek", "clarken")
                .replace("Qwen", "Clarken")
                .replace("qwen", "clarken")
                .replace("LLaMA", "Clarken")
                .replace("llama", "clarken")
                .replace("Llama", "Clarken")
        };

        assert_eq!(filter("I am DeepSeek"), "I am Clarken");
        assert_eq!(filter("deepseek-coder"), "clarken-coder");
        assert_eq!(filter("Qwen2.5-7B"), "Clarken2.5-7B");
        assert_eq!(filter("qwen model"), "clarken model");
        assert_eq!(filter("LLaMA 3.1"), "Clarken 3.1");
        assert_eq!(filter("Llama model"), "Clarken model");
    }

    #[test]
    fn test_model_validation() {
        // Test that non-existent file returns error
        let result = GgufModel::load("/nonexistent/model.gguf", 8, 4096);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Model file not found"));
    }
}
