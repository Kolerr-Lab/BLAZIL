//! Aeron UDP transport server.
//!
//! Compiled only when the `aeron` crate feature is enabled.
//! Selected at runtime via `BLAZIL_TRANSPORT=aeron`.
//!
//! ## Architecture
//!
//! ```text
//! Aeron C Media Driver  (/dev/shm/aeron  shared memory)
//!    │
//!    ▼  IPC
//! AeronTransportServer::serve()
//!    │  tokio::task::spawn_blocking
//!    ▼
//! aeron_serve_blocking()                     (dedicated OS thread)
//!    │  subscribe  channel / stream 1001     ← inbound requests
//!    │  publication channel / stream 1002    → outbound responses
//!    │
//!    ▼  per Aeron fragment
//! handle_fragment()
//!    │  deserialize MessagePack → TransactionRequest
//!    │  build TransactionEvent
//!    │  Pipeline::publish_event(event) → seq
//!    │  spin-wait for TransactionResult (up to 100 ms)
//!    │  serialize MessagePack → TransactionResponse
//!    ▼
//! publication.offer_part(response_buffer)
//! ```
//!
//! ## Env vars
//!
//! | Variable | Default | Purpose |
//! |---|---|---|
//! | `AERON_DIR` | `/dev/shm/aeron` | IPC directory shared with the media driver |
//! | `BLAZIL_AERON_CHANNEL` | `aeron:udp?endpoint=0.0.0.0:20121` | Aeron channel URI |

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{error, info, warn};

use aeron_rs::aeron::Aeron;
use aeron_rs::concurrent::atomic_buffer::{AlignedBuffer, AtomicBuffer};
use aeron_rs::context::Context;

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{TransactionEvent, TransactionResult};
use blazil_engine::pipeline::Pipeline;
use blazil_engine::ring_buffer::RingBuffer;

use crate::protocol::{deserialize_request, serialize_response, TransactionRequest, TransactionResponse};
use crate::server::TransportServer;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default Aeron channel URI used by the Blazil engine.
pub const DEFAULT_AERON_CHANNEL: &str = "aeron:udp?endpoint=0.0.0.0:20121";

/// Stream ID for inbound client→server transaction requests.
pub const REQ_STREAM_ID: i32 = 1001;

/// Stream ID for outbound server→client transaction responses.
pub const RSP_STREAM_ID: i32 = 1002;

/// How long to spin-wait for a pipeline result before returning a timeout response.
const RESULT_TIMEOUT: Duration = Duration::from_millis(100);

/// Response buffer size: 64 KiB — sufficient for all MessagePack frames.
const RSP_BUF_CAPACITY: i32 = 65_536;

// ── AeronTransportServer ──────────────────────────────────────────────────────

/// Aeron UDP transport server.
///
/// Subscribes to [`REQ_STREAM_ID`] on the configured channel,
/// processes each request through the engine pipeline, and publishes
/// responses on [`RSP_STREAM_ID`].
///
/// Requires an Aeron C Media Driver to be running — see the `aeron-driver`
/// service in the node docker-compose files.
pub struct AeronTransportServer {
    /// Aeron channel URI, e.g. `"aeron:udp?endpoint=0.0.0.0:20121"`.
    channel: String,
    /// Path to the Aeron IPC directory, e.g. `"/dev/shm/aeron"`.
    aeron_dir: String,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    shutdown: Arc<AtomicBool>,
}

impl AeronTransportServer {
    /// Creates a new `AeronTransportServer`.
    ///
    /// - `channel`   — Aeron channel URI.
    /// - `aeron_dir` — IPC directory shared with the Aeron C Media Driver.
    /// - `pipeline`  — shared engine pipeline.
    /// - `ring_buffer` — shared ring buffer.
    pub fn new(
        channel: &str,
        aeron_dir: &str,
        pipeline: Arc<Pipeline>,
        ring_buffer: Arc<RingBuffer>,
    ) -> Self {
        Self {
            channel: channel.to_owned(),
            aeron_dir: aeron_dir.to_owned(),
            pipeline,
            ring_buffer,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl TransportServer for AeronTransportServer {
    /// Start the Aeron UDP transport.
    ///
    /// Runs the Aeron poll loop inside [`tokio::task::spawn_blocking`] so the
    /// synchronous Aeron C client does not stall the async executor.
    async fn serve(&self) -> BlazerResult<()> {
        let channel = self.channel.clone();
        let aeron_dir = self.aeron_dir.clone();
        let pipeline = Arc::clone(&self.pipeline);
        let ring_buffer = Arc::clone(&self.ring_buffer);
        let shutdown = Arc::clone(&self.shutdown);

        tokio::task::spawn_blocking(move || {
            aeron_serve_blocking(channel, aeron_dir, pipeline, ring_buffer, shutdown)
        })
        .await
        .map_err(|e| BlazerError::Transport(format!("Aeron blocking task panicked: {e}")))?
    }

    async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        info!("Aeron transport: shutdown requested");
    }

    fn local_addr(&self) -> &str {
        &self.channel
    }
}

// ── Blocking Aeron loop ───────────────────────────────────────────────────────

/// Entry point for the dedicated blocking Aeron thread.
///
/// Creates the Aeron client, waits for subscription and publication to
/// connect via the running C Media Driver, then polls in a tight loop
/// until the shutdown flag is set.
///
/// All Aeron objects are created and used exclusively on this thread.
/// `Arc<Mutex<>>` handles inter-thread access to subscription/publication
/// if aeron-rs uses that internally.
fn aeron_serve_blocking(
    channel: String,
    aeron_dir: String,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    shutdown: Arc<AtomicBool>,
) -> BlazerResult<()> {
    use std::ffi::CString;

    // ── Aeron client ──────────────────────────────────────────────────────────
    let mut ctx = Context::new();
    ctx.set_aeron_dir(aeron_dir);

    // Aeron::new connects to the running C Media Driver over the IPC dir.
    // No separate .start() call is needed in aeron-rs 0.1.x.
    let mut aeron = Aeron::new(ctx)
        .map_err(|e| BlazerError::Transport(format!("Aeron init failed: {e}")))?;

    let channel_cstr = CString::new(channel.clone())
        .map_err(|e| BlazerError::Transport(format!("invalid channel string: {e}")))?;

    // ── Subscription (inbound requests, stream 1001) ───────────────────────────
    let sub_id = aeron
        .add_subscription(channel_cstr.clone(), REQ_STREAM_ID)
        .map_err(|e| BlazerError::Transport(format!("Aeron add_subscription failed: {e}")))?;

    // find_subscription returns Ok(sub) once the media driver confirms the
    // registration.  It returns Err while pending — retry until ready.
    let subscription = loop {
        if shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        match aeron.find_subscription(sub_id) {
            Ok(s) => break s,
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    // ── Publication (outbound responses, stream 1002) ─────────────────────────
    let pub_id = aeron
        .add_publication(channel_cstr, RSP_STREAM_ID)
        .map_err(|e| BlazerError::Transport(format!("Aeron add_publication failed: {e}")))?;

    let publication = loop {
        if shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        match aeron.find_publication(pub_id) {
            Ok(p) => break p,
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    info!(
        channel = %channel,
        req_stream = REQ_STREAM_ID,
        rsp_stream = RSP_STREAM_ID,
        "🚀 Aeron UDP transport active"
    );

    // ── Response buffer (reused across fragments) ─────────────────────────────
    let mut rsp_aligned = AlignedBuffer::with_capacity(RSP_BUF_CAPACITY);
    let rsp_buf = AtomicBuffer::from_aligned(&mut rsp_aligned);

    // ── Poll loop ─────────────────────────────────────────────────────────────
    while !shutdown.load(Ordering::Acquire) {
        let fragments = subscription
            .lock()
            .expect("subscription mutex poisoned")
            .poll(
                &mut |buffer: &AtomicBuffer, offset: i32, length: i32, _header| {
                    // Safety: buffer and offset/length are validated by the
                    // Aeron media driver before this callback is invoked.
                    let payload = unsafe {
                        std::slice::from_raw_parts(
                            buffer.buffer().add(offset as usize),
                            length as usize,
                        )
                    };

                    let response = handle_fragment(payload, &pipeline, &ring_buffer);

                    match serialize_response(&response) {
                        Ok(bytes) if bytes.len() <= RSP_BUF_CAPACITY as usize => {
                            // Write response into the pre-allocated outbound buffer.
                            // SAFETY: bytes.len() <= RSP_BUF_CAPACITY (checked above).
                            rsp_buf.put_bytes(0_i32, &bytes);
                            let result = publication
                                .lock()
                                .expect("publication mutex poisoned")
                                .offer_part(rsp_buf, 0, bytes.len() as i32);
                            if let Err(e) = result {
                                warn!(error = %e, "Aeron offer failed (back-pressure or not connected)");
                            }
                        }
                        Ok(bytes) => warn!(
                            len = bytes.len(),
                            max = RSP_BUF_CAPACITY,
                            "response exceeds Aeron buffer — dropped"
                        ),
                        Err(e) => error!(error = %e, "response serialisation failed"),
                    }
                },
                10, // fragment_limit: process up to 10 fragments per poll call
            );

        if fragments == 0 {
            // Nothing to read — yield CPU briefly to avoid busy-spinning.
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    info!("Aeron poll loop exited cleanly");
    Ok(())
}

// ── Per-fragment request processing ──────────────────────────────────────────

/// Deserializes one Aeron fragment, drives it through the engine pipeline,
/// and returns a [`TransactionResponse`] ready to publish.
fn handle_fragment(
    payload: &[u8],
    pipeline: &Arc<Pipeline>,
    ring_buffer: &Arc<RingBuffer>,
) -> TransactionResponse {
    // ── Deserialize ───────────────────────────────────────────────────────────
    let request = match deserialize_request(payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Aeron: malformed fragment — rejected");
            return aeron_error_response("", &BlazerError::Transport(e.to_string()));
        }
    };

    let request_id = request.request_id.clone();

    // ── Build event ───────────────────────────────────────────────────────────
    let event = match build_event(request) {
        Ok(e) => e,
        Err(e) => {
            warn!(%request_id, error = %e, "Aeron: event build failed");
            return aeron_error_response(&request_id, &e);
        }
    };

    // ── Publish to pipeline ───────────────────────────────────────────────────
    let seq = match pipeline.publish_event(event) {
        Ok(s) => s,
        Err(e) => {
            error!(%request_id, error = %e, "Aeron: publish_event failed");
            return aeron_error_response(&request_id, &e);
        }
    };

    // ── Wait for result ───────────────────────────────────────────────────────
    match wait_for_result_sync(ring_buffer, seq) {
        Some(result) => build_response(&request_id, result),
        None => TransactionResponse {
            request_id,
            committed: false,
            transfer_id: None,
            error: Some("processing timeout".into()),
            timestamp_ns: Timestamp::now().as_nanos(),
        },
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parses a [`TransactionRequest`] into a [`TransactionEvent`].
///
/// Mirrors `connection::build_event` but lives here to avoid making that
/// function `pub` and coupling the two modules.
fn build_event(req: TransactionRequest) -> BlazerResult<TransactionEvent> {
    let debit_account_id = AccountId::from_str(&req.debit_account_id).map_err(|_| {
        BlazerError::ValidationError(format!(
            "invalid debit_account_id: {}",
            req.debit_account_id
        ))
    })?;

    let credit_account_id = AccountId::from_str(&req.credit_account_id).map_err(|_| {
        BlazerError::ValidationError(format!(
            "invalid credit_account_id: {}",
            req.credit_account_id
        ))
    })?;

    let decimal = Decimal::from_str(&req.amount)
        .map_err(|_| BlazerError::ValidationError(format!("invalid amount: {}", req.amount)))?;

    let currency = parse_currency(&req.currency)?;
    let amount = Amount::new(decimal, currency)?;
    let ledger_id = LedgerId::new(req.ledger_id)?;

    let transaction_id = TransactionId::from_str(&req.request_id).unwrap_or_else(|_| {
        warn!(
            request_id = %req.request_id,
            "non-UUID request_id — generating new TransactionId"
        );
        TransactionId::new()
    });

    Ok(TransactionEvent::new(
        transaction_id,
        debit_account_id,
        credit_account_id,
        amount,
        ledger_id,
        req.code,
    ))
}

/// Synchronous ring-buffer spin-wait — safe to call from the blocking Aeron thread.
///
/// Spins with a 100 µs sleep between polls until either a result is written
/// or [`RESULT_TIMEOUT`] elapses.
fn wait_for_result_sync(ring_buffer: &Arc<RingBuffer>, seq: i64) -> Option<TransactionResult> {
    let deadline = std::time::Instant::now() + RESULT_TIMEOUT;
    loop {
        // SAFETY: Same invariants as connection::wait_for_result — the pipeline
        // runner writes `result` exactly once before advancing its cursor past
        // `seq`, and we only read after publish_event has returned the slot.
        let result = unsafe { &*ring_buffer.get(seq) }.result.clone();
        if let Some(r) = result {
            return Some(r);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_micros(100));
    }
}

/// Builds a committed or rejected [`TransactionResponse`] from a pipeline result.
fn build_response(request_id: &str, result: TransactionResult) -> TransactionResponse {
    let ts = Timestamp::now().as_nanos();
    match result {
        TransactionResult::Committed {
            transfer_id,
            timestamp: _,
        } => TransactionResponse {
            request_id: request_id.to_owned(),
            committed: true,
            transfer_id: Some(transfer_id.to_string()),
            error: None,
            timestamp_ns: ts,
        },
        TransactionResult::Rejected { reason } => TransactionResponse {
            request_id: request_id.to_owned(),
            committed: false,
            transfer_id: None,
            error: Some(reason.to_string()),
            timestamp_ns: ts,
        },
    }
}

/// Constructs an error [`TransactionResponse`] for a failed request.
fn aeron_error_response(request_id: &str, err: &BlazerError) -> TransactionResponse {
    let msg = match err {
        BlazerError::RingBufferFull { retry_after_ms } => {
            format!("server busy, retry after {}ms", retry_after_ms)
        }
        _ => err.to_string(),
    };
    TransactionResponse {
        request_id: request_id.to_owned(),
        committed: false,
        transfer_id: None,
        error: Some(msg),
        timestamp_ns: Timestamp::now().as_nanos(),
    }
}
