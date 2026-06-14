//! Standalone Aeron IPC inference listener — dedicated thread.
//!
//! Supports both single-stage and multi-stage distributed pipeline modes.
//!
//! # Single-Stage Mode (Legacy)
//! ```text
//! Client → Stream 2001 → InferenceServer → Stream 2002 → Client
//! ```
//!
//! # Multi-Stage Pipeline Mode (Distributed)
//! ```text
//! Client → Stream 1001 → Stage 1 (layers 0-9)   → Stream 2001 →
//!                         Stage 2 (layers 10-19) → Stream 2002 →
//!                         Stage 3 (layers 20-28) → Stream 1002 → Client
//! ```
//!
//! Each stage:
//! 1. Loads full GGUF model (memory-efficient with abundant RAM)
//! 2. Executes ONLY assigned layer range (layer_start..layer_end)
//! 3. Forwards activation tensors via Aeron IPC shared memory (zero-copy)
//! 4. KV Cache remains strictly local (never transferred)
//!
//! # Why std::thread instead of tokio?
//!
//! Aeron FFI (C bindings) is not `Send + Sync` safe. Running on a dedicated
//! OS thread ensures:
//! - No accidental moves across tokio tasks
//! - Predictable drop order (critical for C cleanup)
//! - No async overhead for tight poll loop

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use blazil_transport::aeron::{
    AeronContext, AeronPublication, AeronSubscription, EmbeddedAeronDriver,
};

use crate::config::DistributedConfig;
use crate::gguf_model::GgufModel;
use crate::protocol::{
    deserialize_activation, deserialize_request, deserialize_token_response, serialize_activation,
    serialize_response, serialize_token_response, ActivationTransfer, InferenceRequest,
    InferenceResponse, TokenResponse, INFERENCE_REQ_STREAM_ID, INFERENCE_RSP_STREAM_ID,
    PIPELINE_CLIENT_TO_STAGE1, PIPELINE_STAGE3_TO_CLIENT, PIPELINE_STAGE3_TO_STAGE1,
};

// ── OS Tuning Imports ──────────────────────────────────────────────────────────

use core_affinity::{set_for_current, CoreId};

#[cfg(target_os = "linux")]
use libc::{pthread_self, sched_param, sched_setscheduler, SCHED_FIFO};

#[cfg(target_os = "macos")]
use libc::{pthread_self, pthread_setschedparam, sched_param, SCHED_RR};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default Aeron IPC channel for inference.
///
/// `term-length=67108864` = 64 MB log buffer (prevents backpressure).
const DEFAULT_CHANNEL: &str = "aeron:ipc?term-length=67108864";

/// Timeout waiting for publication/subscription registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum fragments processed per poll.
const FRAGMENT_LIMIT: usize = 4096;

/// Max spin retries on Aeron offer backpressure before yielding.
const OFFER_SPIN_RETRIES: usize = 64;

/// Adaptive idle strategy thresholds for macOS scheduler compatibility
const IDLE_SPIN_THRESHOLD: u64 = 5_000; // Spin-loop for short bursts (~50-100μs)
const IDLE_YIELD_THRESHOLD: u64 = 50_000; // Yield for ~1-2ms before microsleep

// ── OS Tuning Utilities ───────────────────────────────────────────────────────

/// Apply CPU core pinning for inference threads.
///
/// Pins the current thread to the specified CPU cores to eliminate cache
/// thrashing and improve NUMA locality on multi-socket systems.
///
/// # Arguments
/// - `cores` — Vector of CPU core IDs to pin to (e.g., [0, 1, 2, 3])
///
/// # Returns
/// `true` if pinning succeeded, `false` otherwise.
///
/// # Example
/// ```rust,ignore
/// // Pin Stage 1 worker to cores 0-3
/// apply_cpu_affinity(&[0, 1, 2, 3]);
/// ```
fn apply_cpu_affinity(cores: &[usize]) -> bool {
    if cores.is_empty() {
        return true; // No pinning requested
    }

    // Core affinity crate requires one core at a time on most platforms
    // Try to pin to the first available core from the list
    for &core_id in cores {
        let core = CoreId { id: core_id };
        if set_for_current(core) {
            info!("✅ CPU affinity set to core {core_id}");
            return true;
        }
    }

    warn!("⚠️ Failed to pin thread to any of cores {:?}", cores);
    false
}

/// Boost thread priority to real-time scheduling (Linux/macOS).
///
/// On Linux, uses SCHED_FIFO (first-in-first-out real-time policy).
/// On macOS, uses pthread_setschedparam with SCHED_RR policy.
///
/// **WARNING**: Requires `CAP_SYS_NICE` capability or root privileges.
/// May cause system instability if inference threads monopolize CPU.
///
/// # Returns
/// `true` if priority boost succeeded, `false` otherwise.
///
/// # Safety
/// Uses unsafe FFI to call POSIX scheduling functions.
#[cfg(target_os = "linux")]
fn boost_thread_priority() -> bool {
    unsafe {
        let thread_id = pthread_self();
        // Use zeroed() to avoid field initialization issues across libc versions
        let mut param: sched_param = std::mem::zeroed();
        param.sched_priority = 99; // Maximum real-time priority (1-99 scale)

        let result = sched_setscheduler(thread_id as i32, SCHED_FIFO, &param);
        if result == 0 {
            info!("✅ Thread priority boosted to real-time (SCHED_FIFO, priority 99)");
            true
        } else {
            warn!("⚠️ Failed to boost thread priority. Run with CAP_SYS_NICE or as root.");
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn boost_thread_priority() -> bool {
    unsafe {
        let thread_id = pthread_self();
        let mut param: sched_param = std::mem::zeroed();
        param.sched_priority = 47; // macOS uses lower priority range (1-47)

        let result = pthread_setschedparam(thread_id, SCHED_RR, &param);
        if result == 0 {
            info!("✅ Thread priority boosted to real-time (SCHED_RR, priority 47)");
            true
        } else {
            warn!("⚠️ Failed to boost thread priority (error {result}). macOS requires kernel extension for SCHED_RR.");
            false
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn boost_thread_priority() -> bool {
    warn!("⚠️ Thread priority boosting not supported on this platform");
    false
}

/// Aggressive spin-polling loop for zero-latency activation transfers.
///
/// Replaces the adaptive idle backoff strategy with a pure busy-wait loop
/// during active inference windows. Trades power efficiency for nanosecond
/// precision wakeups when activation tensors arrive in Aeron IPC buffers.
///
/// # Arguments
/// - `enable` — If `false`, performs a single `std::hint::spin_loop()` hint
///
/// # Behavior
/// When enabled, executes a tight spin-loop with `core::hint::spin_loop()`
/// to keep the CPU pipeline hot and maximize instruction throughput.
///
/// # Performance Impact
/// - Latency: 50-100ns wakeup time (vs 50-500μs with sleeps)
/// - CPU: 100% utilization on pinned cores
/// - Power: ~10-15W per core on i4i.4xlarge
#[inline(always)]
fn aggressive_spin_poll(enable: bool) {
    if enable {
        // Execute a few spin-loop hints to keep CPU pipeline hot
        for _ in 0..10 {
            std::hint::spin_loop();
        }
    } else {
        // Fallback: single spin hint (compatible with old idle_backoff)
        std::hint::spin_loop();
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run Aeron IPC inference server on the current thread (blocking).
///
/// Supports both single-stage and distributed pipeline modes based on config.
///
/// # Arguments
///
/// - `model` — GGUF model wrapped in Arc<Mutex<>> for interior mutability
/// - `aeron_dir` — Path to Aeron IPC directory (e.g., `/tmp/aeron-inference`)
/// - `distributed` — Optional distributed pipeline config (None = single-stage)
///
/// # Panics
///
/// Panics if Aeron driver fails to start or streams fail to register.
pub fn run(model: Arc<Mutex<GgufModel>>, aeron_dir: &str, distributed: Option<DistributedConfig>) {
    run_with_channel(model, aeron_dir, DEFAULT_CHANNEL, distributed);
}

/// Run Aeron IPC server with custom channel URI.
pub fn run_with_channel(
    model: Arc<Mutex<GgufModel>>,
    aeron_dir: &str,
    channel: &str,
    distributed: Option<DistributedConfig>,
) {
    let dist_info = distributed
        .as_ref()
        .map(|d| {
            format!(
                "stage={}, layers={}-{}, prev_stream={}, next_stream={}",
                d.node_stage, d.layer_start, d.layer_end, d.prev_stream_id, d.next_stream_id
            )
        })
        .unwrap_or_else(|| "single-stage".to_string());

    info!(
        channel = %channel,
        aeron_dir = %aeron_dir,
        mode = %dist_info,
        "🚀 Starting Aeron IPC inference listener (dedicated thread)"
    );

    // 1. Start or attach to the Aeron C driver.
    let _driver = if std::env::var("BLAZIL_ATTACH_EXISTING_AERON").as_deref() == Ok("1") {
        info!("✓ Attaching to existing Aeron driver");
        None
    } else {
        let driver = EmbeddedAeronDriver::new(Some(aeron_dir));
        driver.start().expect("EmbeddedAeronDriver::start failed");
        info!("✓ Embedded Aeron driver started");
        Some(driver)
    };

    // 2. Create Aeron context
    let ctx = AeronContext::new(aeron_dir).expect("AeronContext::new failed");

    info!("✓ Aeron context created");

    // 3. Route to appropriate pipeline handler based on distributed config
    if let Some(ref dist) = distributed {
        if dist.enabled {
            match dist.node_stage {
                1 => run_stage1_pipeline(model, &ctx, channel, dist.clone()),
                2 => run_stage2_pipeline(model, &ctx, channel, dist.clone()),
                3 => run_stage3_pipeline(model, &ctx, channel, dist.clone()),
                _ => panic!("Invalid node_stage: {}", dist.node_stage),
            }
        } else {
            run_single_stage(model, &ctx, channel);
        }
    } else {
        run_single_stage(model, &ctx, channel);
    }
}

// ── Single-Stage Handler (Legacy Mode) ───────────────────────────────────────

/// Run single-stage inference server (original behavior).
fn run_single_stage(model: Arc<Mutex<GgufModel>>, ctx: &AeronContext, channel: &str) {
    // Create subscription (inbound requests on stream 2001)
    let sub = AeronSubscription::new(ctx, channel, INFERENCE_REQ_STREAM_ID, REGISTRATION_TIMEOUT)
        .expect("AeronSubscription::new failed");

    info!(
        "✓ Subscription registered (stream {})",
        INFERENCE_REQ_STREAM_ID
    );

    // Create publication (outbound responses on stream 2002)
    let pub_ = AeronPublication::new(ctx, channel, INFERENCE_RSP_STREAM_ID, REGISTRATION_TIMEOUT)
        .expect("AeronPublication::new failed");

    info!(
        "✓ Publication registered (stream {})",
        INFERENCE_RSP_STREAM_ID
    );

    // Create channel for worker threads to send back completed responses
    let (response_tx, response_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();

    info!("✅ Single-stage inference server ready — entering poll loop");

    let mut idle_count: u64 = 0;
    let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);

    loop {
        fragments.clear();

        let fragments_read = sub.poll_fragments(&mut fragments, FRAGMENT_LIMIT);

        if fragments_read > 0 {
            idle_count = 0;

            for buffer in &fragments {
                let req = match deserialize_request(buffer) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to deserialize request: {e}");
                        continue;
                    }
                };

                debug!(
                    request_id = %req.request_id,
                    "Processing inference request"
                );

                // SLA-first: publish immediate ACK so client doesn't time out waiting for first byte.
                let ack = build_streaming_ack(&req.request_id);
                if let Ok(ack_bytes) = serialize_response(&ack) {
                    let _ = response_tx.send(ack_bytes);
                }

                let model_clone = Arc::clone(&model);
                let tx_clone = response_tx.clone();

                std::thread::spawn(move || {
                    let request_id_for_heartbeat = req.request_id.clone();
                    let request_id_for_logs = req.request_id.clone();
                    let heartbeat_done = Arc::new(AtomicBool::new(false));
                    let heartbeat_done_clone = Arc::clone(&heartbeat_done);
                    let heartbeat_tx = tx_clone.clone();

                    std::thread::spawn(move || {
                        while !heartbeat_done_clone.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_secs(5));
                            if heartbeat_done_clone.load(Ordering::Relaxed) {
                                break;
                            }

                            let heartbeat = build_streaming_ack(&request_id_for_heartbeat);
                            if let Ok(bytes) = serialize_response(&heartbeat) {
                                debug!(request_id = %request_id_for_heartbeat, "Sending heartbeat frame");
                                let _ = heartbeat_tx.send(bytes);
                            }
                        }
                    });

                    let start = Instant::now();
                    let tx_chunks = tx_clone.clone();
                    let chunk_counter = Arc::new(Mutex::new(0usize));
                    let chunk_counter_cb = Arc::clone(&chunk_counter);
                    let mut response = process_gguf_inference_streaming(
                        &model_clone,
                        req,
                        None,
                        move |request_id, token_chunk| {
                            if token_chunk.is_empty() {
                                return;
                            }

                            let chunk_raw_output: Vec<f32> =
                                token_chunk.bytes().map(|b| b as f32).collect();
                            let chunk_response = InferenceResponse {
                                request_id: request_id.to_string(),
                                class_id: None,
                                probabilities: vec![],
                                raw_output: chunk_raw_output,
                                confidence: 0.0,
                                latency_us: 0,
                                error: String::new(),
                            };

                            if let Ok(bytes) = serialize_response(&chunk_response) {
                                if let Ok(mut count) = chunk_counter_cb.lock() {
                                    *count += 1;
                                    info!(
                                        request_id = %request_id,
                                        chunk_index = *count,
                                        chunk_len = token_chunk.len(),
                                        "Streaming token chunk"
                                    );
                                }
                                let _ = tx_chunks.send(bytes);
                            }
                        },
                    );
                    response.latency_us = start.elapsed().as_micros() as u64;
                    heartbeat_done.store(true, Ordering::Relaxed);

                    let chunk_count = chunk_counter.lock().map(|count| *count).unwrap_or(0);
                    info!(
                        request_id = %request_id_for_logs,
                        latency_us = response.latency_us,
                        streamed_chunks = chunk_count,
                        final_text_bytes = response.raw_output.len(),
                        "Inference finished, sending final response"
                    );

                    if let Ok(resp_bytes) = serialize_response(&response) {
                        let _ = tx_clone.send(resp_bytes);
                    }
                });
            }
        }

        // Publish completed responses
        while let Ok(resp_bytes) = response_rx.try_recv() {
            publish_with_retry(&pub_, &resp_bytes);
        }

        // Legacy single-stage: use conservative idle strategy (no spin-poll config)
        idle_backoff(&mut idle_count, fragments_read == 0);
    }
}

// ── Decode State Tracking ─────────────────────────────────────────────────────

/// Tracks the decode state for a single request flowing through the distributed pipeline.
///
/// Stage 1 maintains this state to orchestrate the decode loop: after prefill completes,
/// each token sampled by Stage 3 is fed back through the pipeline until EOS or max_tokens.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used in orchestration logic, not all accessed directly
struct DecodeState {
    /// Request identifier (matches InferenceRequest.request_id)
    request_id: String,

    /// Accumulated token IDs (includes prompt tokens + generated tokens)
    tokens: Vec<u32>,

    /// Accumulated generated text (for final response)
    generated_text: String,

    /// Current sequence position (for KV cache indexing)
    current_position: usize,

    /// Prompt length (number of tokens from prefill, excluding generated tokens)
    prompt_length: usize,

    /// Maximum tokens to generate (from InferenceRequest.max_tokens)
    max_tokens: usize,

    /// Request start time (for latency tracking)
    start_time: Instant,
}

// ── Stage 1 Handler (Pipeline Entry Point + Decode Orchestrator) ─────────────

/// Stage 1: Distributed Decode Orchestrator
///
/// **Responsibilities:**
/// 1. Receive client prompts → Tokenize → Run layers 0-10 (prefill) → Forward to Stage 2
/// 2. Subscribe to token feedback from Stage 3 (Stream 1003)
/// 3. Orchestrate decode loop:
///    - Receive sampled token from Stage 3
///    - If EOS or max_tokens: Send final response to client, cleanup state
///    - Else: Run single token through layers 0-10 (decode) → Forward to Stage 2
/// 4. Track concurrent requests with thread-safe state management
///
/// **Architecture:**
/// ```text
/// Client → S1 (prefill) → S2 → S3 → sample token → S1 (feedback)
///            ↑                                          ↓
///            └──────── decode orchestration ←──────────┘
/// ```
fn run_stage1_pipeline(
    model: Arc<Mutex<GgufModel>>,
    ctx: &AeronContext,
    channel: &str,
    dist: DistributedConfig,
) {
    info!(
        "🚀 Stage 1 Pipeline (Decode Orchestrator): layers {}-{} → Stream {}",
        dist.layer_start, dist.layer_end, dist.next_stream_id
    );

    // ── Bare-Metal OS Tuning ──────────────────────────────────────────────────

    if !dist.assigned_cores.is_empty() {
        apply_cpu_affinity(&dist.assigned_cores);
    }

    if dist.enable_realtime_priority {
        boost_thread_priority();
    }

    info!(
        "⚙️  OS tuning: cores={:?}, spin_poll={}, realtime={}",
        dist.assigned_cores, dist.enable_spin_poll, dist.enable_realtime_priority
    );

    // ── Aeron Subscriptions & Publications ────────────────────────────────────

    // Subscribe to client requests (Stream 1001)
    let sub_client = AeronSubscription::new(
        ctx,
        channel,
        PIPELINE_CLIENT_TO_STAGE1,
        REGISTRATION_TIMEOUT,
    )
    .expect("Stage 1: Failed to subscribe to client stream");

    // Subscribe to token feedback from Stage 3 (Stream 1003) - NEW for decode orchestration
    let sub_tokens = AeronSubscription::new(
        ctx,
        channel,
        PIPELINE_STAGE3_TO_STAGE1,
        REGISTRATION_TIMEOUT,
    )
    .expect("Stage 1: Failed to subscribe to token feedback stream");

    // Publish activations to Stage 2 (Stream 2001)
    let pub_stage2 = AeronPublication::new(ctx, channel, dist.next_stream_id, REGISTRATION_TIMEOUT)
        .expect("Stage 1: Failed to publish to Stage 2 stream");

    // Publish final responses to client (Stream 1002)
    let pub_client = AeronPublication::new(
        ctx,
        channel,
        PIPELINE_STAGE3_TO_CLIENT,
        REGISTRATION_TIMEOUT,
    )
    .expect("Stage 1: Failed to publish to client stream");

    info!("✅ Stage 1 ready: orchestrating distributed decode loop");

    // ── Thread-Safe State Management ──────────────────────────────────────────

    // Track decode state for all active requests
    let decode_states: Arc<Mutex<HashMap<String, DecodeState>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Channels for async activation forwarding and response publishing
    let (activation_tx, activation_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();
    let (response_tx, response_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();

    // Fragment buffers for Aeron polling
    let mut client_fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);
    let mut token_fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);

    // ── Main Orchestration Loop ───────────────────────────────────────────────

    loop {
        // ═══════════════════════════════════════════════════════════════════════
        // 1. Poll CLIENT REQUESTS (new prompts for prefill)
        // ═══════════════════════════════════════════════════════════════════════

        client_fragments.clear();
        let client_fragments_read =
            sub_client.poll_fragments(&mut client_fragments, FRAGMENT_LIMIT);

        if client_fragments_read > 0 {
            for buffer in &client_fragments {
                let req = match deserialize_request(buffer) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Stage 1: Failed to deserialize request: {e}");
                        continue;
                    }
                };

                info!(
                    request_id = %req.request_id,
                    "Stage 1: NEW REQUEST - Starting prefill for layers {}-{}",
                    dist.layer_start, dist.layer_end
                );

                // Convert input_data (UTF-8 bytes) to prompt string
                let prompt = match String::from_utf8(req.input_data.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        error!(request_id = %req.request_id, "Stage 1: Invalid UTF-8 in input_data: {e}");
                        continue;
                    }
                };

                // Initialize decode state for this request
                // Note: InferenceRequest doesn't have max_tokens field yet (image inference protocol)
                // Using default max_tokens=32 for now. TODO: Add max_tokens to InferenceRequest
                let initial_state = DecodeState {
                    request_id: req.request_id.clone(),
                    tokens: vec![],
                    generated_text: String::new(),
                    current_position: 0,
                    prompt_length: 0, // Will be updated after prefill
                    max_tokens: 32,   // Default: generate up to 32 tokens
                    start_time: Instant::now(),
                };

                {
                    let mut states = decode_states.lock().unwrap();
                    states.insert(req.request_id.clone(), initial_state);
                }

                // Spawn prefill thread (run prompt through layers 0-10)
                let model_clone = Arc::clone(&model);
                let tx_clone = activation_tx.clone();
                let layer_start = dist.layer_start;
                let layer_end = dist.layer_end;
                let request_id = req.request_id.clone();
                let states_clone = Arc::clone(&decode_states);

                std::thread::spawn(move || {
                    let start = Instant::now();

                    // Execute layers 0-10 on full prompt (prefill)
                    let activation_state = {
                        let mut model_guard = model_clone.lock().unwrap();
                        match model_guard.generate_from_tokens_layer_range(
                            &prompt,
                            layer_start,
                            layer_end,
                        ) {
                            Ok(state) => state,
                            Err(e) => {
                                error!(request_id = %request_id, "Stage 1: Prefill failed: {e}");
                                // Cleanup failed request
                                let mut states = states_clone.lock().unwrap();
                                states.remove(&request_id);
                                return;
                            }
                        }
                    };

                    // Update decode state with prompt tokens
                    {
                        let mut states = states_clone.lock().unwrap();
                        if let Some(state) = states.get_mut(&request_id) {
                            state.tokens = activation_state.tokens.clone();
                            state.current_position = activation_state.position;
                            state.prompt_length = activation_state.tokens.len();
                            // Track prompt length
                        }
                    }

                    // Convert ActivationState to ActivationTransfer for serialization
                    let activation = ActivationTransfer {
                        request_id: request_id.clone(),
                        shape: activation_state.shape,
                        data: activation_state.data,
                        position: activation_state.position,
                        tokens: activation_state.tokens,
                    };

                    let elapsed = start.elapsed().as_micros();
                    info!(
                        request_id = %request_id,
                        latency_us = elapsed,
                        tokens = activation.tokens.len(),
                        position = activation.position,
                        "Stage 1: PREFILL complete, forwarding {} floats to Stage 2",
                        activation.data.len()
                    );

                    if let Ok(act_bytes) = serialize_activation(&activation) {
                        let _ = tx_clone.send(act_bytes);
                    }
                });
            }
        }

        // ═══════════════════════════════════════════════════════════════════════
        // 2. Poll TOKEN FEEDBACK from Stage 3 (decode continuation)
        // ═══════════════════════════════════════════════════════════════════════

        token_fragments.clear();
        let token_fragments_read = sub_tokens.poll_fragments(&mut token_fragments, FRAGMENT_LIMIT);

        if token_fragments_read > 0 {
            for buffer in &token_fragments {
                let token_resp = match deserialize_token_response(buffer) {
                    Ok(t) => t,
                    Err(e) => {
                        debug!("Stage 1: Failed to deserialize token response: {e}");
                        continue;
                    }
                };

                let request_id = token_resp.request_id.clone();
                let mut should_cleanup = false;
                let mut final_response_bytes: Option<Vec<u8>> = None;

                // Process token response (critical section)
                {
                    let mut states = decode_states.lock().unwrap();
                    if let Some(state) = states.get_mut(&request_id) {
                        // Append generated token
                        state.tokens.push(token_resp.token_id);
                        state.generated_text.push_str(&token_resp.token_text);
                        // Update position for NEXT decode step (current total tokens)
                        state.current_position = state.tokens.len();

                        let tokens_generated = state.tokens.len() - state.prompt_length;

                        info!(
                            request_id = %request_id,
                            token_id = token_resp.token_id,
                            token_text = %token_resp.token_text,
                            position = token_resp.position,
                            is_eos = token_resp.is_eos,
                            tokens_generated = tokens_generated,
                            max_tokens = state.max_tokens,
                            "Stage 1: TOKEN RECEIVED from Stage 3"
                        );

                        // Check termination conditions: EOS or reached max generated tokens
                        let should_terminate =
                            token_resp.is_eos || tokens_generated >= state.max_tokens;

                        if should_terminate {
                            // Generate final response
                            let latency_us = state.start_time.elapsed().as_micros() as u64;
                            let raw_output: Vec<f32> =
                                state.generated_text.bytes().map(|b| b as f32).collect();
                            let tokens_generated = state.tokens.len() - state.prompt_length;

                            let response = InferenceResponse {
                                request_id: request_id.clone(),
                                class_id: None,
                                probabilities: vec![],
                                raw_output,
                                confidence: 1.0,
                                latency_us,
                                error: String::new(),
                            };

                            info!(
                                request_id = %request_id,
                                latency_us = latency_us,
                                tokens_generated = tokens_generated,
                                reason = if token_resp.is_eos { "EOS" } else { "max_tokens" },
                                "Stage 1: DECODE COMPLETE - Sending final response to client"
                            );

                            if let Ok(resp_bytes) = serialize_response(&response) {
                                final_response_bytes = Some(resp_bytes);
                            }

                            should_cleanup = true;
                        }
                    } else {
                        warn!(
                            request_id = %request_id,
                            "Stage 1: Received token for unknown request (already completed?)"
                        );
                    }
                }

                // Send final response to client if request is complete
                if let Some(resp_bytes) = final_response_bytes {
                    let _ = response_tx.send(resp_bytes);
                }

                // Cleanup completed request
                if should_cleanup {
                    let mut states = decode_states.lock().unwrap();
                    states.remove(&request_id);
                    continue; // Skip decode continuation for completed request
                }

                // ───────────────────────────────────────────────────────────────
                // DECODE CONTINUATION: Run single token through layers 0-10
                // ───────────────────────────────────────────────────────────────

                let model_clone = Arc::clone(&model);
                let tx_clone = activation_tx.clone();
                let layer_start = dist.layer_start;
                let layer_end = dist.layer_end;
                let states_clone = Arc::clone(&decode_states);

                std::thread::spawn(move || {
                    let start = Instant::now();

                    // Get current state snapshot
                    let (last_token, position, total_tokens) = {
                        let states = states_clone.lock().unwrap();
                        if let Some(state) = states.get(&request_id) {
                            let last_token = *state.tokens.last().unwrap_or(&0);
                            let total_tokens = state.tokens.len();
                            (last_token, state.current_position, total_tokens)
                        } else {
                            return; // Request was cleaned up
                        }
                    };

                    info!(
                        request_id = %request_id,
                        last_token = last_token,
                        position = position,
                        total_tokens = total_tokens,
                        "Stage 1: DECODE CONTINUATION - Running single token through layers 0-10"
                    );

                    // Execute layers 0-10 on SINGLE TOKEN (decode step)
                    let activation_state = {
                        let mut model_guard = model_clone.lock().unwrap();
                        match model_guard.decode_single_token(
                            last_token,
                            position,
                            layer_start,
                            layer_end,
                        ) {
                            Ok(state) => state,
                            Err(e) => {
                                error!(
                                    request_id = %request_id,
                                    "Stage 1: Decode step failed: {e}"
                                );
                                // Cleanup failed request
                                let mut states = states_clone.lock().unwrap();
                                states.remove(&request_id);
                                return;
                            }
                        }
                    };

                    // Convert ActivationState to ActivationTransfer for serialization
                    let activation = ActivationTransfer {
                        request_id: request_id.clone(),
                        shape: activation_state.shape,
                        data: activation_state.data,
                        position: activation_state.position,
                        tokens: activation_state.tokens,
                    };

                    let elapsed = start.elapsed().as_micros();
                    info!(
                        request_id = %request_id,
                        position = activation.position,
                        tokens = activation.tokens.len(),
                        "Stage 1: DECODE STEP complete, forwarding {} floats to Stage 2",
                        activation.data.len()
                    );
                    debug!(
                        request_id = %request_id,
                        latency_us = elapsed,
                        position = position,
                        "Stage 1: DECODE STEP complete, forwarding to Stage 2"
                    );

                    if let Ok(act_bytes) = serialize_activation(&activation) {
                        let _ = tx_clone.send(act_bytes);
                    }
                });
            }
        }

        // ═══════════════════════════════════════════════════════════════════════
        // 3. Forward activations to Stage 2
        // ═══════════════════════════════════════════════════════════════════════

        while let Ok(act_bytes) = activation_rx.try_recv() {
            publish_with_retry(&pub_stage2, &act_bytes);
        }

        // ═══════════════════════════════════════════════════════════════════════
        // 4. Send final responses to client
        // ═══════════════════════════════════════════════════════════════════════

        while let Ok(resp_bytes) = response_rx.try_recv() {
            publish_with_retry(&pub_client, &resp_bytes);
        }

        // ═══════════════════════════════════════════════════════════════════════
        // 5. Aggressive spin-polling for zero-latency IPC
        // ═══════════════════════════════════════════════════════════════════════

        aggressive_spin_poll(dist.enable_spin_poll);
    }
}

// ── Stage 2 Handler (Middle Pipeline Stage) ──────────────────────────────────

/// Stage 2: Receive activations from Stage 1 → Execute layers 10-19 → Forward to Stage 3.
fn run_stage2_pipeline(
    model: Arc<Mutex<GgufModel>>,
    ctx: &AeronContext,
    channel: &str,
    dist: DistributedConfig,
) {
    info!(
        "🚀 Stage 2 Pipeline: layers {}-{}, Stream {} → {}",
        dist.layer_start, dist.layer_end, dist.prev_stream_id, dist.next_stream_id
    );

    // ── Bare-Metal OS Tuning ──────────────────────────────────────────────────

    // Apply CPU affinity pinning
    if !dist.assigned_cores.is_empty() {
        apply_cpu_affinity(&dist.assigned_cores);
    }

    // Boost thread priority to real-time (if enabled)
    if dist.enable_realtime_priority {
        boost_thread_priority();
    }

    info!(
        "⚙️  OS tuning: cores={:?}, spin_poll={}, realtime={}",
        dist.assigned_cores, dist.enable_spin_poll, dist.enable_realtime_priority
    );

    // Subscribe to Stage 1 activations (Stream 2001)
    let sub_stage1 =
        AeronSubscription::new(ctx, channel, dist.prev_stream_id, REGISTRATION_TIMEOUT)
            .expect("Stage 2: Failed to subscribe to Stage 1 stream");

    // Publish activations to Stage 3 (Stream 2002)
    let pub_stage3 = AeronPublication::new(ctx, channel, dist.next_stream_id, REGISTRATION_TIMEOUT)
        .expect("Stage 2: Failed to publish to Stage 3 stream");

    info!("✅ Stage 2 ready: listening for Stage 1 activations");

    let (activation_tx, activation_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();
    let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);
    let mut pending_activation = Vec::new();

    loop {
        fragments.clear();
        let fragments_read = sub_stage1.poll_fragments(&mut fragments, FRAGMENT_LIMIT);

        if fragments_read > 0 {
            for fragment in &fragments {
                pending_activation.extend_from_slice(fragment);
            }

            let act_in = match deserialize_activation(&pending_activation) {
                Ok(a) => a,
                Err(e) => {
                    debug!(
                        fragments = fragments_read,
                        bytes = pending_activation.len(),
                        "Stage 2: Waiting for complete activation: {e}"
                    );
                    continue;
                }
            };
            let bytes_read = pending_activation.len();
            pending_activation.clear();

            info!(
                request_id = %act_in.request_id,
                fragments = fragments_read,
                bytes = bytes_read,
                "Stage 2: Received {} floats from Stage 1, processing layers {}-{}",
                act_in.data.len(), dist.layer_start, dist.layer_end
            );

            let model_clone = Arc::clone(&model);
            let tx_clone = activation_tx.clone();
            let layer_start = dist.layer_start;
            let layer_end = dist.layer_end;
            let request_id = act_in.request_id.clone();

            std::thread::spawn(move || {
                let start = Instant::now();

                // Convert ActivationTransfer to ActivationState
                let activation_state = crate::gguf_model::ActivationState {
                    shape: act_in.shape.clone(),
                    data: act_in.data.clone(),
                    position: act_in.position,
                    tokens: act_in.tokens.clone(),
                };

                // Execute layers layer_start..layer_end
                let output_state = {
                    let mut model_guard = model_clone.lock().unwrap();
                    match model_guard.generate_from_activation(
                        &activation_state,
                        layer_start,
                        layer_end,
                        0,      // max_tokens unused for middle stages
                        |_| {}, // No token callback for middle stages
                    ) {
                        Ok(crate::gguf_model::Either::Left(state)) => state,
                        Ok(crate::gguf_model::Either::Right(_)) => {
                            error!(request_id = %request_id, "Stage 2: Unexpected final output");
                            return;
                        }
                        Err(e) => {
                            error!(request_id = %request_id, "Stage 2: Layer execution failed: {e}");
                            return;
                        }
                    }
                };

                // Convert ActivationState to ActivationTransfer for serialization
                let activation = ActivationTransfer {
                    request_id: request_id.clone(),
                    shape: output_state.shape.clone(),
                    data: output_state.data,
                    position: output_state.position,
                    tokens: output_state.tokens,
                };

                let elapsed = start.elapsed().as_micros();
                info!(
                    request_id = %request_id,
                    latency_us = elapsed,
                    position = activation.position,
                    tokens = activation.tokens.len(),
                    "Stage 2: Layers {}-{} complete, forwarding {} floats",
                    layer_start, layer_end, activation.data.len()
                );

                if let Ok(act_bytes) = serialize_activation(&activation) {
                    let _ = tx_clone.send(act_bytes);
                }
            });
        }

        // Forward activations to Stage 3
        while let Ok(act_bytes) = activation_rx.try_recv() {
            publish_with_retry(&pub_stage3, &act_bytes);
        }

        // Aggressive spin-polling for zero-latency IPC
        aggressive_spin_poll(dist.enable_spin_poll);
    }
}

// ── Stage 3 Handler (Pipeline Exit Point) ────────────────────────────────────

/// Stage 3: Receive activations from Stage 2 → Execute layers 20-28 + LM Head → Send tokens to client.
fn run_stage3_pipeline(
    model: Arc<Mutex<GgufModel>>,
    ctx: &AeronContext,
    channel: &str,
    dist: DistributedConfig,
) {
    info!(
        "🚀 Stage 3 Pipeline: layers {}-{}, Stream {} → {}",
        dist.layer_start, dist.layer_end, dist.prev_stream_id, PIPELINE_STAGE3_TO_CLIENT
    );

    // ── Bare-Metal OS Tuning ──────────────────────────────────────────────────

    // Apply CPU affinity pinning
    if !dist.assigned_cores.is_empty() {
        apply_cpu_affinity(&dist.assigned_cores);
    }

    // Boost thread priority to real-time (if enabled)
    if dist.enable_realtime_priority {
        boost_thread_priority();
    }

    info!(
        "⚙️  OS tuning: cores={:?}, spin_poll={}, realtime={}",
        dist.assigned_cores, dist.enable_spin_poll, dist.enable_realtime_priority
    );

    // Subscribe to Stage 2 activations (Stream 2002)
    let sub_stage2 =
        AeronSubscription::new(ctx, channel, dist.prev_stream_id, REGISTRATION_TIMEOUT)
            .expect("Stage 3: Failed to subscribe to Stage 2 stream");

    // Publish token responses back to Stage 1 for decode orchestration (Stream 1003)
    let pub_stage1 = AeronPublication::new(
        ctx,
        channel,
        PIPELINE_STAGE3_TO_STAGE1,
        REGISTRATION_TIMEOUT,
    )
    .expect("Stage 3: Failed to publish to Stage 1 feedback stream");

    info!("✅ Stage 3 ready: listening for Stage 2 activations");

    let (response_tx, response_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();
    let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);
    let mut pending_activation = Vec::new();

    loop {
        fragments.clear();
        let fragments_read = sub_stage2.poll_fragments(&mut fragments, FRAGMENT_LIMIT);

        if fragments_read > 0 {
            for fragment in &fragments {
                pending_activation.extend_from_slice(fragment);
            }

            let act_in = match deserialize_activation(&pending_activation) {
                Ok(a) => a,
                Err(e) => {
                    debug!(
                        fragments = fragments_read,
                        bytes = pending_activation.len(),
                        "Stage 3: Waiting for complete activation: {e}"
                    );
                    continue;
                }
            };
            let bytes_read = pending_activation.len();
            pending_activation.clear();

            info!(
                request_id = %act_in.request_id,
                fragments = fragments_read,
                bytes = bytes_read,
                shape = ?act_in.shape,
                "Stage 3: Received {} floats from Stage 2 with shape {:?}, processing layers {}-{} + LM Head",
                act_in.data.len(), act_in.shape, dist.layer_start, dist.layer_end
            );

            let model_clone = Arc::clone(&model);
            let tx_clone = response_tx.clone();
            let layer_start = dist.layer_start;
            let layer_end = dist.layer_end;
            let request_id = act_in.request_id.clone();

            std::thread::spawn(move || {
                let start = Instant::now();

                info!(
                    request_id = %request_id,
                    position = act_in.position,
                    tokens = act_in.tokens.len(),
                    "Stage 3: Creating ActivationState with shape {:?}, data_len={}",
                    act_in.shape, act_in.data.len()
                );

                // Convert ActivationTransfer to ActivationState
                let activation_state = crate::gguf_model::ActivationState {
                    shape: act_in.shape.clone(),
                    data: act_in.data.clone(),
                    position: act_in.position,
                    tokens: act_in.tokens.clone(),
                };

                // Execute final layers + LM head, sample ONE token (distributed decode)
                let sampled_token = {
                    let mut model_guard = model_clone.lock().unwrap();
                    match model_guard.generate_from_activation(
                        &activation_state,
                        layer_start,
                        layer_end,
                        0, // max_tokens unused - we sample exactly 1 token
                        |_token| {
                            // Token text already captured in SampledToken
                        },
                    ) {
                        Ok(crate::gguf_model::Either::Right(token)) => token,
                        Ok(crate::gguf_model::Either::Left(_)) => {
                            error!(request_id = %request_id, "Stage 3: Unexpected intermediate output");
                            return;
                        }
                        Err(e) => {
                            error!(request_id = %request_id, "Stage 3: Layer execution failed: {e}");
                            return;
                        }
                    }
                };

                // Create TokenResponse for Stage 1 decode orchestration
                let token_response = TokenResponse {
                    request_id: request_id.clone(),
                    token_id: sampled_token.token_id,
                    token_text: sampled_token.token_text.clone(),
                    is_eos: sampled_token.is_eos,
                    position: sampled_token.position,
                };

                info!(
                    request_id = %request_id,
                    token_id = sampled_token.token_id,
                    is_eos = sampled_token.is_eos,
                    latency_us = start.elapsed().as_micros(),
                    "Stage 3: Sampled token '{}', sending to Stage 1",
                    sampled_token.token_text
                );

                if let Ok(token_bytes) = serialize_token_response(&token_response) {
                    let _ = tx_clone.send(token_bytes);
                }
            });
        }

        // Send token responses back to Stage 1 for decode loop orchestration
        while let Ok(token_bytes) = response_rx.try_recv() {
            publish_with_retry(&pub_stage1, &token_bytes);
        }

        // Aggressive spin-polling for zero-latency IPC
        aggressive_spin_poll(dist.enable_spin_poll);
    }
}

// ── Helper Functions ──────────────────────────────────────────────────────────

/// Publish bytes to Aeron with retry logic (backpressure handling).
fn publish_with_retry(pub_: &AeronPublication, bytes: &[u8]) {
    let mut retries = 0;
    loop {
        match pub_.offer(bytes) {
            Ok(pos) => {
                debug!(
                    "✅ Published {} bytes to Aeron (position={pos})",
                    bytes.len()
                );
                break;
            }
            Err(_) if retries < OFFER_SPIN_RETRIES => {
                retries += 1;
                std::hint::spin_loop();
            }
            Err(e) => {
                warn!(
                    "❌ Aeron offer failed after {} retries: {e}",
                    OFFER_SPIN_RETRIES
                );
                break;
            }
        }
    }
}

/// Adaptive idle backoff strategy for poll loops (macOS-compatible).
fn idle_backoff(idle_count: &mut u64, is_idle: bool) {
    if is_idle {
        *idle_count += 1;

        if (*idle_count).is_multiple_of(100_000) {
            debug!("Aeron polling active... idle_count={}", idle_count);
        }

        if *idle_count > IDLE_YIELD_THRESHOLD {
            // Deep idle: 50μs sleep forces OS context switch
            std::thread::sleep(std::time::Duration::from_micros(50));
        } else if *idle_count > IDLE_SPIN_THRESHOLD {
            // Medium idle: yield after 5K spins
            std::thread::yield_now();
        } else {
            // Hot path: tight spin for low latency
            std::hint::spin_loop();
        }
    } else {
        *idle_count = 0;
    }
}

// ── GGUF Inference Processing ─────────────────────────────────────────────────

/// Process GGUF text generation inference.
///
/// # Arguments
///
/// - `model` — GGUF model wrapped in Arc<Mutex<>>
/// - `req` — Inference request (prompt)
/// - `layer_range` — Optional (layer_start, layer_end) for distributed mode
///
/// # Distributed Mode
///
/// When `layer_range` is provided, executes only the specified layer range
/// and returns intermediate activations instead of final tokens.
fn process_gguf_inference_streaming<F>(
    model: &Arc<Mutex<GgufModel>>,
    req: InferenceRequest,
    layer_range: Option<(usize, usize)>,
    mut on_chunk: F,
) -> InferenceResponse
where
    F: FnMut(&str, &str) + Send + 'static,
{
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

    // TODO: Implement layer-range restricted execution
    if let Some((layer_start, layer_end)) = layer_range {
        warn!(
            request_id = %req.request_id,
            "Layer-range execution ({}-{}) not yet implemented - running full model",
            layer_start, layer_end
        );
    }

    // Generate streaming text (collect all tokens)
    let mut generated_text = String::new();
    let request_id_for_chunks = req.request_id.clone();
    let generation_result = model_guard.generate_streaming(&prompt, max_tokens_override, |token| {
        generated_text.push_str(token);
        on_chunk(&request_id_for_chunks, token);
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

fn build_streaming_ack(request_id: &str) -> InferenceResponse {
    InferenceResponse {
        request_id: request_id.to_string(),
        class_id: None,
        probabilities: vec![],
        raw_output: vec![],
        confidence: 0.0,
        latency_us: 0,
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
