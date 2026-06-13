//! Aeron IPC test client for Blazil Inference Server.
//!
//! Tests the Hybrid Matrix quantization pipeline by sending text prompts
//! and receiving streaming token responses via Aeron IPC.
//!
//! # Protocol
//! ```text
//! Client → Stream 2001 (InferenceRequest) → Server
//!   → Hybrid Matrix inference (INT8 → BitNet → INT8)
//!   → Stream 2002 (InferenceResponse) → Client
//! ```
//!
//! # Usage
//! ```bash
//! cargo run --release --bin test-inference
//! ```

use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{error, info};

use blazil_transport::aeron::{AeronContext, AeronPublication, AeronSubscription};

use blazil_inference_service::protocol::{
    deserialize_response, serialize_request, InferenceRequest, INFERENCE_REQ_STREAM_ID,
    INFERENCE_RSP_STREAM_ID,
};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Aeron IPC channel (must match server configuration).
const AERON_CHANNEL: &str = "aeron:ipc?term-length=67108864";

/// Aeron IPC directory (must match server).
const AERON_DIR: &str = "/tmp/aeron-inference-hybrid";

/// Timeout for publication/subscription registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum fragments to poll per iteration.
const FRAGMENT_LIMIT: usize = 256;

/// Poll timeout (seconds to wait for response).
const POLL_TIMEOUT_SECS: u64 = 30;

// ── Main Test Client ──────────────────────────────────────────────────────────

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    info!("🚀 Blazil Inference Test Client starting...");

    // Attach to existing Aeron driver (server already started one)
    info!("✅ Attaching to existing Aeron driver (dir: {})", AERON_DIR);

    // Create Aeron context (no need to start driver, reuse existing one)
    let ctx = AeronContext::new(AERON_DIR)?;
    info!("✅ Aeron context created");

    // Create publication to server (stream 2001)
    let pub_to_server = AeronPublication::new(
        &ctx,
        AERON_CHANNEL,
        INFERENCE_REQ_STREAM_ID,
        REGISTRATION_TIMEOUT,
    )?;
    info!(
        "✅ Publication created: {} → stream {}",
        AERON_CHANNEL, INFERENCE_REQ_STREAM_ID
    );

    // Create subscription from server (stream 2002)
    let sub_from_server = AeronSubscription::new(
        &ctx,
        AERON_CHANNEL,
        INFERENCE_RSP_STREAM_ID,
        REGISTRATION_TIMEOUT,
    )?;
    info!(
        "✅ Subscription created: {} ← stream {}",
        AERON_CHANNEL, INFERENCE_RSP_STREAM_ID
    );

    // Wait for publication to connect
    info!("⏳ Waiting for publication to connect...");
    let pub_start = Instant::now();
    while !pub_to_server.is_connected() {
        if pub_start.elapsed().as_secs() > 5 {
            anyhow::bail!("Publication failed to connect after 5 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    info!("✅ Publication connected");

    // Wait for subscription to connect
    info!("⏳ Waiting for subscription to connect...");
    let sub_start = Instant::now();
    while !sub_from_server.is_connected() {
        if sub_start.elapsed().as_secs() > 5 {
            anyhow::bail!("Subscription failed to connect after 5 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    info!("✅ Subscription connected");

    // ── Send Inference Request ────────────────────────────────────────────────

    let request_id = "test-001";

    // Raw prompt - server will inject system prompt and handle formatting
    let prompt = "What is 2 + 2?";
    let max_tokens = 16;

    info!("📤 Sending raw prompt (server handles formatting)");

    let request = InferenceRequest {
        request_id: request_id.to_string(),
        input_data: prompt.as_bytes().to_vec(),
        input_shape: vec![max_tokens as u32, 0, 0], // First element = max_tokens
        model_version: "v1".to_string(),
    };

    let request_bytes = serialize_request(&request)?;
    info!(
        "   Request size: {} bytes (prompt: {} chars, max_tokens: {})",
        request_bytes.len(),
        prompt.len(),
        max_tokens
    );

    // Send request (retry on backpressure)
    let send_start = Instant::now();
    loop {
        match pub_to_server.offer(&request_bytes) {
            Ok(_) => {
                info!("✅ Request sent successfully");
                break;
            }
            Err(e) => {
                if send_start.elapsed().as_secs() > 5 {
                    anyhow::bail!("Failed to send request after 5 seconds: {}", e);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    // ── Poll for Response ─────────────────────────────────────────────────────

    info!(
        "⏳ Waiting for response (timeout: {}s)...",
        POLL_TIMEOUT_SECS
    );

    let poll_start = Instant::now();
    let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);
    let mut received_response = false;

    while poll_start.elapsed().as_secs() < POLL_TIMEOUT_SECS {
        fragments.clear();

        let fragments_read = sub_from_server.poll_fragments(&mut fragments, FRAGMENT_LIMIT);

        if fragments_read > 0 {
            info!("📥 Received {} fragment(s)", fragments_read);

            for buffer in &fragments {
                match deserialize_response(buffer) {
                    Ok(response) => {
                        received_response = true;

                        if !response.error.is_empty() {
                            error!("❌ Inference error: {}", response.error);
                            continue;
                        }

                        // Decode raw_output (f32 bytes) back to UTF-8 text
                        let text_bytes: Vec<u8> =
                            response.raw_output.iter().map(|&f| f as u8).collect();
                        let generated_text = String::from_utf8_lossy(&text_bytes);

                        // Calculate metrics
                        let latency_ms = response.latency_us as f64 / 1000.0;
                        let token_count = generated_text.split_whitespace().count();
                        let tokens_per_sec = if latency_ms > 0.0 {
                            (token_count as f64 / latency_ms) * 1000.0
                        } else {
                            0.0
                        };

                        // Print results
                        info!("✅ Response received:");
                        info!("   Request ID: {}", response.request_id);
                        info!("   Generated text: '{}'", generated_text.trim());
                        info!("   Latency: {:.2} ms", latency_ms);
                        info!("   Tokens: ~{}", token_count);
                        info!("   Throughput: {:.2} tokens/sec", tokens_per_sec);

                        break;
                    }
                    Err(e) => {
                        error!("❌ Failed to deserialize response: {}", e);
                    }
                }
            }

            if received_response {
                break;
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if !received_response {
        anyhow::bail!("No response received after {} seconds", POLL_TIMEOUT_SECS);
    }

    info!("🎉 Test completed successfully!");

    Ok(())
}
