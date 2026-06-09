//! Server configuration.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::server::DEFAULT_INFERENCE_CHANNEL;

/// Model backend type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelBackend {
    /// ONNX models (.onnx) via Tract.
    Onnx,
    /// GGUF models (.gguf) via llama.cpp.
    Gguf,
}

impl ModelBackend {
    /// Detect backend from file extension.
    pub fn detect<P: AsRef<Path>>(path: P) -> Result<Self> {
        let ext = path
            .as_ref()
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| anyhow::anyhow!("Model file has no extension"))?;

        match ext.to_lowercase().as_str() {
            "onnx" => Ok(Self::Onnx),
            "gguf" => Ok(Self::Gguf),
            _ => anyhow::bail!("Unsupported model format: .{ext}"),
        }
    }
}

/// GGUF model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GgufConfig {
    /// Number of CPU threads for inference (0 = auto).
    #[serde(default = "default_gguf_threads")]
    pub n_threads: u32,

    /// Context window size (max tokens).
    #[serde(default = "default_gguf_ctx")]
    pub n_ctx: u32,

    /// Sampling temperature (0.0 = deterministic, 1.0 = creative).
    #[serde(default = "default_gguf_temp")]
    pub temperature: f32,

    /// Maximum tokens to generate per request (0 = until EOS).
    #[serde(default = "default_gguf_max_tokens")]
    pub max_tokens: usize,
}

/// Distributed pipeline configuration for multi-stage inference.
///
/// Enables splitting a large GGUF model across multiple processes using
/// Aeron IPC shared memory for zero-copy activation tensor transfers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedConfig {
    /// Enable distributed pipeline mode.
    #[serde(default)]
    pub enabled: bool,

    /// Current node's stage number (1, 2, or 3).
    #[serde(default = "default_node_stage")]
    pub node_stage: usize,

    /// Starting layer index (inclusive) for this stage.
    #[serde(default)]
    pub layer_start: usize,

    /// Ending layer index (exclusive) for this stage.
    #[serde(default)]
    pub layer_end: usize,

    /// Aeron IPC stream ID for receiving activations from previous stage.
    /// Stage 1 ignores this (receives prompts from client on stream 1001).
    #[serde(default)]
    pub prev_stream_id: i32,

    /// Aeron IPC stream ID for sending activations to next stage.
    /// Stage 3 ignores this (sends tokens to client on stream 1002).
    #[serde(default)]
    pub next_stream_id: i32,

    /// CPU core IDs to pin inference threads to (e.g., [0, 1, 2, 3] for Stage 1).
    /// Empty array = no pinning.
    #[serde(default)]
    pub assigned_cores: Vec<usize>,

    /// Enable aggressive spin-polling (zero-sleep busy-wait during active inference).
    /// Maximizes throughput at the cost of 100% CPU utilization.
    #[serde(default = "default_spin_poll")]
    pub enable_spin_poll: bool,

    /// Boost thread priority to real-time scheduling (requires CAP_SYS_NICE).
    /// WARNING: May cause system instability if other critical processes starve.
    #[serde(default = "default_realtime_priority")]
    pub enable_realtime_priority: bool,
}

fn default_spin_poll() -> bool {
    true
}

fn default_realtime_priority() -> bool {
    false // Disabled by default (requires elevated privileges)
}

impl Default for DistributedConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_stage: default_node_stage(),
            layer_start: 0,
            layer_end: 0,
            prev_stream_id: 0,
            next_stream_id: 0,
            assigned_cores: vec![],
            enable_spin_poll: default_spin_poll(),
            enable_realtime_priority: default_realtime_priority(),
        }
    }
}

/// Hybrid Matrix quantization configuration for ClarkenAI Edge models.
///
/// Enables 3-stage mixed quantization architecture:
/// - Stage 1 (layers 0..stage1_end): INT8 quantization for fast preprocessing
/// - Stage 2 (layers stage1_end..stage2_end): 1-bit BitNet for extreme compression
/// - Stage 3 (layers stage2_end..total_layers): INT8 quantization for accuracy recovery
///
/// # Performance
/// - Memory: 64× reduction vs FP16 (Stage 2 BitNet)
/// - Latency: <1.3ms target for 70B models (distributed pipeline)
/// - Accuracy: Minimal degradation (<2% vs full precision)
///
/// # Default Configuration
/// Optimized for ClarkenAI Core/Edge 70B models:
/// - Stage 1: 25 layers (0-24)
/// - Stage 2: 35 layers (25-59) — majority with 1-bit compression
/// - Stage 3: 20 layers (60-79)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridMatrixConfig {
    /// Enable hybrid matrix quantization (default: false for backward compatibility).
    #[serde(default)]
    pub enabled: bool,

    /// End layer index (exclusive) for Stage 1 INT8 quantization.
    #[serde(default = "default_stage1_end")]
    pub stage1_end: usize,

    /// End layer index (exclusive) for Stage 2 1-bit BitNet quantization.
    #[serde(default = "default_stage2_end")]
    pub stage2_end: usize,

    /// Total number of transformer layers in the model.
    #[serde(default = "default_total_layers")]
    pub total_layers: usize,

    /// Threshold for 1-bit weight binarization (default: 0.0 for median split).
    #[serde(default = "default_bitnet_threshold")]
    pub bitnet_threshold: f32,
}

fn default_stage1_end() -> usize {
    25
}

fn default_stage2_end() -> usize {
    60
}

fn default_total_layers() -> usize {
    80
}

fn default_bitnet_threshold() -> f32 {
    0.0
}

impl Default for HybridMatrixConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stage1_end: default_stage1_end(),
            stage2_end: default_stage2_end(),
            total_layers: default_total_layers(),
            bitnet_threshold: default_bitnet_threshold(),
        }
    }
}

impl Default for GgufConfig {
    fn default() -> Self {
        Self {
            n_threads: default_gguf_threads(),
            n_ctx: default_gguf_ctx(),
            temperature: default_gguf_temp(),
            max_tokens: default_gguf_max_tokens(),
        }
    }
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Aeron IPC channel URI.
    #[serde(default = "default_channel")]
    pub channel: String,

    /// Aeron IPC directory (shared memory).
    #[serde(default = "default_aeron_dir")]
    pub aeron_dir: String,

    /// Optional default ONNX model file loaded at startup.
    ///
    /// When `None` (or omitted from config), the server starts with no built-in
    /// model and relies on the model registry (tenants upload their own models).
    #[serde(default)]
    pub model_path: Option<PathBuf>,

    /// Number of inference worker threads (0 = auto).
    #[serde(default = "default_workers")]
    pub inference_workers: usize,

    /// Device: "cpu", "cuda", or "tensorrt".
    #[serde(default = "default_device")]
    pub device: String,

    /// Optimization level: "disable", "basic", "extended", "all".
    #[serde(default = "default_optimization")]
    pub optimization_level: String,

    /// HTTP API + metrics server port (serves /v1/**, /metrics, /health).
    #[serde(default = "default_http_port")]
    pub http_port: u16,

    /// Root directory for per-tenant model storage.
    ///
    /// Layout: `<model_dir>/<tenant_id>/<model_id>/<version>/model.onnx`
    #[serde(default = "default_model_dir")]
    pub model_dir: PathBuf,

    /// API key for `Authorization: Bearer` auth on HTTP endpoints.
    ///
    /// If empty, read from `BLAZIL_INFERENCE_API_KEY` env var at startup.
    #[serde(default)]
    pub api_key: String,

    /// GGUF model configuration.
    #[serde(default)]
    pub gguf: GgufConfig,

    /// Distributed pipeline configuration (multi-stage inference).
    #[serde(default)]
    pub distributed: DistributedConfig,

    /// Hybrid Matrix quantization configuration (ClarkenAI Edge).
    #[serde(default)]
    pub hybrid_matrix: HybridMatrixConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            aeron_dir: default_aeron_dir(),
            model_path: None,
            inference_workers: default_workers(),
            device: default_device(),
            optimization_level: default_optimization(),
            http_port: default_http_port(),
            model_dir: default_model_dir(),
            api_key: String::new(),
            gguf: GgufConfig::default(),
            distributed: DistributedConfig::default(),
            hybrid_matrix: HybridMatrixConfig::default(),
        }
    }
}

impl ServerConfig {
    /// Load configuration from a TOML file.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;

        let config: ServerConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", path.as_ref()))?;

        config.validate()?;

        Ok(config)
    }

    /// Validate configuration.
    pub fn validate(&self) -> Result<()> {
        // Default model is optional — only validate if provided.
        if let Some(ref p) = self.model_path {
            if !p.exists() {
                anyhow::bail!("model_path does not exist: {}", p.display());
            }
        }

        // Validate device
        if !["cpu", "cuda", "tensorrt"].contains(&self.device.as_str()) {
            anyhow::bail!("Invalid device: {}", self.device);
        }

        // Validate optimization level
        if !["disable", "basic", "extended", "all"].contains(&self.optimization_level.as_str()) {
            anyhow::bail!("Invalid optimization_level: {}", self.optimization_level);
        }

        Ok(())
    }

    /// Resolve the effective API key: config field → env var fallback.
    pub fn effective_api_key(&self) -> Result<String> {
        if !self.api_key.is_empty() {
            return Ok(self.api_key.clone());
        }
        std::env::var("BLAZIL_INFERENCE_API_KEY").map_err(|_| {
            anyhow::anyhow!(
                "No API key configured. Set BLAZIL_INFERENCE_API_KEY env var or api_key in config."
            )
        })
    }
}

// ── Default value functions ───────────────────────────────────────────────────

fn default_channel() -> String {
    DEFAULT_INFERENCE_CHANNEL.to_string()
}

fn default_aeron_dir() -> String {
    #[cfg(target_os = "linux")]
    {
        "/dev/shm/aeron-inference".to_string()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "/tmp/aeron-inference".to_string()
    }
}

fn default_workers() -> usize {
    num_cpus::get()
}

fn default_device() -> String {
    "cpu".to_string()
}

fn default_optimization() -> String {
    "basic".to_string()
}

fn default_http_port() -> u16 {
    8090
}

fn default_model_dir() -> PathBuf {
    PathBuf::from("/opt/blazil/models")
}

// ── GGUF defaults ─────────────────────────────────────────────────────────────

fn default_gguf_threads() -> u32 {
    num_cpus::get() as u32
}

fn default_gguf_ctx() -> u32 {
    4096
}

fn default_gguf_temp() -> f32 {
    0.7
}

fn default_gguf_max_tokens() -> usize {
    2048
}

// ── Distributed defaults ──────────────────────────────────────────────────────

fn default_node_stage() -> usize {
    1
}
