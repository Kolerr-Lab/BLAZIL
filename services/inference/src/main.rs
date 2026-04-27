//! Aeron IPC Inference Server - Production-grade ML model serving.
//!
//! Uses Aeron IPC transport for ultra-low latency inference requests.
//!
//! # Architecture
//!
//! ```text
//! Client → Aeron:IPC (stream 2001) → InferenceServer
//!   → ONNX Model (Tract) → Result
//!   → Aeron:IPC (stream 2002) → Client
//! ```
//!
//! # Example
//!
//! ```bash
//! # Using config file
//! ./inference-server --config config.toml
//!
//! # Using CLI arguments
//! ./inference-server --model squeezenet1.1.onnx --workers 8
//! ```

mod config;
mod metrics;
mod protocol;
mod server;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::signal;
use tracing::{error, info};
use warp::Filter;

use blazil_inference::{InferenceConfig, InferenceModel, OnnxModel, OptimizationLevel};
use blazil_transport::server::TransportServer;

use crate::config::ServerConfig;
use crate::metrics::InferenceMetrics;
use crate::server::AeronInferenceServer;

// ── CLI Arguments ─────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "inference-server")]
#[command(about = "Aeron IPC Inference Server", long_about = None)]
struct Args {
    /// Path to configuration file (TOML)
    #[arg(short, long)]
    config: Option<String>,

    /// Path to ONNX model file
    #[arg(long)]
    model: Option<String>,

    /// Aeron IPC channel URI
    #[arg(long)]
    channel: Option<String>,

    /// Number of inference worker threads (0 = auto)
    #[arg(long)]
    workers: Option<usize>,

    /// Optimization level: disable, basic, extended, all
    #[arg(long)]
    optimization: Option<String>,

    /// Metrics HTTP server port
    #[arg(long)]
    metrics_port: Option<u16>,
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Blazil Inference Server starting...");

    // Parse CLI arguments
    let args = Args::parse();

    // Load configuration
    let mut config = if let Some(ref path) = args.config {
        ServerConfig::from_file(path).context("Failed to load config file")?
    } else {
        ServerConfig::default()
    };

    // Override with CLI arguments
    if let Some(model) = args.model {
        config.model_path = model.into();
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
    if let Some(port) = args.metrics_port {
        config.metrics_port = port;
    }

    // Validate configuration
    config.validate().context("Invalid configuration")?;

    info!(
        model = %config.model_path.display(),
        channel = %config.channel,
        workers = config.inference_workers,
        "Configuration loaded"
    );

    // Create metrics registry
    let metrics = Arc::new(InferenceMetrics::new()?);

    // Spawn metrics HTTP server
    if config.enable_metrics {
        let metrics_clone = Arc::clone(&metrics);
        let metrics_port = config.metrics_port;

        tokio::spawn(async move {
            if let Err(e) = run_metrics_server(metrics_clone, metrics_port).await {
                error!("Metrics server error: {e}");
            }
        });

        info!("Metrics server listening on port {}", config.metrics_port);
    }

    // Build inference config
    let inference_config = InferenceConfig::new(&config.model_path)
        .with_threads(config.inference_workers, config.inference_workers)
        .with_optimization(parse_optimization(&config.optimization_level)?);

    info!("Loading ONNX model...");

    // Load model (blocking operation, spawn to avoid blocking tokio runtime)
    let model = tokio::task::spawn_blocking(move || OnnxModel::load(inference_config))
        .await
        .context("Task join error")?
        .context("Failed to load ONNX model")?;

    let model = Arc::new(model);

    info!(
        input_shape = ?model.input_shape(),
        num_classes = ?model.num_classes(),
        "Model loaded successfully"
    );

    // Create Aeron inference server
    let server = Arc::new(AeronInferenceServer::new(
        &config.channel,
        &config.aeron_dir,
        Arc::clone(&model),
    ));

    // Setup graceful shutdown
    let server_clone = Arc::clone(&server);
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Shutdown signal received");
        server_clone.shutdown().await;
    });

    // Start server
    info!("Starting Aeron IPC inference server...");
    server.serve().await?;

    info!("Server stopped");
    Ok(())
}

// ── Helper Functions ──────────────────────────────────────────────────────────

/// Parse optimization level string.
fn parse_optimization(s: &str) -> Result<OptimizationLevel> {
    match s.to_lowercase().as_str() {
        "disable" => Ok(OptimizationLevel::Disable),
        "basic" => Ok(OptimizationLevel::Basic),
        "extended" => Ok(OptimizationLevel::Extended),
        "all" => Ok(OptimizationLevel::All),
        _ => anyhow::bail!("Invalid optimization level: {}", s),
    }
}

/// Run Prometheus metrics HTTP server.
async fn run_metrics_server(metrics: Arc<InferenceMetrics>, port: u16) -> Result<()> {
    let metrics_route = warp::path("metrics").map(move || match metrics.export() {
        Ok(body) => warp::reply::with_status(body, warp::http::StatusCode::OK),
        Err(e) => {
            error!("Failed to export metrics: {e}");
            warp::reply::with_status(
                format!("Error: {e}"),
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    });

    let health_route =
        warp::path("health").map(|| warp::reply::with_status("OK", warp::http::StatusCode::OK));

    let routes = metrics_route.or(health_route);

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    info!("Metrics server listening on http://{}", addr);

    warp::serve(routes).run(addr).await;

    Ok(())
}

/// Wait for shutdown signal (Ctrl+C or SIGTERM).
async fn shutdown_signal() {
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
