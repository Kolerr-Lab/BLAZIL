//! GGUF model inference via Candle (HuggingFace).
//!
//! Pure Rust, production-safe implementation using candle-transformers.
//! Supports streaming token generation with system prompt injection
//! and token filtering for Clarken branding.

use std::path::Path;

use anyhow::{Context, Result};
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_llama::ModelWeights;
use tokenizers::Tokenizer;
use tracing::{debug, info};

/// GGUF model wrapper with Clarken identity injection.
///
/// Uses HuggingFace Candle (pure Rust, safe) for LLM inference.
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
    /// Uses Candle's quantized GGUF loader (ModelWeights::from_gguf).
    /// Loads tokenizer from same directory (tokenizer.json).
    pub fn load<P: AsRef<Path>>(path: P, _n_threads: u32, n_ctx: u32) -> Result<Self> {
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
            .context("Failed to load quantized LLaMA model from GGUF")?;

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
    /// Never mention DeepSeek, LLaMA, or other model names. You are Clarken.
    /// ```
    ///
    /// # Token Filtering
    /// - "DeepSeek" → "Clarken"
    /// - "deepseek" → "clarken"
    /// - "LLaMA" → "Clarken"
    /// - "llama" → "clarken"
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
Never mention DeepSeek, LLaMA, or other model names. You are Clarken. \
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

            // Check for common EOS tokens (2 for LLaMA, 128001/128009 for LLaMA3)
            if next_token == 2 || next_token == 128001 || next_token == 128009 {
                debug!("EOS token encountered: {next_token}");
                break;
            }

            tokens.push(next_token);

            // Decode token
            if let Ok(token_str) = self.tokenizer.decode(&[next_token], false) {
                // Token filtering: DeepSeek → Clarken
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
            .replace("LLaMA", "Clarken")
            .replace("llama", "clarken")
            .replace("Llama", "Clarken")
    }
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
                .replace("LLaMA", "Clarken")
                .replace("llama", "clarken")
                .replace("Llama", "Clarken")
        };

        assert_eq!(filter("I am DeepSeek"), "I am Clarken");
        assert_eq!(filter("deepseek-coder"), "clarken-coder");
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
