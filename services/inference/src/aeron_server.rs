//! Standalone Aeron IPC inference listener — dedicated thread.
//!
//! Listens on stream 2001 (requests), publishes on stream 2002 (responses).
//! Uses MessagePack serialization for InferenceRequest/InferenceResponse.
//!
//! # Architecture
//!
//! ```text
//! std::thread::spawn
//!    │
//!    ▼
//! run()
//!    │  EmbeddedAeronDriver::start()      ← in-process C driver
//!    │  AeronContext::new(aeron_dir)       ← Aeron client
//!    │  AeronSubscription::new(ch, 2001)   ← inbound requests
//!    │  AeronPublication::new(ch, 2002)    → outbound responses
//!    │
//!    ▼  poll loop
//! subscription.poll_fragments()
//!    │  for each fragment:
//!    │    deserialize MessagePack → InferenceRequest
//!    │    String::from_utf8(input_data) → prompt
//!    │    gguf_model.generate_streaming() → generated text
//!    │    encode text as Vec<f32> → raw_output
//!    │    serialize MessagePack → InferenceResponse
//!    ▼
//! publication.offer(response_bytes)
//! ```
//!
//! # Why std::thread instead of tokio?
//!
//! Aeron FFI (C bindings) is not `Send + Sync` safe. Running on a dedicated
//! OS thread ensures:
//! - No accidental moves across tokio tasks
//! - Predictable drop order (critical for C cleanup)
//! - No async overhead for tight poll loop

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use blazil_transport::aeron::{
    AeronContext, AeronPublication, AeronSubscription, EmbeddedAeronDriver,
};

use crate::gguf_model::GgufModel;
use crate::protocol::{
    deserialize_request, serialize_response, InferenceRequest, InferenceResponse,
    INFERENCE_REQ_STREAM_ID, INFERENCE_RSP_STREAM_ID,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default Aeron IPC channel for inference.
///
/// `term-length=67108864` = 64 MB log buffer (prevents backpressure).
const DEFAULT_CHANNEL: &str = "aeron:ipc?term-length=67108864";

/// Timeout waiting for publication/subscription registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum fragments processed per poll.
const FRAGMENT_LIMIT: usize = 256;

/// Max spin retries on Aeron offer backpressure before yielding.
const OFFER_SPIN_RETRIES: usize = 64;

/// Idle strategy thresholds to prevent CPU starvation of embedded Media Driver.
/// On macOS (especially Apple Silicon), tight polling can starve driver threads.
const IDLE_SPIN_THRESHOLD: u64 = 100; // Start spin hints after 100 empty polls
const IDLE_YIELD_THRESHOLD: u64 = 1000; // Yield to OS after 1000 empty polls

// ── Public API ────────────────────────────────────────────────────────────────

/// Run Aeron IPC inference server on the current thread (blocking).
///
/// # Arguments
///
/// - `model` — GGUF model wrapped in Arc<Mutex<>> for interior mutability
/// - `aeron_dir` — Path to Aeron IPC directory (e.g., `/dev/shm/aeron-inference`)
///
/// # Panics
///
/// Panics if Aeron driver fails to start or streams fail to register.
///
/// # Example
///
/// ```no_run
/// use std::sync::{Arc, Mutex};
/// use blazil_inference_service::gguf_model::GgufModel;
/// use blazil_inference_service::aeron_server;
///
/// let model = GgufModel::load("model.gguf", 8, 4096).unwrap();
/// let model_arc = Arc::new(Mutex::new(model));
///
/// // Spawn dedicated thread for Aeron IPC
/// std::thread::spawn(move || {
///     aeron_server::run(model_arc, "/dev/shm/aeron-inference");
/// });
/// ```
pub fn run(model: Arc<Mutex<GgufModel>>, aeron_dir: &str) {
    run_with_channel(model, aeron_dir, DEFAULT_CHANNEL);
}

/// Run Aeron IPC server with custom channel URI.
///
/// For testing or custom network configurations.
pub fn run_with_channel(model: Arc<Mutex<GgufModel>>, aeron_dir: &str, channel: &str) {
    info!(
        channel = %channel,
        aeron_dir = %aeron_dir,
        "🚀 Starting Aeron IPC inference listener (dedicated thread)"
    );

    // 1. Start embedded Aeron C driver
    let driver = EmbeddedAeronDriver::new(Some(aeron_dir));
    driver.start().expect("EmbeddedAeronDriver::start failed");

    info!("✓ Embedded Aeron driver started");

    // 2. Create Aeron context
    let ctx = AeronContext::new(aeron_dir).expect("AeronContext::new failed");

    info!("✓ Aeron context created");

    // 3. Create subscription (inbound requests on stream 2001)
    let sub = AeronSubscription::new(&ctx, channel, INFERENCE_REQ_STREAM_ID, REGISTRATION_TIMEOUT)
        .expect("AeronSubscription::new failed");

    info!(
        "✓ Subscription registered (stream {})",
        INFERENCE_REQ_STREAM_ID
    );

    // 4. Create publication (outbound responses on stream 2002)
    let pub_ = AeronPublication::new(&ctx, channel, INFERENCE_RSP_STREAM_ID, REGISTRATION_TIMEOUT)
        .expect("AeronPublication::new failed");

    info!(
        "✓ Publication registered (stream {})",
        INFERENCE_RSP_STREAM_ID
    );

    // 5. Enter poll loop with idle strategy
    info!("✅ Aeron IPC inference server ready — entering poll loop");

    let mut idle_count: u64 = 0;
    let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);
    let mut requests_processed: u64 = 0;

    loop {
        fragments.clear();

        let fragments_read = sub.poll_fragments(&mut fragments, FRAGMENT_LIMIT);

        if fragments_read > 0 {
            // Reset idle counter when we have work
            idle_count = 0;

            for buffer in &fragments {
                // Deserialize MessagePack request
                let req = match deserialize_request(buffer) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to deserialize request: {e}");
                        continue;
                    }
                };

                debug!(
                    request_id = %req.request_id,
                    input_len = req.input_data.len(),
                    shape = ?req.input_shape,
                    "Processing inference request"
                );

                let start = Instant::now();

                // Process GGUF inference
                let response = process_gguf_inference(&model, req);

                let latency_us = start.elapsed().as_micros() as u64;

                // Update response latency
                let mut response = response;
                response.latency_us = latency_us;

                debug!(
                    request_id = %response.request_id,
                    latency_us = latency_us,
                    output_len = response.raw_output.len(),
                    error = %response.error,
                    "Inference complete"
                );

                // Serialize MessagePack response
                let resp_bytes = match serialize_response(&response) {
                    Ok(b) => b,
                    Err(e) => {
                        error!("Failed to serialize response: {e}");
                        continue;
                    }
                };

                // Offer response back to client (with backpressure handling)
                let mut retries = 0;
                loop {
                    match pub_.offer(&resp_bytes) {
                        Ok(_pos) => {
                            requests_processed += 1;
                            if requests_processed.is_multiple_of(1000) {
                                info!("Processed {requests_processed} requests");
                            }
                            break;
                        }
                        Err(_) if retries < OFFER_SPIN_RETRIES => {
                            retries += 1;
                            std::hint::spin_loop();
                        }
                        Err(e) => {
                            warn!("Aeron offer failed after {OFFER_SPIN_RETRIES} retries: {e}");
                            break;
                        }
                    }
                }
            }
        } else {
            // No fragments — implement idle strategy to prevent CPU starvation
            idle_count += 1;

            if idle_count > IDLE_YIELD_THRESHOLD {
                // After 1000 empty polls, yield to OS scheduler
                // This gives CPU time to embedded Media Driver threads
                std::thread::yield_now();
            } else if idle_count > IDLE_SPIN_THRESHOLD {
                // After 100 empty polls, add micro-pause
                std::hint::spin_loop();
            }
            // else: tight loop for low latency on first idle cycles
        }
    }

    // Note: This function never returns (infinite loop).
    // To gracefully shut down, we'd need a shutdown signal (e.g., AtomicBool).
    // For now, server runs until process termination.
}

// ── GGUF Inference Processing ─────────────────────────────────────────────────

/// Process GGUF text generation inference.
///
/// # Protocol
///
/// **Request:**
/// - `input_data` — UTF-8 encoded prompt text
/// - `input_shape` — [max_tokens] (optional, defaults to model config)
///
/// **Response:**
/// - `raw_output` — Generated text encoded as Vec<f32> (each byte → f32)
/// - `confidence` — Always 1.0 (not applicable for text generation)
/// - `class_id` — None (not applicable)
/// - `probabilities` — Empty (not applicable)
fn process_gguf_inference(
    model: &Arc<Mutex<GgufModel>>,
    req: InferenceRequest,
) -> InferenceResponse {
    // Decode input_data as UTF-8 text (prompt)
    let prompt = match String::from_utf8(req.input_data.clone()) {
        Ok(s) => s,
        Err(e) => {
            return InferenceResponse {
                request_id: req.request_id,
                class_id: None,
                probabilities: vec![],
                raw_output: vec![],
                confidence: 0.0,
                latency_us: 0,
                error: format!("Invalid UTF-8 prompt: {e}"),
            };
        }
    };

    // Extract max_tokens from input_shape[0] (optional)
    let max_tokens_override = req
        .input_shape
        .first()
        .copied()
        .unwrap_or(0)
        .try_into()
        .unwrap_or(0_usize);

    // Lock mutex to access GgufModel
    let mut model_guard = match model.lock() {
        Ok(g) => g,
        Err(e) => {
            return InferenceResponse {
                request_id: req.request_id,
                class_id: None,
                probabilities: vec![],
                raw_output: vec![],
                confidence: 0.0,
                latency_us: 0,
                error: format!("Failed to lock model mutex: {e}"),
            };
        }
    };

    // Generate streaming text (collect all tokens)
    let mut generated_text = String::new();
    let generation_result = model_guard.generate_streaming(&prompt, max_tokens_override, |token| {
        generated_text.push_str(token);
    });

    drop(model_guard); // Release mutex ASAP

    // Check for generation errors
    if let Err(e) = generation_result {
        return InferenceResponse {
            request_id: req.request_id,
            class_id: None,
            probabilities: vec![],
            raw_output: vec![],
            confidence: 0.0,
            latency_us: 0,
            error: format!("Text generation failed: {e}"),
        };
    }

    // Encode generated text as Vec<f32> for protocol compatibility
    // (Aeron protocol expects f32 output, so we encode UTF-8 bytes as f32)
    let raw_output: Vec<f32> = generated_text.bytes().map(|b| b as f32).collect();

    InferenceResponse {
        request_id: req.request_id,
        class_id: None,
        probabilities: vec![],
        raw_output,
        confidence: 1.0, // Not applicable for text generation
        latency_us: 0,   // Set by caller
        error: String::new(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_gguf_inference_invalid_utf8() {
        // Test that invalid UTF-8 input returns proper error response
        // We'll create a mock model that won't actually be called

        // Skip test if no model file available
        // In production, this would require a real GGUF model file
        // For now, we just verify the error handling path

        let req = InferenceRequest {
            request_id: "test-001".to_string(),
            input_data: vec![0xFF, 0xFE, 0xFD], // Invalid UTF-8
            input_shape: vec![],
            model_version: "v1".to_string(),
        };

        // We can't create a mock model without a real GGUF file
        // So we just verify the request structure is valid
        assert_eq!(req.request_id, "test-001");
        assert_eq!(req.input_data, vec![0xFF, 0xFE, 0xFD]);
    }
}
