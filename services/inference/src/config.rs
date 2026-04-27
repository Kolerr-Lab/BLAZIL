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

    /// Path to the ONNX model file.
    pub model_path: PathBuf,

    /// Number of inference worker threads (0 = auto).
    #[serde(default = "default_workers")]
    pub inference_workers: usize,

    /// Device: "cpu", "cuda", or "tensorrt".
    #[serde(default)]
    pub device: String,

    /// Optimization level: "disable", "basic", "extended", "all".
    #[serde(default = "default_optimization")]
    pub optimization_level: String,

    /// Enable Prometheus metrics HTTP server.
    #[serde(default = "default_enable_metrics")]
    pub enable_metrics: bool,

    /// Metrics HTTP server port.
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            aeron_dir: default_aeron_dir(),
            model_path: PathBuf::from("model.onnx"),
            inference_workers: default_workers(),
            device: "cpu".to_string(),
            optimization_level: default_optimization(),
            enable_metrics: default_enable_metrics(),
            metrics_port: default_metrics_port(),
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
        // Check model file exists
        if !self.model_path.exists() {
            anyhow::bail!("Model file does not exist: {}", self.model_path.display());
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

fn default_optimization() -> String {
    "basic".to_string()
}

fn default_enable_metrics() -> bool {
    true
}

fn default_metrics_port() -> u16 {
    9091
}
