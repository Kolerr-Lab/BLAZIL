//! Blazil Inference Server — Aeron IPC + HTTP API.
//!
//! Runs two servers concurrently in the same process:
//!
//! 1. **Aeron IPC** (`aeron:ipc`) — ultra-low latency inference for internal
//!    services (stream 2001 → 2002, MessagePack).
//!
//! 2. **HTTP API** (`http_port`, default 8090) — REST inference + model
//!    management for external SaaS tenants.
//!
//! Both servers share the same model (or model registry) and metrics instance.
//!
//! # Usage
//!
//! ```bash
//! # With config file:
//! ./inference-server --config config.toml
//!
//! # CLI overrides:
//! ./inference-server --http-port 9000 --model squeezenet.onnx --workers 8
//! ```

mod aeron_server;
mod config;
mod gguf_model;
mod http_api;
mod metrics;
mod model_registry;
mod models; // Vendored model architectures with distributed pipeline support
mod protocol;
mod server;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{error, info};

use blazil_inference::{InferenceConfig, InferenceModel, OnnxModel, OptimizationLevel};
use blazil_transport::server::TransportServer;

use crate::config::{ModelBackend as ConfigBackend, ServerConfig};
use crate::gguf_model::GgufModel;
use crate::http_api::AppState;
use crate::metrics::InferenceMetrics;
use crate::model_registry::ModelRegistry;
use crate::server::{AeronInferenceServer, ModelBackend as AeronBackend};

// ── CLI Arguments ─────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "inference-server")]
#[command(about = "Blazil Inference Server (Aeron IPC + HTTP API)", long_about = None)]
struct Args {
    /// Path to configuration file (TOML).
    #[arg(short, long)]
    config: Option<String>,

    /// Path to default ONNX model file (optional; tenants can use the registry).
    #[arg(long)]
    model: Option<String>,

    /// Aeron IPC channel URI.
    #[arg(long)]
    channel: Option<String>,

    /// Number of inference worker threads (0 = auto).
    #[arg(long)]
    workers: Option<usize>,

    /// Optimization level: disable, basic, extended, all.
    #[arg(long)]
    optimization: Option<String>,

    /// HTTP API + metrics server port (default: 8090).
    #[arg(long)]
    http_port: Option<u16>,

    /// Root directory for per-tenant model storage.
    #[arg(long)]
    model_dir: Option<String>,
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Blazil Inference Server starting...");

    let args = Args::parse();

    // ── Load configuration ────────────────────────────────────────────────────
    let mut config = if let Some(ref path) = args.config {
        ServerConfig::from_file(path).context("Failed to load config file")?
    } else {
        ServerConfig::default()
    };

    // CLI overrides.
    if let Some(model) = args.model {
        config.model_path = Some(model.into());
    }
    if let Some(channel) = args.channel {
        config.channel = channel;
    }
    if let Some(workers) = args.workers {
        config.inference_workers = workers;
    }
    if let Some(opt) = args.optimization {
        config.optimization_level = opt;
    }
    if let Some(port) = args.http_port {
        config.http_port = port;
    }
    if let Some(dir) = args.model_dir {
        config.model_dir = dir.into();
    }

    config.validate().context("Invalid configuration")?;

    let opt_level = parse_optimization(&config.optimization_level)?;

    // ── API key ───────────────────────────────────────────────────────────────
    let api_key = config
        .effective_api_key()
        .context("Cannot start HTTP API without an API key")?;

    // ── Shared components ─────────────────────────────────────────────────────
    let metrics = Arc::new(InferenceMetrics::new()?);

    let registry = Arc::new(
        ModelRegistry::new(&config.model_dir, opt_level)
            .context("Failed to initialise model registry")?,
    );

    // ── Optional default model ────────────────────────────────────────────────
    let (default_model, aeron_backend): (Option<Arc<OnnxModel>>, Option<AeronBackend>) =
        if let Some(ref path) = config.model_path {
            // Detect backend from file extension
            let backend = ConfigBackend::detect(path).context("Failed to detect model backend")?;

            match backend {
                ConfigBackend::Onnx => {
                    info!(path = %path.display(), "Loading default ONNX model...");
                    let cfg = InferenceConfig::new(path)
                        .with_threads(config.inference_workers, config.inference_workers)
                        .with_optimization(opt_level);
                    let model = tokio::task::spawn_blocking(move || OnnxModel::load(cfg))
                        .await
                        .context("spawn_blocking join error")?
                        .context("Failed to load default ONNX model")?;
                    info!(
                        input_shape = ?model.input_shape(),
                        num_classes = ?model.num_classes(),
                        "ONNX model loaded"
                    );
                    let model_arc = Arc::new(model);
                    (
                        Some(Arc::clone(&model_arc)),
                        Some(AeronBackend::Onnx(model_arc)),
                    )
                }
                ConfigBackend::Gguf => {
                    info!(path = %path.display(), "Loading default GGUF model...");
                    let path_clone = path.clone();
                    let n_threads = config.gguf.n_threads;
                    let n_ctx = config.gguf.n_ctx;
                    let mut model = tokio::task::spawn_blocking(move || {
                        GgufModel::load(&path_clone, n_threads, n_ctx)
                    })
                    .await
                    .context("spawn_blocking join error")?
                    .context("Failed to load default GGUF model")?;

                    // Configure temperature and max_tokens
                    model.set_temperature(config.gguf.temperature);
                    model.set_max_tokens(config.gguf.max_tokens);

                    info!(
                        n_ctx = n_ctx,
                        temp = config.gguf.temperature,
                        "GGUF model loaded — Aeron IPC + HTTP API enabled"
                    );

                    // Wrap in Arc<Mutex<>> for Aeron IPC (interior mutability for generate_streaming)
                    let gguf_arc = Arc::new(Mutex::new(model));
                    (None, Some(AeronBackend::Gguf(gguf_arc)))
                }
            }
        } else {
            info!("No default model configured — tenants use the model registry");
            (None, None)
        };

    // ── HTTP API server ───────────────────────────────────────────────────────
    let http_addr: SocketAddr = ([0, 0, 0, 0], config.http_port).into();
    let app_state = AppState {
        registry: Arc::clone(&registry),
        metrics: Arc::clone(&metrics),
        default_model: default_model.clone(),
        api_key: Arc::new(api_key),
    };

    let http_handle = tokio::spawn(async move {
        if let Err(e) = http_api::serve(app_state, http_addr).await {
            error!("HTTP API server error: {e}");
        }
    });

    // ── Aeron IPC server ──────────────────────────────────────────────────────
    // Spawn dedicated thread for Aeron IPC (GGUF models only).
    // ONNX models continue using tokio-based AeronInferenceServer.
    if let Some(backend) = aeron_backend {
        match backend {
            AeronBackend::Gguf(gguf_model) => {
                // Spawn dedicated std::thread for GGUF + Aeron IPC
                // (Aeron FFI is not Send/Sync — requires OS thread)
                let aeron_dir = config.aeron_dir.clone();
                let distributed = if config.distributed.enabled {
                    Some(config.distributed.clone())
                } else {
                    None
                };
                std::thread::Builder::new()
                    .name("aeron-gguf-listener".to_string())
                    .spawn(move || {
                        aeron_server::run(gguf_model, &aeron_dir, distributed);
                    })
                    .expect("Failed to spawn Aeron IPC thread");

                info!(
                    aeron_dir = %config.aeron_dir,
                    distributed = config.distributed.enabled,
                    "🚀 GGUF Aeron IPC listener started (dedicated thread)"
                );
                info!("blazil-inference-server ready (GGUF + HTTP)");

                // Block on HTTP server (Aeron thread runs independently)
                let _ = http_handle.await;
            }
            AeronBackend::Onnx(onnx_model) => {
                // ONNX models use existing AeronInferenceServer (tokio-based)
                let aeron_server = Arc::new(AeronInferenceServer::new(
                    &config.channel,
                    &config.aeron_dir,
                    AeronBackend::Onnx(onnx_model),
                ));

                info!(
                    channel = %config.channel,
                    "🚀 ONNX Aeron IPC inference server starting"
                );
                info!("blazil-inference-server ready (ONNX + HTTP)");

                let server_clone = Arc::clone(&aeron_server);
                tokio::spawn(async move {
                    shutdown_signal().await;
                    info!("Shutdown signal received");
                    server_clone.shutdown().await;
                });

                if let Err(e) = aeron_server.serve().await {
                    error!("Aeron inference server error: {e}");
                }
            }
        }
    } else {
        info!("🚀 HTTP-only mode — Aeron IPC disabled (no default model)");
        info!("blazil-inference-server ready");
        // Block until http task exits (Ctrl+C / SIGTERM handled inside axum).
        let _ = http_handle.await;
    }

    info!("Server stopped");
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_optimization(s: &str) -> Result<OptimizationLevel> {
    match s.to_lowercase().as_str() {
        "disable" => Ok(OptimizationLevel::Disable),
        "basic" => Ok(OptimizationLevel::Basic),
        "extended" => Ok(OptimizationLevel::Extended),
        "all" => Ok(OptimizationLevel::All),
        _ => anyhow::bail!("Invalid optimization level: {s}"),
    }
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
