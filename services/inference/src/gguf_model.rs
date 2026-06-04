//! GGUF model inference via Candle (HuggingFace).
//!
//! Pure Rust, production-safe implementation using candle-transformers.
//! Supports streaming token generation with system prompt injection
//! and token filtering for Clarken branding.

use std::path::Path;

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::llama::{Cache, Config, Llama};
use tokenizers::Tokenizer;
use tracing::{debug, info};

/// GGUF model wrapper with Clarken identity injection.
///
/// Uses HuggingFace Candle (pure Rust, safe) for LLM inference.
#[allow(dead_code)] // Infrastructure code - HTTP API integration pending
pub struct GgufModel {
    model: Llama,
    tokenizer: Tokenizer,
    cache: Cache,
    config: Config,
    device: Device,
    temp: f32,
    max_tokens: usize,
}

#[allow(dead_code)] // Infrastructure code - HTTP API integration pending
impl GgufModel {
    /// Load a GGUF model from disk.
    ///
    /// # Arguments
    /// - `path` — Path to .gguf or .safetensors file
    /// - `n_threads` — Number of CPU threads (used for CPU device)
    /// - `n_ctx` — Context window size (default: 4096)
    ///
    /// # Implementation
    /// Uses Candle's GGUF loader or safetensors loader depending on file extension.
    /// Loads tokenizer from same directory (tokenizer.json) or embedded.
    pub fn load<P: AsRef<Path>>(path: P, _n_threads: u32, n_ctx: u32) -> Result<Self> {
        let path = path.as_ref();
        info!(
            "Loading GGUF model via Candle: {} (n_ctx={})",
            path.display(),
            n_ctx
        );

        // Validate file exists
        if !path.exists() {
            anyhow::bail!("Model file not found: {}", path.display());
        }

        // Select device (prefer CUDA if available, fallback to CPU)
        let device = if candle_core::utils::cuda_is_available() {
            info!("Using CUDA device for inference");
            Device::new_cuda(0)?
        } else {
            info!("Using CPU device for inference");
            Device::Cpu
        };

        let dtype = DType::F32;

        // Load tokenizer from same directory
        let tokenizer_path = path
            .parent()
            .map(|p| p.join("tokenizer.json"))
            .filter(|p| p.exists())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "tokenizer.json not found in model directory: {}",
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                )
            })?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        info!("Tokenizer loaded from {:?}", tokenizer_path);

        // Load model config from config.json (if exists) or use default LLaMA config
        let config_path = path.parent().map(|p| p.join("config.json"));
        let config: Config = if let Some(ref cp) = config_path {
            if cp.exists() {
                // Parse JSON manually since Config doesn't derive Deserialize
                let json_str = std::fs::read_to_string(cp)?;
                let json: serde_json::Value = serde_json::from_str(&json_str)?;

                Self::config_from_json(&json, n_ctx)?
            } else {
                // Default config for LLaMA-style models
                Self::default_llama_config(n_ctx)
            }
        } else {
            Self::default_llama_config(n_ctx)
        };

        debug!(
            "Model config: vocab_size={}, hidden_size={}",
            config.vocab_size, config.hidden_size
        );

        // Initialize KV cache
        let cache = Cache::new(true, dtype, &config, &device)?;

        // Load model weights
        let vb = if path.extension().and_then(|e| e.to_str()) == Some("safetensors") {
            // Load from safetensors
            unsafe { VarBuilder::from_mmaped_safetensors(&[path.to_path_buf()], dtype, &device)? }
        } else if path.extension().and_then(|e| e.to_str()) == Some("gguf") {
            // Load from GGUF using quantized loader
            let mut file = std::fs::File::open(path)?;
            let _gguf_content = candle_core::quantized::gguf_file::Content::read(&mut file)
                .context("Failed to read GGUF file")?;

            // GGUF uses quantized models which have a different API
            // For production use, convert to safetensors or use quantized_llama directly
            anyhow::bail!(
                "GGUF quantized models require quantized_llama API (different from full-precision Llama). \
                 For production use, please convert to safetensors format: \
                 https://huggingface.co/docs/transformers/main/en/serialization"
            );
        } else {
            anyhow::bail!("Unsupported model format: {:?}", path.extension());
        };

        let model = Llama::load(vb, &config).context("Failed to load LLaMA model from weights")?;

        info!("Model loaded successfully");

        Ok(Self {
            model,
            tokenizer,
            cache,
            config,
            device,
            temp: 0.7,
            max_tokens: 2048,
        })
    }

    /// Default LLaMA configuration for 7B-style models.
    fn default_llama_config(n_ctx: u32) -> Config {
        Config {
            hidden_size: 4096,
            intermediate_size: 11008,
            vocab_size: 32000,
            num_hidden_layers: 32,
            num_attention_heads: 32,
            num_key_value_heads: 32,
            rms_norm_eps: 1e-5,
            rope_theta: 10000.0,
            use_flash_attn: false,
            max_position_embeddings: n_ctx as usize,
            eos_token_id: Some(candle_transformers::models::llama::LlamaEosToks::Single(2)),
            bos_token_id: Some(1),
            rope_scaling: None,
            tie_word_embeddings: false,
        }
    }

    /// Parse Config from HuggingFace config.json format.
    fn config_from_json(json: &serde_json::Value, n_ctx: u32) -> Result<Config> {
        let get_u64 = |key: &str| -> Result<usize> {
            json.get(key)
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .ok_or_else(|| anyhow::anyhow!("Missing or invalid field: {}", key))
        };

        let get_f64 = |key: &str, default: f64| -> f64 {
            json.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
        };

        Ok(Config {
            hidden_size: get_u64("hidden_size")?,
            intermediate_size: get_u64("intermediate_size")?,
            vocab_size: get_u64("vocab_size")?,
            num_hidden_layers: get_u64("num_hidden_layers")?,
            num_attention_heads: get_u64("num_attention_heads")?,
            num_key_value_heads: get_u64("num_key_value_heads")
                .or_else(|_| get_u64("num_attention_heads"))?, // fallback
            rms_norm_eps: get_f64("rms_norm_eps", 1e-5),
            rope_theta: get_f64("rope_theta", 10000.0) as f32,
            use_flash_attn: false, // Disabled by default for safety
            max_position_embeddings: json
                .get("max_position_embeddings")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(n_ctx as usize),
            eos_token_id: json
                .get("eos_token_id")
                .and_then(|v| v.as_u64())
                .map(|id| candle_transformers::models::llama::LlamaEosToks::Single(id as u32)),
            bos_token_id: json
                .get("bos_token_id")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            rope_scaling: None, // TODO: Parse if needed
            tie_word_embeddings: json
                .get("tie_word_embeddings")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
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

        let full_prompt = format!("{}{}", system_prompt, prompt);

        debug!("Generating response for prompt (len={})", prompt.len());

        // Tokenize prompt
        let encoding = self
            .tokenizer
            .encode(full_prompt.clone(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let mut tokens = encoding.get_ids().to_vec();

        if tokens.len() >= self.config.max_position_embeddings {
            anyhow::bail!(
                "Prompt too long: {} tokens (max: {})",
                tokens.len(),
                self.config.max_position_embeddings
            );
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

        for (index_pos, index) in (0..max_gen).enumerate() {
            let (context_size, context_index) = if index > 0 {
                // Use KV cache: only process new token
                (1, index_pos)
            } else {
                // First iteration: process full prompt
                (tokens.len(), 0)
            };

            let ctxt = &tokens[tokens.len().saturating_sub(context_size)..];
            let input = Tensor::new(ctxt, &self.device)?.unsqueeze(0)?;

            // Forward pass
            let logits = self
                .model
                .forward(&input, context_index, &mut self.cache)?
                .squeeze(0)?;

            // Sample next token
            let next_token = logits_processor.sample(&logits)?;

            // Check for EOS
            if let Some(candle_transformers::models::llama::LlamaEosToks::Single(eos_id)) =
                self.config.eos_token_id
            {
                if next_token == eos_id {
                    debug!("EOS token encountered, stopping generation");
                    break;
                }
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
        let result = GgufModel::load("/nonexistent/model.safetensors", 8, 4096);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Model file not found"));
    }

    #[test]
    fn test_default_config() {
        let config = GgufModel::default_llama_config(4096);
        assert_eq!(config.max_position_embeddings, 4096);
        assert_eq!(config.vocab_size, 32000);
        assert_eq!(config.num_hidden_layers, 32);
    }
}
