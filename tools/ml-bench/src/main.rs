// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Blazil ML Benchmark Tool
//!
//! Measures dataset streaming throughput (dataloader) and end-to-end inference
//! performance (inference mode) with real-time dashboard integration.
//!
//! # Usage
//! ```bash
//! # Dataloader mode with live dashboard
//! ./target/release/ml-bench \
//!   --mode dataloader \
//!   --dataset imagenet \
//!   --path /data/imagenet \
//!   --batch-size 256 \
//!   --duration 120 \
//!   --metrics-port 9092
//!
//! # Inference mode
//! ./target/release/ml-bench \
//!   --mode inference \
//!   --model models/squeezenet1.1.onnx \
//!   --dataset imagenet \
//!   --path /data/imagenet \
//!   --metrics-port 9092
//! ```

#[cfg(feature = "metrics-ws")]
mod ws_server;

use blazil_dataloader::{datasets::ImageNetDataset, Dataset, DatasetConfig, Pipeline};
use blazil_inference::{InferenceConfig, InferenceModel, InferencePipeline, OnnxModel};
use clap::Parser;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::timeout;

#[cfg(feature = "metrics-ws")]
use serde_json::json;

// ─────────────────────────────────────────────
// CLI Arguments
// ─────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "ml-bench")]
#[command(version = "0.1.0")]
#[command(about = "Blazil ML benchmark — dataloader and inference throughput")]
struct Args {
    /// Benchmark mode: dataloader (dataset streaming) or inference (end-to-end)
    #[arg(long, default_value = "dataloader")]
    mode: String,

    /// Path to ONNX model (required for inference mode)
    #[arg(long)]
    model: Option<PathBuf>,

    /// Inference workers (parallel inference threads)
    #[arg(long, default_value_t = 4)]
    inference_workers: usize,

    /// Dataset type (currently: imagenet)
    #[arg(long, default_value = "imagenet")]
    dataset: String,

    /// Path to dataset root directory
    #[arg(long)]
    path: String,

    /// Batch size (samples per batch)
    #[arg(long, default_value_t = 256)]
    batch_size: usize,

    /// Benchmark duration in seconds
    #[arg(long, default_value_t = 120)]
    duration: u64,

    /// Number of parallel decode workers
    #[arg(long, default_value_t = 8)]
    num_workers: usize,

    /// Ring buffer depth (in-flight batches)
    #[arg(long, default_value_t = 256)]
    ring_capacity: usize,

    /// Shuffle samples (reproducible with --seed)
    #[arg(long)]
    shuffle: bool,

    /// RNG seed for reproducible shuffling
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Warmup duration in seconds before recording metrics
    #[arg(long, default_value_t = 5)]
    warmup: u64,

    /// Print progress every N seconds
    #[arg(long, default_value_t = 10)]
    report_interval: u64,

    /// WebSocket port for live dashboard metrics (optional)
    #[arg(long)]
    metrics_port: Option<u16>,

    /// Verbose debug output
    #[arg(long, short)]
    verbose: bool,

    /// Fault injection mode: none | worker_stall | disk_unplug | oom_pressure | aeron_drop
    /// Simulates real production failure scenarios mid-benchmark.
    #[arg(long, default_value = "none")]
    fault_mode: String,

    /// Seconds into benchmark when fault is injected (default: 30s)
    #[arg(long, default_value_t = 30)]
    fault_at: u64,

    /// Duration of fault in seconds (default: 10s, then auto-recover)
    #[arg(long, default_value_t = 10)]
    fault_duration: u64,
}

// ─────────────────────────────────────────────
// Fault Injection
// ─────────────────────────────────────────────

/// Active fault state shared between injector task and benchmark loop.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FaultKind {
    None,
    WorkerStall, // stall the receive loop (simulate blocked workers)
    DiskUnplug,  // inject timeout errors (simulate I/O failure)
    OomPressure, // allocate and hold memory (simulate OOM pressure)
    AeronDrop,   // drop all batches (simulate transport disconnect)
}

impl FaultKind {
    fn from_str(s: &str) -> Self {
        match s {
            "worker_stall" => Self::WorkerStall,
            "disk_unplug" => Self::DiskUnplug,
            "oom_pressure" => Self::OomPressure,
            "aeron_drop" => Self::AeronDrop,
            _ => Self::None,
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WorkerStall => "worker_stall",
            Self::DiskUnplug => "disk_unplug",
            Self::OomPressure => "oom_pressure",
            Self::AeronDrop => "aeron_drop",
        }
    }
}

struct FaultState {
    active: AtomicBool,
    kind: std::sync::atomic::AtomicU8, // encode FaultKind as u8
}

impl FaultState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            kind: std::sync::atomic::AtomicU8::new(0),
        })
    }
    fn activate(&self, kind: FaultKind) {
        self.kind.store(kind as u8, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
    }
    fn deactivate(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.kind.store(0, Ordering::SeqCst);
    }
    fn current(&self) -> FaultKind {
        if !self.active.load(Ordering::Relaxed) {
            return FaultKind::None;
        }
        match self.kind.load(Ordering::Relaxed) {
            1 => FaultKind::WorkerStall,
            2 => FaultKind::DiskUnplug,
            3 => FaultKind::OomPressure,
            4 => FaultKind::AeronDrop,
            _ => FaultKind::None,
        }
    }
}

impl FaultKind {
    #[allow(dead_code)] // reserved for WS protocol encoding
    fn as_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::WorkerStall => 1,
            Self::DiskUnplug => 2,
            Self::OomPressure => 3,
            Self::AeronDrop => 4,
        }
    }
}

/// Spawn a fault injector task that activates/deactivates the fault at the
/// configured time offsets, and broadcasts events to the dashboard.
#[cfg(feature = "metrics-ws")]
fn spawn_fault_injector(
    fault_state: Arc<FaultState>,
    fault_kind: FaultKind,
    fault_at: u64,
    fault_duration: u64,
    metrics_tx: Option<tokio::sync::broadcast::Sender<String>>,
) {
    if fault_kind == FaultKind::None {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(fault_at)).await;
        let label = fault_kind.label();
        println!("\n  ⚡ FAULT INJECT [{label}] at t+{fault_at}s — active for {fault_duration}s\n");
        if let Some(ref tx) = metrics_tx {
            tx.send(
                serde_json::json!({
                    "type": "event",
                    "t": fault_at,
                    "kind": "fault_inject",
                    "fault": label,
                    "message": format!("FAULT INJECTED: {label} (duration: {fault_duration}s)")
                })
                .to_string(),
            )
            .ok();
        }
        fault_state.activate(fault_kind);
        tokio::time::sleep(Duration::from_secs(fault_duration)).await;
        fault_state.deactivate();
        let recover_at = fault_at + fault_duration;
        println!("\n  ✓  FAULT RECOVERED [{label}] at t+{recover_at}s\n");
        if let Some(ref tx) = metrics_tx {
            tx.send(
                serde_json::json!({
                    "type": "event",
                    "t": recover_at,
                    "kind": "fault_recover",
                    "fault": label,
                    "message": format!("FAULT RECOVERED: {label}")
                })
                .to_string(),
            )
            .ok();
        }
    });
}

#[cfg(not(feature = "metrics-ws"))]
fn spawn_fault_injector(
    fault_state: Arc<FaultState>,
    fault_kind: FaultKind,
    fault_at: u64,
    fault_duration: u64,
) {
    if fault_kind == FaultKind::None {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(fault_at)).await;
        let label = fault_kind.label();
        println!("\n  ⚡ FAULT INJECT [{label}] at t+{fault_at}s — active for {fault_duration}s\n");
        fault_state.activate(fault_kind);
        tokio::time::sleep(Duration::from_secs(fault_duration)).await;
        fault_state.deactivate();
        let recover_at = fault_at + fault_duration;
        println!("\n  ✓  FAULT RECOVERED [{label}] at t+{recover_at}s\n");
    });
}

// ─────────────────────────────────────────────
// Metrics
// ─────────────────────────────────────────────

struct Metrics {
    total_samples: AtomicU64,
    total_batches: AtomicU64,
    total_errors: AtomicU64,
}

impl Metrics {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            total_samples: AtomicU64::new(0),
            total_batches: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        })
    }
}

struct InferenceMetrics {
    total_samples: AtomicU64,
    total_batches: AtomicU64,
    total_predictions: AtomicU64,
    total_errors: AtomicU64,
}

impl InferenceMetrics {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            total_samples: AtomicU64::new(0),
            total_batches: AtomicU64::new(0),
            total_predictions: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        })
    }
}

// Approximate percentile from a sorted slice.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 * pct / 100.0) as usize).min(sorted.len() - 1);
    sorted[idx]
}

// ─────────────────────────────────────────────
// Entry Point
// ─────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(if args.verbose { "debug" } else { "info" })
        .init();

    // Start WebSocket metrics server if --metrics-port provided
    #[cfg(feature = "metrics-ws")]
    let (metrics_tx, _cmd_rx, config_cache) = if let Some(port) = args.metrics_port {
        let (tx, rx, cache) = ws_server::start(port);
        // Give server time to bind
        tokio::time::sleep(Duration::from_millis(100)).await;
        (Some(tx), Some(rx), Some(cache))
    } else {
        (None, None, None)
    };

    #[cfg(not(feature = "metrics-ws"))]
    let metrics_tx: Option<tokio::sync::broadcast::Sender<String>> = None;
    #[cfg(not(feature = "metrics-ws"))]
    let config_cache: Option<Arc<tokio::sync::RwLock<Option<String>>>> = None;

    match args.mode.as_str() {
        "dataloader" => run_dataloader_benchmark(args, metrics_tx, config_cache).await,
        "inference" => run_inference_benchmark(args, metrics_tx, config_cache).await,
        other => anyhow::bail!("Unknown mode '{}'. Supported: dataloader, inference", other),
    }
}

// ─────────────────────────────────────────────
// Dataloader Benchmark Mode
// ─────────────────────────────────────────────

#[allow(unused_variables)]
async fn run_dataloader_benchmark(
    args: Args,
    #[allow(unused_variables)] metrics_tx: Option<tokio::sync::broadcast::Sender<String>>,
    #[allow(unused_variables)] config_cache: Option<Arc<tokio::sync::RwLock<Option<String>>>>,
) -> anyhow::Result<()> {
    let config = DatasetConfig::default()
        .with_batch_size(args.batch_size)
        .with_workers(args.num_workers)
        .with_ring_capacity(args.ring_capacity)
        .with_shuffle(args.shuffle)
        .with_seed(args.seed);

    // ── Load dataset ──────────────────────────
    print_header(&args);
    let t_load = Instant::now();

    let dataset = match args.dataset.as_str() {
        "imagenet" => ImageNetDataset::open(&args.path, config.clone())?,
        other => anyhow::bail!("Unknown dataset '{}'. Supported: imagenet", other),
    };

    let num_samples = dataset.len();
    let num_classes = dataset.num_classes();
    let load_ms = t_load.elapsed().as_millis();

    println!("  Samples      : {num_samples}");
    println!("  Classes      : {num_classes}");
    println!("  Index time   : {load_ms}ms");
    println!("{}", DIVIDER);

    if num_samples == 0 {
        anyhow::bail!("Dataset is empty — check path: {}", args.path);
    }

    // ── Build pipeline ────────────────────────
    let pipeline = Pipeline::new(dataset, config);

    // Broadcast config to dashboard
    #[cfg(feature = "metrics-ws")]
    if let (Some(ref tx), Some(ref cache)) = (&metrics_tx, &config_cache) {
        let config_msg = json!({
            "type": "config",
            "mode": "dataloader",
            "dataset": args.dataset,
            "batch_size": args.batch_size,
            "workers": args.num_workers,
            "duration_secs": args.duration,
            "num_samples": num_samples,
            "num_classes": num_classes,
        })
        .to_string();
        *cache.write().await = Some(config_msg.clone());
        tx.send(config_msg).ok();
    }

    // ── Fault injection setup ─────────────────
    let fault_kind = FaultKind::from_str(&args.fault_mode);
    let fault_state = FaultState::new();
    if fault_kind != FaultKind::None {
        println!(
            "  Fault mode   : {} (inject at t+{}s, duration {}s)",
            args.fault_mode, args.fault_at, args.fault_duration
        );
        println!("{}", DIVIDER);
    }

    // ── Warmup ───────────────────────────────
    if args.warmup > 0 {
        println!("  Warmup: {}s ...", args.warmup);

        #[cfg(feature = "metrics-ws")]
        if let Some(ref tx) = metrics_tx {
            tx.send(
                json!({
                    "type": "event",
                    "t": 0,
                    "kind": "warmup_start",
                    "message": format!("Warmup started ({}s)", args.warmup)
                })
                .to_string(),
            )
            .ok();
        }

        run_phase(
            &pipeline,
            Duration::from_secs(args.warmup),
            &args,
            false,
            None,
            FaultState::new(),
        )
        .await?;

        #[cfg(feature = "metrics-ws")]
        if let Some(ref tx) = metrics_tx {
            tx.send(
                json!({
                    "type": "event",
                    "t": args.warmup,
                    "kind": "warmup_done",
                    "message": "Warmup complete"
                })
                .to_string(),
            )
            .ok();
        }

        println!("  Warmup done.\n{}", DIVIDER);
    }

    // ── Benchmark ────────────────────────────
    println!("  Benchmark: {}s ...", args.duration);

    #[cfg(feature = "metrics-ws")]
    if let Some(ref tx) = metrics_tx {
        tx.send(
            json!({
                "type": "event",
                "t": 0,
                "kind": "bench_start",
                "message": format!("Benchmark started ({}s)", args.duration)
            })
            .to_string(),
        )
        .ok();
    }

    // Spawn fault injector after warmup, counting from bench start
    #[cfg(feature = "metrics-ws")]
    spawn_fault_injector(
        Arc::clone(&fault_state),
        fault_kind,
        args.fault_at,
        args.fault_duration,
        metrics_tx.clone(),
    );
    #[cfg(not(feature = "metrics-ws"))]
    spawn_fault_injector(
        Arc::clone(&fault_state),
        fault_kind,
        args.fault_at,
        args.fault_duration,
    );

    let (samples, batches, errors, latencies_us) = run_phase(
        &pipeline,
        Duration::from_secs(args.duration),
        &args,
        true,
        metrics_tx.clone(),
        Arc::clone(&fault_state),
    )
    .await?;

    // ── Report ───────────────────────────────
    print_report(
        args.duration as f64,
        samples,
        batches,
        errors,
        &latencies_us,
        args.batch_size,
    );

    // Broadcast summary to dashboard
    #[cfg(feature = "metrics-ws")]
    if let Some(ref tx) = metrics_tx {
        let mut sorted = latencies_us.clone();
        sorted.sort_unstable();
        let p50_us = percentile(&sorted, 50.0);
        let p99_us = percentile(&sorted, 99.0);
        let p999_us = percentile(&sorted, 99.9);
        let samples_per_sec = samples as f64 / args.duration as f64;

        // Calculate bandwidth (224x224 RGB)
        let bytes_per_sample = 224 * 224 * 3;
        let total_gb = (samples as f64 * bytes_per_sample as f64) / (1024.0_f64.powi(3));
        let bandwidth_gb_s = total_gb / args.duration as f64;

        tx.send(
            json!({
                "type": "summary",
                "mode": "dataloader",
                "total_samples": samples,
                "total_batches": batches,
                "total_errors": errors,
                "error_rate": if samples + errors > 0 { errors as f64 / (samples + errors) as f64 * 100.0 } else { 0.0 },
                "samples_per_sec": samples_per_sec,
                "batches_per_sec": batches as f64 / args.duration as f64,
                "bandwidth_gb_s": bandwidth_gb_s,
                "total_gb": total_gb,
                "p50_us": p50_us,
                "p99_us": p99_us,
                "p999_us": p999_us,
                "wall_secs": args.duration,
            })
            .to_string(),
        )
        .ok();

        tx.send(
            json!({
                "type": "event",
                "t": args.duration,
                "kind": "bench_done",
                "message": format!("Benchmark complete: {:.0} samples/sec", samples_per_sec)
            })
            .to_string(),
        )
        .ok();
    }

    Ok(())
}

// ─────────────────────────────────────────────
// Inference Benchmark Mode
// ─────────────────────────────────────────────

#[allow(unused_variables)]
async fn run_inference_benchmark(
    args: Args,
    #[allow(unused_variables)] metrics_tx: Option<tokio::sync::broadcast::Sender<String>>,
    #[allow(unused_variables)] config_cache: Option<Arc<tokio::sync::RwLock<Option<String>>>>,
) -> anyhow::Result<()> {
    let model_path = args
        .model
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--model required for inference mode"))?;

    let config = DatasetConfig::default()
        .with_batch_size(args.batch_size)
        .with_workers(args.num_workers)
        .with_ring_capacity(args.ring_capacity)
        .with_shuffle(args.shuffle)
        .with_seed(args.seed);

    // ── Load dataset ──────────────────────────
    print_header(&args);
    let t_load = Instant::now();

    let dataset = match args.dataset.as_str() {
        "imagenet" => ImageNetDataset::open(&args.path, config.clone())?,
        other => anyhow::bail!("Unknown dataset '{}'. Supported: imagenet", other),
    };

    let num_samples = dataset.len();
    let num_classes = dataset.num_classes();
    let dataset_load_ms = t_load.elapsed().as_millis();

    println!("  Samples      : {num_samples}");
    println!("  Classes      : {num_classes}");
    println!("  Index time   : {dataset_load_ms}ms");

    if num_samples == 0 {
        anyhow::bail!("Dataset is empty — check path: {}", args.path);
    }

    // ── Load model ────────────────────────────
    let t_model = Instant::now();
    let inference_config = InferenceConfig::new(model_path);
    let model = OnnxModel::load(inference_config)?;
    let model_load_ms = t_model.elapsed().as_millis();

    let input_shape = model.input_shape();
    let model_classes = model.num_classes();

    println!("  Model        : {}", model_path.display());
    println!("  Input shape  : {:?}", input_shape);
    println!("  Model classes: {:?}", model_classes);
    println!("  Model load   : {model_load_ms}ms");
    println!("  Inf workers  : {}", args.inference_workers);
    println!("{}", DIVIDER);

    // ── Build pipelines ───────────────────────
    let data_pipeline = Pipeline::new(dataset, config);
    let inference_pipeline = InferencePipeline::new(model, args.inference_workers);

    // Broadcast config to dashboard
    #[cfg(feature = "metrics-ws")]
    if let (Some(ref tx), Some(ref cache)) = (&metrics_tx, &config_cache) {
        let config_msg = json!({
            "type": "config",
            "mode": "inference",
            "dataset": args.dataset,
            "model": model_path.file_name().unwrap().to_string_lossy(),
            "batch_size": args.batch_size,
            "workers": args.num_workers,
            "inference_workers": args.inference_workers,
            "duration_secs": args.duration,
            "num_samples": num_samples,
            "num_classes": num_classes,
        })
        .to_string();
        *cache.write().await = Some(config_msg.clone());
        tx.send(config_msg).ok();
    }

    // ── Warmup ───────────────────────────────
    if args.warmup > 0 {
        println!("  Warmup: {}s ...", args.warmup);

        #[cfg(feature = "metrics-ws")]
        if let Some(ref tx) = metrics_tx {
            tx.send(
                json!({
                    "type": "event",
                    "t": 0,
                    "kind": "warmup_start",
                    "message": format!("Warmup started ({}s)", args.warmup)
                })
                .to_string(),
            )
            .ok();
        }

        run_inference_phase(
            &data_pipeline,
            &inference_pipeline,
            Duration::from_secs(args.warmup),
            &args,
            false,
            None,
            FaultState::new(),
        )
        .await?;

        #[cfg(feature = "metrics-ws")]
        if let Some(ref tx) = metrics_tx {
            tx.send(
                json!({
                    "type": "event",
                    "t": args.warmup,
                    "kind": "warmup_done",
                    "message": "Warmup complete"
                })
                .to_string(),
            )
            .ok();
        }

        println!("  Warmup done.\n{}", DIVIDER);
    }

    // ── Benchmark ────────────────────────────
    println!("  Benchmark: {}s ...", args.duration);

    #[cfg(feature = "metrics-ws")]
    if let Some(ref tx) = metrics_tx {
        tx.send(
            json!({
                "type": "event",
                "t": 0,
                "kind": "bench_start",
                "message": format!("Benchmark started ({}s)", args.duration)
            })
            .to_string(),
        )
        .ok();
    }

    // ── Fault injection setup ─────────────────
    let fault_kind = FaultKind::from_str(&args.fault_mode);
    let fault_state = FaultState::new();
    if fault_kind != FaultKind::None {
        println!(
            "  Fault mode   : {} (inject at t+{}s, duration {}s)",
            args.fault_mode, args.fault_at, args.fault_duration
        );
    }

    #[cfg(feature = "metrics-ws")]
    spawn_fault_injector(
        Arc::clone(&fault_state),
        fault_kind,
        args.fault_at,
        args.fault_duration,
        metrics_tx.clone(),
    );
    #[cfg(not(feature = "metrics-ws"))]
    spawn_fault_injector(
        Arc::clone(&fault_state),
        fault_kind,
        args.fault_at,
        args.fault_duration,
    );

    let (samples, batches, predictions, errors, latencies_us) = run_inference_phase(
        &data_pipeline,
        &inference_pipeline,
        Duration::from_secs(args.duration),
        &args,
        true,
        metrics_tx.clone(),
        Arc::clone(&fault_state),
    )
    .await?;

    // ── Report ───────────────────────────────
    print_inference_report(
        args.duration as f64,
        samples,
        batches,
        predictions,
        errors,
        &latencies_us,
        args.batch_size,
    );

    // Broadcast summary to dashboard
    #[cfg(feature = "metrics-ws")]
    if let Some(ref tx) = metrics_tx {
        let mut sorted = latencies_us.clone();
        sorted.sort_unstable();
        let p50_us = percentile(&sorted, 50.0);
        let p99_us = percentile(&sorted, 99.0);
        let p999_us = percentile(&sorted, 99.9);
        let rps = predictions as f64 / args.duration as f64;

        // Calculate input bandwidth (224x224 RGB)
        let bytes_per_sample = 224 * 224 * 3;
        let total_gb = (samples as f64 * bytes_per_sample as f64) / (1024.0_f64.powi(3));
        let bandwidth_gb_s = total_gb / args.duration as f64;

        tx.send(
            json!({
                "type": "summary",
                "mode": "inference",
                "total_samples": samples,
                "total_batches": batches,
                "total_predictions": predictions,
                "total_errors": errors,
                "error_rate": if samples + errors > 0 { errors as f64 / (samples + errors) as f64 * 100.0 } else { 0.0 },
                "rps": rps,
                "samples_per_sec": samples as f64 / args.duration as f64,
                "batches_per_sec": batches as f64 / args.duration as f64,
                "bandwidth_gb_s": bandwidth_gb_s,
                "total_gb": total_gb,
                "p50_us": p50_us,
                "p99_us": p99_us,
                "p999_us": p999_us,
                "wall_secs": args.duration,
            })
            .to_string(),
        )
        .ok();

        tx.send(
            json!({
                "type": "event",
                "t": args.duration,
                "kind": "bench_done",
                "message": format!("Benchmark complete: {:.0} RPS", rps)
            })
            .to_string(),
        )
        .ok();
    }

    Ok(())
}

// ─────────────────────────────────────────────
// Benchmark phase runner
// ─────────────────────────────────────────────

#[allow(unused_variables)]
async fn run_phase(
    pipeline: &Pipeline<ImageNetDataset>,
    duration: Duration,
    args: &Args,
    record: bool,
    #[allow(unused_variables)] metrics_tx: Option<tokio::sync::broadcast::Sender<String>>,
    fault_state: Arc<FaultState>,
) -> anyhow::Result<(u64, u64, u64, Vec<u64>)> {
    let metrics = Metrics::new();
    let mut latencies_us: Vec<u64> = Vec::new();
    let stop = Arc::new(AtomicBool::new(false));
    let bench_start = Instant::now();

    // Terminal progress reporting task (every report_interval seconds)
    if record && args.report_interval > 0 {
        let m = Arc::clone(&metrics);
        let stop_flag = Arc::clone(&stop);
        let interval = args.report_interval;
        tokio::spawn(async move {
            let mut last_samples = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_secs(interval));
            ticker.tick().await; // skip first tick
            loop {
                ticker.tick().await;
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                let current = m.total_samples.load(Ordering::Relaxed);
                let delta = current - last_samples;
                let rate = delta / interval;
                let batches = m.total_batches.load(Ordering::Relaxed);
                let errors = m.total_errors.load(Ordering::Relaxed);
                println!(
                    "  [+{interval}s] samples={current:<10} batches={batches:<8} \
                     rate={rate:<8}/s errors={errors}"
                );
                last_samples = current;
            }
        });
    }

    // WebSocket tick task — always 1s for smooth dashboard chart
    #[cfg(feature = "metrics-ws")]
    if record {
        if let Some(ref tx) = metrics_tx {
            let m = Arc::clone(&metrics);
            let stop_flag = Arc::clone(&stop);
            let ws_tx = tx.clone();
            let bench_start_ws = bench_start;
            tokio::spawn(async move {
                let mut last_samples = 0u64;
                let mut ticker = tokio::time::interval(Duration::from_secs(1));
                ticker.tick().await; // skip first tick
                loop {
                    ticker.tick().await;
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let current = m.total_samples.load(Ordering::Relaxed);
                    let delta = current - last_samples;
                    let batches = m.total_batches.load(Ordering::Relaxed);
                    let errors = m.total_errors.load(Ordering::Relaxed);
                    let elapsed = bench_start_ws.elapsed().as_secs();
                    ws_tx
                        .send(
                            json!({
                                "type": "tick",
                                "t": elapsed,
                                "mode": "dataloader",
                                "samples_per_sec": delta,
                                "total_samples": current,
                                "total_batches": batches,
                                "total_errors": errors,
                            })
                            .to_string(),
                        )
                        .ok();
                    last_samples = current;
                }
            });
        }
    }

    let deadline = Instant::now() + duration;
    let mut rx = if args.shuffle {
        pipeline.stream_shuffled(args.seed)
    } else {
        pipeline.stream()
    };

    while Instant::now() < deadline {
        let batch_start = Instant::now();

        // Apply fault behavior
        match fault_state.current() {
            FaultKind::WorkerStall => {
                // Stall the loop — simulate blocked worker threads
                tokio::time::sleep(Duration::from_millis(200)).await;
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            FaultKind::DiskUnplug => {
                // Simulate I/O error — count as error, skip batch
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            FaultKind::AeronDrop => {
                // Drop all batches — transport blackhole
                let _ = timeout(Duration::from_millis(100), rx.recv()).await;
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            FaultKind::OomPressure => {
                // Simulate OOM by allocating a 128MB chunk then releasing
                let _pressure: Vec<u8> = vec![0u8; 128 * 1024 * 1024];
                // fall through to normal processing
            }
            FaultKind::None => {}
        }

        match timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(Ok(batch))) => {
                let n = batch.samples.len() as u64;
                if record {
                    metrics.total_samples.fetch_add(n, Ordering::Relaxed);
                    metrics.total_batches.fetch_add(1, Ordering::Relaxed);
                    latencies_us.push(batch_start.elapsed().as_micros() as u64);
                }
            }
            Ok(Some(Err(e))) => {
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::warn!(error = %e, "batch error");
            }
            Ok(None) => {
                // Dataset exhausted — loop (simulate multi-epoch training).
                rx = if args.shuffle {
                    pipeline.stream_shuffled(args.seed)
                } else {
                    pipeline.stream()
                };
            }
            Err(_) => {
                // Timeout — pipeline starved (too few workers or slow disk).
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::debug!("receive timeout — pipeline stalled");
            }
        }
    }

    stop.store(true, Ordering::Relaxed);

    let s = metrics.total_samples.load(Ordering::Relaxed);
    let b = metrics.total_batches.load(Ordering::Relaxed);
    let e = metrics.total_errors.load(Ordering::Relaxed);
    Ok((s, b, e, latencies_us))
}

#[allow(unused_variables)]
async fn run_inference_phase(
    data_pipeline: &Pipeline<ImageNetDataset>,
    inference_pipeline: &InferencePipeline<OnnxModel>,
    duration: Duration,
    args: &Args,
    record: bool,
    #[allow(unused_variables)] metrics_tx: Option<tokio::sync::broadcast::Sender<String>>,
    fault_state: Arc<FaultState>,
) -> anyhow::Result<(u64, u64, u64, u64, Vec<u64>)> {
    let metrics = InferenceMetrics::new();
    let mut latencies_us: Vec<u64> = Vec::new();
    let stop = Arc::new(AtomicBool::new(false));
    let bench_start = Instant::now();

    // Terminal progress reporting task (every report_interval seconds)
    if record && args.report_interval > 0 {
        let m = Arc::clone(&metrics);
        let stop_flag = Arc::clone(&stop);
        let interval = args.report_interval;
        tokio::spawn(async move {
            let mut last_samples = 0u64;
            let mut last_predictions = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_secs(interval));
            ticker.tick().await; // skip first tick
            loop {
                ticker.tick().await;
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                let current_samples = m.total_samples.load(Ordering::Relaxed);
                let current_predictions = m.total_predictions.load(Ordering::Relaxed);
                let delta_samples = current_samples - last_samples;
                let delta_predictions = current_predictions - last_predictions;
                let rate_samples = delta_samples / interval;
                let rate_predictions = delta_predictions / interval;
                let batches = m.total_batches.load(Ordering::Relaxed);
                let errors = m.total_errors.load(Ordering::Relaxed);
                println!(
                    "  [+{interval}s] samples={current_samples:<10} predictions={current_predictions:<10} \
                     batches={batches:<8} rate={rate_samples:<8}/s pred_rate={rate_predictions:<8}/s errors={errors}"
                );
                last_samples = current_samples;
                last_predictions = current_predictions;
            }
        });
    }

    // WebSocket tick task — always 1s for smooth dashboard chart
    #[cfg(feature = "metrics-ws")]
    if record {
        if let Some(ref tx) = metrics_tx {
            let m = Arc::clone(&metrics);
            let stop_flag = Arc::clone(&stop);
            let ws_tx = tx.clone();
            let bench_start_ws = bench_start;
            tokio::spawn(async move {
                let mut last_samples = 0u64;
                let mut last_predictions = 0u64;
                let mut ticker = tokio::time::interval(Duration::from_secs(1));
                ticker.tick().await; // skip first tick
                loop {
                    ticker.tick().await;
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let current_samples = m.total_samples.load(Ordering::Relaxed);
                    let current_predictions = m.total_predictions.load(Ordering::Relaxed);
                    let delta_samples = current_samples - last_samples;
                    let delta_predictions = current_predictions - last_predictions;
                    let batches = m.total_batches.load(Ordering::Relaxed);
                    let errors = m.total_errors.load(Ordering::Relaxed);
                    let elapsed = bench_start_ws.elapsed().as_secs();
                    ws_tx
                        .send(
                            json!({
                                "type": "tick",
                                "t": elapsed,
                                "mode": "inference",
                                "rps": delta_predictions,
                                "samples_per_sec": delta_samples,
                                "total_samples": current_samples,
                                "total_predictions": current_predictions,
                                "total_batches": batches,
                                "total_errors": errors,
                            })
                            .to_string(),
                        )
                        .ok();
                    last_samples = current_samples;
                    last_predictions = current_predictions;
                }
            });
        }
    }

    let deadline = Instant::now() + duration;
    let mut data_rx = if args.shuffle {
        data_pipeline.stream_shuffled(args.seed)
    } else {
        data_pipeline.stream()
    };

    // Start inference pipeline
    let mut inference_rx = inference_pipeline.stream(data_rx).await?;

    while Instant::now() < deadline {
        let batch_start = Instant::now();

        // Apply fault behavior
        match fault_state.current() {
            FaultKind::WorkerStall => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            FaultKind::DiskUnplug | FaultKind::AeronDrop => {
                let _ = timeout(Duration::from_millis(100), inference_rx.recv()).await;
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            FaultKind::OomPressure => {
                let _pressure: Vec<u8> = vec![0u8; 128 * 1024 * 1024];
            }
            FaultKind::None => {}
        }

        match timeout(Duration::from_millis(500), inference_rx.recv()).await {
            Ok(Some(Ok(inference_batch))) => {
                let n_predictions = inference_batch.predictions.len() as u64;
                if record {
                    metrics
                        .total_samples
                        .fetch_add(n_predictions, Ordering::Relaxed);
                    metrics
                        .total_predictions
                        .fetch_add(n_predictions, Ordering::Relaxed);
                    metrics.total_batches.fetch_add(1, Ordering::Relaxed);
                    latencies_us.push(batch_start.elapsed().as_micros() as u64);
                }
            }
            Ok(Some(Err(e))) => {
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::warn!(error = %e, "inference error");
            }
            Ok(None) => {
                // Pipeline exhausted — restart
                data_rx = if args.shuffle {
                    data_pipeline.stream_shuffled(args.seed)
                } else {
                    data_pipeline.stream()
                };
                inference_rx = inference_pipeline.stream(data_rx).await?;
            }
            Err(_) => {
                // Timeout — pipeline stalled
                if record {
                    metrics.total_errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::debug!("receive timeout — pipeline stalled");
            }
        }
    }

    stop.store(true, Ordering::Relaxed);

    let s = metrics.total_samples.load(Ordering::Relaxed);
    let b = metrics.total_batches.load(Ordering::Relaxed);
    let p = metrics.total_predictions.load(Ordering::Relaxed);
    let e = metrics.total_errors.load(Ordering::Relaxed);
    Ok((s, b, p, e, latencies_us))
}

// ─────────────────────────────────────────────
// Output formatting
// ─────────────────────────────────────────────

const DIVIDER: &str = "  ────────────────────────────────────────────────────────";
const DOUBLE: &str = "  ════════════════════════════════════════════════════════";

fn print_header(args: &Args) {
    println!();
    println!("{DOUBLE}");
    match args.mode.as_str() {
        "dataloader" => println!("  Blazil Dataloader Benchmark  v0.1.0"),
        "inference" => println!("  Blazil Inference Benchmark  v0.1.0"),
        _ => println!("  Blazil ML Benchmark  v0.1.0"),
    }
    println!("{DOUBLE}");
    println!("  Mode         : {}", args.mode);
    println!("  Dataset      : {}", args.dataset);
    println!("  Path         : {}", args.path);
    println!("  Batch size   : {}", args.batch_size);
    println!("  Workers      : {}", args.num_workers);
    println!("  Ring depth   : {}", args.ring_capacity);
    println!("  Shuffle      : {}", args.shuffle);
    println!("  Duration     : {}s", args.duration);
    println!("  Warmup       : {}s", args.warmup);
    println!("{DIVIDER}");
}

fn print_report(
    elapsed_s: f64,
    samples: u64,
    batches: u64,
    errors: u64,
    latencies_us: &[u64],
    _batch_size: usize,
) {
    let avg_samples_sec = samples as f64 / elapsed_s;
    let avg_batches_sec = batches as f64 / elapsed_s;
    let total_requests = samples + errors;
    let error_rate = if total_requests > 0 {
        errors as f64 / total_requests as f64 * 100.0
    } else {
        0.0
    };

    let mut sorted = latencies_us.to_vec();
    sorted.sort_unstable();
    let p50_ms = percentile(&sorted, 50.0) as f64 / 1000.0;
    let p99_ms = percentile(&sorted, 99.0) as f64 / 1000.0;
    let p999_ms = percentile(&sorted, 99.9) as f64 / 1000.0;

    println!();
    println!("{DOUBLE}");
    println!("  RESULTS");
    println!("{DOUBLE}");
    println!("  Duration         : {elapsed_s:.1}s");
    println!("  Total samples    : {samples}");
    println!("  Total batches    : {batches}");
    println!("  Errors           : {errors}  ({error_rate:.4}%)");
    println!("{DIVIDER}");
    println!("  Throughput");
    println!("    Avg            : {:.0} samples/sec", avg_samples_sec);
    println!("    Batches/sec    : {:.0} batches/sec", avg_batches_sec);
    println!("{DIVIDER}");
    println!("  Batch latency (decode + queue)");
    println!("    P50            : {p50_ms:.2}ms");
    println!("    P99            : {p99_ms:.2}ms");
    println!("    P999           : {p999_ms:.2}ms");
    println!("{DOUBLE}");

    let status = if error_rate <= 0.0 {
        "  ✓  Error rate: 0.00%  (target met)"
    } else if error_rate < 0.01 {
        "  ⚠  Error rate < 0.01%"
    } else {
        "  ✗  Error rate exceeds 0.01% threshold"
    };
    println!("{status}");

    let throughput_target = 10_000_000.0;
    if avg_samples_sec >= throughput_target {
        println!(
            "  ✓  Throughput: {:.1}M samples/sec  (target: 10M+)",
            avg_samples_sec / 1_000_000.0
        );
    } else {
        println!(
            "  ↑  Throughput: {:.1}M samples/sec  (target: 10M+ — add workers or faster disk)",
            avg_samples_sec / 1_000_000.0
        );
    }

    // Data volume
    let bytes_per_sample = 224 * 224 * 3; // 224×224 RGB
    let total_gb = (samples as f64 * bytes_per_sample as f64) / (1024.0_f64.powi(3));
    let bandwidth_gb_s = total_gb / elapsed_s;
    println!("  Bandwidth        : {bandwidth_gb_s:.2} GB/s  ({total_gb:.1} GB total)");

    println!("{DOUBLE}");
    println!();
}

fn print_inference_report(
    elapsed_s: f64,
    samples: u64,
    batches: u64,
    predictions: u64,
    errors: u64,
    latencies_us: &[u64],
    _batch_size: usize,
) {
    let avg_samples_sec = samples as f64 / elapsed_s;
    let avg_predictions_sec = predictions as f64 / elapsed_s;
    let avg_batches_sec = batches as f64 / elapsed_s;
    let total_requests = samples + errors;
    let error_rate = if total_requests > 0 {
        errors as f64 / total_requests as f64 * 100.0
    } else {
        0.0
    };

    let mut sorted = latencies_us.to_vec();
    sorted.sort_unstable();
    let p50_ms = percentile(&sorted, 50.0) as f64 / 1000.0;
    let p99_ms = percentile(&sorted, 99.0) as f64 / 1000.0;
    let p999_ms = percentile(&sorted, 99.9) as f64 / 1000.0;

    println!();
    println!("{DOUBLE}");
    println!("  INFERENCE RESULTS");
    println!("{DOUBLE}");
    println!("  Duration         : {elapsed_s:.1}s");
    println!("  Total samples    : {samples}");
    println!("  Total predictions: {predictions}");
    println!("  Total batches    : {batches}");
    println!("  Errors           : {errors}  ({error_rate:.4}%)");
    println!("{DIVIDER}");
    println!("  Throughput");
    println!("    Samples/sec    : {:.0} samples/sec", avg_samples_sec);
    println!(
        "    Predictions/sec: {:.0} predictions/sec",
        avg_predictions_sec
    );
    println!("    Batches/sec    : {:.0} batches/sec", avg_batches_sec);
    println!("{DIVIDER}");
    println!("  End-to-End Latency (dataloader + inference)");
    println!("    P50            : {p50_ms:.2}ms");
    println!("    P99            : {p99_ms:.2}ms");
    println!("    P999           : {p999_ms:.2}ms");
    println!("{DOUBLE}");

    let status = if error_rate <= 0.0 {
        "  ✓  Error rate: 0.00%  (target met)"
    } else if error_rate < 0.01 {
        "  ⚠  Error rate < 0.01%"
    } else {
        "  ✗  Error rate exceeds 0.01% threshold"
    };
    println!("{status}");

    // Check latency target: <10ms p99
    let latency_target_ms = 10.0;
    if p99_ms < latency_target_ms {
        println!("  ✓  P99 latency: {p99_ms:.2}ms  (target: <{latency_target_ms}ms)");
    } else {
        println!(
            "  ↑  P99 latency: {p99_ms:.2}ms  (target: <{latency_target_ms}ms — optimize model or add GPU)"
        );
    }

    // Data volume
    let bytes_per_sample = 224 * 224 * 3; // 224×224 RGB
    let total_gb = (samples as f64 * bytes_per_sample as f64) / (1024.0_f64.powi(3));
    let bandwidth_gb_s = total_gb / elapsed_s;
    println!("  Bandwidth        : {bandwidth_gb_s:.2} GB/s  ({total_gb:.1} GB total)");

    println!("{DOUBLE}");
    println!();
}
