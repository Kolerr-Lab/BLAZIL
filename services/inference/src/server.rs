//! Aeron IPC inference server implementing [`TransportServer`] trait.
//!
//! ## Architecture
//!
//! ```text
//! AeronInferenceServer::serve()
//!    │  tokio::task::spawn_blocking
//!    ▼
//! aeron_inference_loop()                      (dedicated OS thread)
//!    │  EmbeddedAeronDriver::start()          ← in-process C driver
//!    │  AeronContext::new(aeron_dir)           ← Aeron client
//!    │  AeronSubscription::new(ch, 2001)       ← inbound requests
//!    │  AeronPublication::new(ch, 2002)        → outbound responses
//!    │
//!    ▼  poll loop
//! subscription.poll_fragments()
//!    │  for each raw fragment:
//!    │    deserialize MessagePack → InferenceRequest
//!    │    convert to Sample → run_inference()
//!    │    measure latency
//!    │    serialize MessagePack → InferenceResponse
//!    ▼
//! publication.offer(response_bytes)
//! ```
//!
//! ## Drop Safety
//!
//! 1. `AeronPublication` + `AeronSubscription` (close streams)
//! 2. `AeronContext` (`aeron_close`)
//! 3. `EmbeddedAeronDriver` (driver exits, `aeron_driver_close`)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_dataloader::Sample;
use blazil_inference::{InferenceModel, OnnxModel};
use blazil_transport::aeron::{
    AeronContext, AeronPublication, AeronSubscription, EmbeddedAeronDriver,
};
use blazil_transport::server::TransportServer;

use crate::protocol::{
    deserialize_request, serialize_response, InferenceRequest, InferenceResponse,
    INFERENCE_REQ_STREAM_ID, INFERENCE_RSP_STREAM_ID,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default Aeron IPC channel for inference.
///
/// `term-length=67108864` = 64 MB log buffer (prevents backpressure).
pub const DEFAULT_INFERENCE_CHANNEL: &str = "aeron:ipc?term-length=67108864";

/// Timeout waiting for publication/subscription registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum fragments processed per poll.
const FRAGMENT_LIMIT: usize = 256;

/// Max spin retries on Aeron offer backpressure before yielding.
const OFFER_SPIN_RETRIES: usize = 64;

/// After this many empty polls, yield to other threads.
const SPIN_BEFORE_YIELD: u32 = 512;

// ── AeronInferenceServer ──────────────────────────────────────────────────────

/// Aeron IPC inference server using embedded C Media Driver.
///
/// Subscribes to [`INFERENCE_REQ_STREAM_ID`] on the configured channel,
/// processes each request through the ONNX model, and publishes responses
/// on [`INFERENCE_RSP_STREAM_ID`].
pub struct AeronInferenceServer {
    /// Aeron channel URI.
    channel: String,
    /// Path to Aeron IPC shared-memory directory.
    aeron_dir: String,
    /// Loaded ONNX model (shared across threads).
    model: Arc<OnnxModel>,
    /// Shutdown signal.
    shutdown: Arc<AtomicBool>,
    /// Cumulative Aeron publication offer() failures.
    offer_failures: Arc<AtomicU64>,
    /// Total requests processed.
    requests_processed: Arc<AtomicU64>,
}

impl AeronInferenceServer {
    /// Creates a new `AeronInferenceServer`.
    ///
    /// - `channel`   — Aeron channel URI (see [`DEFAULT_INFERENCE_CHANNEL`]).
    /// - `aeron_dir` — IPC directory for the embedded C Media Driver.
    /// - `model`     — Pre-loaded ONNX model.
    pub fn new(channel: &str, aeron_dir: &str, model: Arc<OnnxModel>) -> Self {
        Self {
            channel: channel.to_owned(),
            aeron_dir: aeron_dir.to_owned(),
            model,
            shutdown: Arc::new(AtomicBool::new(false)),
            offer_failures: Arc::new(AtomicU64::new(0)),
            requests_processed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Cumulative Aeron offer() failure count.
    #[allow(dead_code)]
    pub fn offer_failures(&self) -> &Arc<AtomicU64> {
        &self.offer_failures
    }

    /// Total requests processed since start.
    #[allow(dead_code)]
    pub fn requests_processed(&self) -> &Arc<AtomicU64> {
        &self.requests_processed
    }
}

#[async_trait]
impl TransportServer for AeronInferenceServer {
    /// Start the Aeron IPC inference server.
    ///
    /// Runs the Aeron poll loop inside [`tokio::task::spawn_blocking`] to avoid
    /// blocking the tokio runtime.
    async fn serve(&self) -> BlazerResult<()> {
        let channel = self.channel.clone();
        let aeron_dir = self.aeron_dir.clone();
        let model = Arc::clone(&self.model);
        let shutdown = Arc::clone(&self.shutdown);
        let offer_failures = Arc::clone(&self.offer_failures);
        let requests_processed = Arc::clone(&self.requests_processed);

        info!(
            channel = %channel,
            aeron_dir = %aeron_dir,
            "Starting Aeron inference server"
        );

        let result = tokio::task::spawn_blocking(move || {
            aeron_inference_loop(
                &channel,
                &aeron_dir,
                model,
                shutdown,
                offer_failures,
                requests_processed,
            )
        })
        .await;

        match result {
            Ok(Ok(())) => {
                info!("Aeron inference server shut down cleanly");
                Ok(())
            }
            Ok(Err(e)) => {
                error!("Aeron inference loop error: {e}");
                Err(e)
            }
            Err(e) => {
                error!("Tokio spawn_blocking panicked: {e}");
                Err(BlazerError::Internal(format!("spawn_blocking panic: {e}")))
            }
        }
    }

    async fn shutdown(&self) {
        info!("Shutting down Aeron inference server");
        self.shutdown.store(true, Ordering::SeqCst);
    }

    fn local_addr(&self) -> &str {
        &self.channel
    }
}

// ── Aeron Poll Loop (Blocking Thread) ────────────────────────────────────────

fn aeron_inference_loop(
    channel: &str,
    aeron_dir: &str,
    model: Arc<OnnxModel>,
    shutdown: Arc<AtomicBool>,
    offer_failures: Arc<AtomicU64>,
    requests_processed: Arc<AtomicU64>,
) -> BlazerResult<()> {
    // 1. Start embedded Aeron C driver
    let driver = EmbeddedAeronDriver::new(Some(aeron_dir));
    driver
        .start()
        .map_err(|e| BlazerError::Transport(format!("start Aeron driver: {e}")))?;

    info!("Embedded Aeron driver started");

    // 2. Create Aeron context
    let ctx = AeronContext::new(aeron_dir)
        .map_err(|e| BlazerError::Transport(format!("create Aeron context: {e}")))?;

    info!("Aeron context created");

    // 3. Create subscription (inbound requests)
    let sub = AeronSubscription::new(&ctx, channel, INFERENCE_REQ_STREAM_ID, REGISTRATION_TIMEOUT)
        .map_err(|e| BlazerError::Transport(format!("create subscription: {e}")))?;

    info!("Subscription registered");

    // 4. Create publication (outbound responses)
    let pub_ = AeronPublication::new(&ctx, channel, INFERENCE_RSP_STREAM_ID, REGISTRATION_TIMEOUT)
        .map_err(|e| BlazerError::Transport(format!("create publication: {e}")))?;

    info!("Publication registered");

    // 5. Poll loop
    info!("Aeron inference server ready - entering poll loop");

    let mut empty_polls: u32 = 0;
    let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);

    while !shutdown.load(Ordering::Relaxed) {
        fragments.clear();

        let fragments_read = sub.poll_fragments(&mut fragments, FRAGMENT_LIMIT);

        if fragments_read > 0 {
            for buffer in &fragments {
                // Deserialize request
                let req = match deserialize_request(buffer) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to deserialize request: {e}");
                        continue;
                    }
                };

                debug!(
                    request_id = %req.request_id,
                    shape = ?req.input_shape,
                    "Processing inference request"
                );

                let start = Instant::now();

                // Process inference
                let response = process_inference_request(&model, req);

                let latency_us = start.elapsed().as_micros() as u64;

                // Update response latency
                let mut response = response;
                response.latency_us = latency_us;

                // Serialize response
                let resp_bytes = match serialize_response(&response) {
                    Ok(b) => b,
                    Err(e) => {
                        error!("Failed to serialize response: {e}");
                        continue;
                    }
                };

                // Offer response back to client
                let mut retries = 0;
                loop {
                    match pub_.offer(&resp_bytes) {
                        Ok(_pos) => {
                            requests_processed.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                        Err(_) if retries < OFFER_SPIN_RETRIES => {
                            retries += 1;
                            std::hint::spin_loop();
                        }
                        Err(e) => {
                            warn!("Aeron offer failed after retries: {e}");
                            offer_failures.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
            empty_polls = 0;
        } else {
            empty_polls += 1;
            if empty_polls >= SPIN_BEFORE_YIELD {
                std::thread::yield_now();
                empty_polls = 0;
            }
        }
    }

    info!("Shutdown signal received - cleaning up");

    // Drop order is critical for C safety
    drop(sub);
    drop(pub_);
    drop(ctx);
    drop(driver);

    info!("Aeron inference server stopped");
    Ok(())
}

// ── Inference Processing ──────────────────────────────────────────────────────

fn process_inference_request(model: &Arc<OnnxModel>, req: InferenceRequest) -> InferenceResponse {
    // Convert request to Sample
    let sample = Sample {
        data: req.input_data.clone(),
        label: 0, // Not used for inference
        metadata: None,
    };

    // Run inference
    match model.run_batch(&[sample]) {
        Ok(predictions) => {
            if let Some(pred) = predictions.first() {
                InferenceResponse {
                    request_id: req.request_id,
                    class_id: pred.class_id,
                    probabilities: pred.probabilities.clone().unwrap_or_default(),
                    raw_output: pred.raw_output.clone(),
                    confidence: pred.confidence,
                    latency_us: 0, // Set by caller
                    error: String::new(),
                }
            } else {
                InferenceResponse {
                    request_id: req.request_id,
                    class_id: None,
                    probabilities: vec![],
                    raw_output: vec![],
                    confidence: 0.0,
                    latency_us: 0,
                    error: "Model returned no predictions".to_string(),
                }
            }
        }
        Err(e) => InferenceResponse {
            request_id: req.request_id,
            class_id: None,
            probabilities: vec![],
            raw_output: vec![],
            confidence: 0.0,
            latency_us: 0,
            error: format!("Inference failed: {e}"),
        },
    }
}
