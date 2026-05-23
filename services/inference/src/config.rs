//! Server configuration.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::server::DEFAULT_INFERENCE_CHANNEL;

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
