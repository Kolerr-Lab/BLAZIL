//! Aeron UDP transport server — embedded C Media Driver edition.
//!
//! Compiled only when the `aeron` crate feature is enabled.
//!
//! ## Architecture
//!
//! ```text
//! AeronTransportServer::serve()
//!    │  tokio::task::spawn_blocking
//!    ▼
//! aeron_serve_blocking()                      (dedicated OS thread)
//!    │  EmbeddedAeronDriver::start()          ← in-process C driver
//!    │  AeronContext::new(aeron_dir)           ← Aeron client
//!    │  AeronSubscription::new(ch, 1001)       ← inbound requests
//!    │  AeronPublication::new(ch, 1002)        → outbound responses
//!    │
//!    ▼  poll loop
//! subscription.poll_fragments()
//!    │  for each raw fragment:
//!    │    deserialize MessagePack → TransactionRequest
//!    │    build TransactionEvent → Pipeline::publish_event
//!    │    spin-wait for TransactionResult (≤ 100 ms)
//!    │    serialize MessagePack → TransactionResponse
//!    ▼
//! publication.offer(response_bytes)
//! ```
//!
//! ## Drop ordering (critical for C safety)
//!
//! 1. `AeronPublication` + `AeronSubscription` (close streams with driver)
//! 2. `AeronContext` (`aeron_close`)
//! 3. `EmbeddedAeronDriver` (driver main-loop exits, `aeron_driver_close`)
//!
//! The [`aeron_serve_blocking`] function enforces this ordering via explicit
//! `drop` calls before the driver is dropped.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use rust_decimal::Decimal;
use tracing::{error, info, warn};

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{TransactionEvent, TransactionResult};
use blazil_engine::pipeline::Pipeline;
use blazil_ledger::convert::amount_to_minor_units;

use crate::protocol::{
    deserialize_request, serialize_response, TransactionRequest, TransactionResponse,
};
use crate::server::TransportServer;

use super::context::AeronContext;
use super::driver::EmbeddedAeronDriver;
use super::publication::AeronPublication;
use super::subscription::AeronSubscription;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default Aeron channel URI for the Blazil engine.
pub const DEFAULT_AERON_CHANNEL: &str = "aeron:udp?endpoint=0.0.0.0:20121";

/// Stream ID for inbound client→server transaction requests.
pub const REQ_STREAM_ID: i32 = 1001;

/// Stream ID for outbound server→client transaction responses.
pub const RSP_STREAM_ID: i32 = 1002;

/// Pre-allocated capacity of the in-flight sliding window.
/// Must be ≥ WINDOW_SIZE_TB in the bench client. The Vec grows beyond this
/// only if TB is exceptionally slow — not on the hot path.
const PIPELINE_DEPTH: usize = 2048;

/// Timeout waiting for publication / subscription async registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of fragments processed per `poll_fragments` call.
/// Matches WINDOW_SIZE_TB=256 so the serve loop can absorb a full bench
/// window in one shot, allowing LedgerHandler to build a maximum-size
/// TigerBeetle batch (up to 8,190 transfers) per round trip.
const FRAGMENT_LIMIT: usize = 256;

// ── AeronTransportServer ──────────────────────────────────────────────────────

/// Aeron UDP transport server using an embedded C Media Driver.
///
/// Subscribes to [`REQ_STREAM_ID`] on the configured channel,
/// processes each request through the engine pipeline, and publishes
/// responses on [`RSP_STREAM_ID`].
///
/// The in-process C Media Driver is started automatically by [`serve`];
/// no external `aeronmd` binary is required.
///
/// [`serve`]: AeronTransportServer::serve
pub struct AeronTransportServer {
    /// Aeron channel URI, e.g. `"aeron:udp?endpoint=0.0.0.0:20121"`.
    channel: String,
    /// Path to the Aeron IPC shared-memory directory.
    aeron_dir: String,
    pipeline: Arc<Pipeline>,
    shutdown: Arc<AtomicBool>,
}

impl AeronTransportServer {
    /// Creates a new `AeronTransportServer`.
    ///
    /// - `channel`   — Aeron channel URI (see [`DEFAULT_AERON_CHANNEL`]).
    /// - `aeron_dir` — IPC directory for the embedded C Media Driver.
    /// - `pipeline`  — shared engine pipeline.
    pub fn new(channel: &str, aeron_dir: &str, pipeline: Arc<Pipeline>) -> Self {
        Self {
            channel: channel.to_owned(),
            aeron_dir: aeron_dir.to_owned(),
            pipeline,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl TransportServer for AeronTransportServer {
    /// Start the Aeron UDP transport.
    ///
    /// Runs the Aeron poll loop inside [`tokio::task::spawn_blocking`] so that
    /// the synchronous C Media Driver does not stall the async executor.
    async fn serve(&self) -> BlazerResult<()> {
        let channel = self.channel.clone();
        let aeron_dir = self.aeron_dir.clone();
        let pipeline = Arc::clone(&self.pipeline);
        let shutdown = Arc::clone(&self.shutdown);

        tokio::task::spawn_blocking(move || {
            aeron_serve_blocking(channel, aeron_dir, pipeline, shutdown)
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

/// Entry point for the dedicated blocking Aeron OS thread.
///
/// Starts the embedded C Media Driver, connects the client, creates the
/// subscription / publication pair, then polls in a tight loop until the
/// shutdown flag is set.  Resources are dropped in the correct order before
/// returning.
fn aeron_serve_blocking(
    channel: String,
    aeron_dir: String,
    pipeline: Arc<Pipeline>,
    shutdown: Arc<AtomicBool>,
) -> BlazerResult<()> {
    // ── 1. Embedded C Media Driver ────────────────────────────────────────────
    let driver = EmbeddedAeronDriver::new(Some(&aeron_dir));
    driver.start()?;

    // ── 1b. Core affinity — pin serve thread to core 0 (Linux only) ─────────
    // Core 0 is the network-transport core on DO nodes. Pipeline workers start
    // at core 1+. Pinning prevents the OS scheduler from migrating this tight
    // spin loop between cores, which would flush L1/L2 caches and add ~µs jitter.
    #[cfg(target_os = "linux")]
    {
        if let Some(core_ids) = core_affinity::get_core_ids() {
            if let Some(id) = core_ids.first() {
                core_affinity::set_for_current(*id);
                info!(core = id.id, "Aeron serve thread pinned to core");
            }
        }
    }

    // ── 2. Aeron client context ───────────────────────────────────────────────
    let ctx = AeronContext::new(&aeron_dir)?;

    // ── 3. Subscription (inbound requests, stream 1001) ───────────────────────
    let sub = AeronSubscription::new(&ctx, &channel, REQ_STREAM_ID, REGISTRATION_TIMEOUT)?;

    // ── 4. Publication (outbound responses, stream 1002) ─────────────────────
    let pub_ = AeronPublication::new(&ctx, &channel, RSP_STREAM_ID, REGISTRATION_TIMEOUT)?;

    info!(
        channel = %channel,
        req_stream = REQ_STREAM_ID,
        rsp_stream = RSP_STREAM_ID,
        aeron_dir = %aeron_dir,
        "Aeron UDP transport active (embedded C driver)"
    );

    // ── 5. Poll loop (async-pipeline) ────────────────────────────────────────
    //
    // Every iteration runs two non-blocking phases:
    //
    // Phase 1 — Drain: scan the in-flight window for completed TB results;
    //   reply immediately for each one found. Uses O(1) swap_remove so that
    //   replies are sent in completion order, not submission order (fine —
    //   each TransactionResponse carries its own request_id).
    //
    // Phase 2 — Pump: poll Aeron for new fragments (up to FRAGMENT_LIMIT)
    //   and publish each to the ring buffer. If the ring buffer is full,
    //   busy-spin while continuing Phase 1 — this drains results and frees
    //   ring-buffer slots without ever yielding to the OS scheduler.
    //
    // The critical difference from the previous design: there is no blocking
    // Phase B.  The serve thread never stalls waiting for TB. LedgerHandler
    // accumulates a full batch per TB round trip independently on the pipeline
    // runner thread; the serve thread keeps pumping new events in parallel,
    // keeping the ring buffer and the TB batch pipeline continuously full.
    //
    // Pre-allocate all hot-path buffers once outside the loop.
    // The outer Vec<Vec<u8>> is reused every iteration (clear() does not
    // free the outer allocation). Inner Vec<u8> per fragment are allocated
    // by the C trampoline in AeronSubscription; unavoidable with current ABI.
    let mut frags: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);
    // Sliding window: (ring-buffer seq, request_id as u64) per in-flight event.
    // u64 is 8 bytes — Copy, stack-sized, zero heap allocation for the window.
    // swap_remove gives O(1) removal; reply order does not matter since every
    // TransactionResponse carries its own request_id.
    let mut pending: Vec<(i64, u64)> = Vec::with_capacity(PIPELINE_DEPTH);

    while !shutdown.load(Ordering::Acquire) {
        // ── Phase 1: drain completed results (non-blocking) ───────────────
        drain_ready_results(&mut pending, pipeline.results(), &pub_);

        // ── Phase 2: poll Aeron → publish to ring buffer ──────────────────
        frags.clear();
        let count = sub.poll_fragments(&mut frags, FRAGMENT_LIMIT);

        for payload in &frags {
            let request = match deserialize_request(payload) {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "Aeron: malformed fragment");
                    let resp = error_response("", &BlazerError::Transport(e.to_string()));
                    if let Ok(bytes) = serialize_response(&resp) {
                        let _ = pub_.offer(&bytes);
                    }
                    continue;
                }
            };

            // Parse request_id to stack-allocated u64 — no heap.
            // TransactionId wraps u64 and is Copy. The String from the wire
            // is consumed here; nothing is cloned or heap-retained.
            let req_id_u64: u64 = TransactionId::from_str(&request.request_id)
                .unwrap_or_else(|_| TransactionId::new())
                .as_u64();

            let event = match build_event(request) {
                Ok(e) => e,
                Err(e) => {
                    // Reconstruct string only at error-reply time, not in the
                    // hot path.
                    let req_id_str = TransactionId::from_u64(req_id_u64).to_string();
                    warn!(request_id = %req_id_str, error = %e, "Aeron: event build failed");
                    let resp = error_response(&req_id_str, &e);
                    if let Ok(bytes) = serialize_response(&resp) {
                        let _ = pub_.offer(&bytes);
                    }
                    continue;
                }
            };

            // Busy-spin on ring-buffer backpressure — never yield to OS.
            // Drain Phase 1 inside the spin: frees ring-buffer slots as TB
            // batches complete, so we unblock as fast as physically possible.
            while !pipeline.ring_buffer().has_available_capacity() {
                drain_ready_results(&mut pending, pipeline.results(), &pub_);
                std::hint::spin_loop();
            }

            // Single producer guarantee: capacity verified above — cannot
            // return RingBufferFull here.
            match pipeline.publish_event(event) {
                Ok(seq) => pending.push((seq, req_id_u64)),
                Err(e) => {
                    let req_id_str = TransactionId::from_u64(req_id_u64).to_string();
                    error!(request_id = %req_id_str, error = %e, "Aeron: publish_event failed");
                    let resp = error_response(&req_id_str, &e);
                    if let Ok(bytes) = serialize_response(&resp) {
                        let _ = pub_.offer(&bytes);
                    }
                }
            }
        }

        if count == 0 && pending.is_empty() {
            std::hint::spin_loop();
        }
    }

    info!("Aeron poll loop exited cleanly");

    // ── 6. Ordered teardown ───────────────────────────────────────────────────
    // DROP ORDER IS CRITICAL — streams before client, client before driver.
    drop(pub_); // aeron_publication_close
    drop(sub); // aeron_subscription_close
    drop(ctx); // aeron_close  → aeron_context_close
    drop(driver); // driver thread exits → aeron_driver_close → context_close

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Drains completed TB results from `pending` and sends replies.
///
/// Scans the full `pending` slice each call; uses `swap_remove` for O(1)
/// removal (last element fills the gap — reply order does not matter since
/// every response carries its own `request_id`).
///
/// Called both in the main serve loop and inside the ring-buffer-full
/// backpressure spin so that TB batch completions are always forwarded to
/// the client even when the ring buffer is temporarily saturated.
#[inline(always)]
fn drain_ready_results(
    pending: &mut Vec<(i64, u64)>,
    results: &Arc<DashMap<i64, TransactionResult>>,
    pub_: &AeronPublication,
) {
    let mut i = 0;
    while i < pending.len() {
        let seq = pending[i].0;
        if let Some((_, result)) = results.remove(&seq) {
            // O(1): swap last element into slot i, decrement len.
            let (_, req_id_u64) = pending.swap_remove(i);
            // String allocation happens only here, at reply time — not during
            // the in-flight window. The u64 lived entirely on the stack.
            let req_id_str = TransactionId::from_u64(req_id_u64).to_string();
            let resp = build_response(&req_id_str, result);
            if let Ok(bytes) = serialize_response(&resp) {
                // AeronPublication::offer already spins on back-pressure.
                let _ = pub_.offer(&bytes);
            }
            // Do NOT increment i — the new occupant of slot i needs checking.
        } else {
            i += 1;
        }
    }
}

/// Parses a [`TransactionRequest`] into a [`TransactionEvent`].
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
    let amount_units = amount_to_minor_units(&amount)? as u64;
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
        amount_units,
        ledger_id,
        req.code,
    ))
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
fn error_response(request_id: &str, err: &BlazerError) -> TransactionResponse {
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
