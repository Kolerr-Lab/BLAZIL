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

use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// After this many consecutive empty Aeron polls, call yield_now() once
/// to let tokio worker threads (TB async callbacks) and Aeron C driver
/// background threads get scheduled. Must be a power of two.
/// 512 spins ≈ ~1–2 µs on a 3 GHz core — enough to un-starve TB callbacks
/// without flooding the OS scheduler with unnecessary context switches.
const SPIN_BEFORE_YIELD: u32 = 512;

/// Timeout waiting for publication / subscription async registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of fragments processed per `poll_fragments` call.
/// Must be ≥ WINDOW_SIZE_TB (1024) so the serve loop can absorb the full
/// in-flight window in a single poll, allowing LedgerHandler to build a
/// maximum-size TigerBeetle batch (up to 8,190 transfers) per round trip.
const FRAGMENT_LIMIT: usize = 1024;

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
    /// Live count of in-flight (seq, req_id) pairs awaiting TB results.
    /// Written by the serve thread; read by bench/monitoring.
    pending_len: Arc<AtomicUsize>,
    /// Cumulative Aeron publication offer() failures (back-pressure spills).
    offer_failures: Arc<AtomicU64>,
    /// Sequence-based reorder buffer: one AtomicBool per ring-buffer slot.
    ///
    /// LedgerHandler sets `flags[seq % cap] = true` (Release) after writing
    /// the result to the DashMap. The drain checks this flag (Acquire) instead
    /// of polling the DashMap, giving O(1) cache-friendly ready detection.
    /// Size must equal the ring buffer capacity (power of 2).
    ready_flags: Option<Arc<Vec<AtomicBool>>>,
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
            pending_len: Arc::new(AtomicUsize::new(0)),
            offer_failures: Arc::new(AtomicU64::new(0)),
            ready_flags: None,
        }
    }

    /// Attach a sequence reorder buffer.
    ///
    /// When set, the drain path uses O(1) AtomicBool flag-checks instead of
    /// DashMap polling, and VecDeque ordering is restored (in-order drain).
    /// Must be the same `Arc<Vec<AtomicBool>>` passed to `LedgerHandler::with_reorder_flags`.
    pub fn with_reorder_flags(mut self, flags: Arc<Vec<AtomicBool>>) -> Self {
        self.ready_flags = Some(flags);
        self
    }

    /// Current pending-window size (requests in-flight, awaiting TB results).
    pub fn pending_len(&self) -> &Arc<AtomicUsize> {
        &self.pending_len
    }

    /// Cumulative Aeron offer() failure count since start.
    pub fn offer_failures(&self) -> &Arc<AtomicU64> {
        &self.offer_failures
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
        let pending_len = Arc::clone(&self.pending_len);
        let offer_failures = Arc::clone(&self.offer_failures);
        let ready_flags = self.ready_flags.clone();

        tokio::task::spawn_blocking(move || {
            aeron_serve_blocking(
                channel,
                aeron_dir,
                pipeline,
                shutdown,
                pending_len,
                offer_failures,
                ready_flags,
            )
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
    pending_len_metric: Arc<AtomicUsize>,
    offer_failures_metric: Arc<AtomicU64>,
    ready_flags: Option<Arc<Vec<AtomicBool>>>,
) -> BlazerResult<()> {
    // ── 1. Embedded C Media Driver ────────────────────────────────────────────
    let driver = EmbeddedAeronDriver::new(Some(&aeron_dir));
    driver.start()?;

    // ── Core affinity: pin serve thread to core 0 (Linux only) ───────────────
    //
    // WHY RE-ENABLED: previously removed because TB async callbacks run on
    // tokio worker threads that shared core 0, causing starvation.
    //
    // NOW SAFE: the bench `ledger_rt` tokio runtime is explicitly built with
    // `on_thread_start` pinning its workers to cores 2/3. The pipeline runner
    // is already on core 1+. So core 0 is exclusively the Aeron poll thread:
    //
    //   Core 0         — Aeron serve thread (this)
    //   Core 1         — Pipeline runner (LedgerHandler batch accumulator)
    //   Cores 2..3     — `ledger_rt` workers (TB async callbacks)
    //   Cores 4..N-1   — `#[tokio::main]` workers (bench coordination)
    //
    // Result: zero cache-line contention and no scheduler interference on the
    // hot path. The Aeron poll loop stays on its P-core at full clock speed.
    #[cfg(target_os = "linux")]
    {
        if let Some(core_ids) = core_affinity::get_core_ids() {
            if let Some(id) = core_ids.first() {
                core_affinity::set_for_current(*id);
                info!("Aeron serve thread pinned to core {}", id.id);
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
    // u64 is 8 bytes — Copy, stack-sized, zero heap allocation per slot.
    //
    // VecDeque maintains insertion order (ring-buffer submission order).
    // When ready_flags is Some, drain checks AtomicBool[seq%cap] instead of
    // polling the DashMap — O(1) cache-line read, Acquire semantics guarantee
    // the result is visible. When flags are not provided, falls back to direct
    // DashMap front-check (fine for in-order TB delivery).
    let mut pending: VecDeque<(i64, u64)> = VecDeque::with_capacity(PIPELINE_DEPTH);
    // Borrow the ready-flags slice once for the loop lifetime.
    let flags_ref: Option<&[AtomicBool]> = ready_flags.as_deref().map(|v| v.as_slice());
    // Idle spin counter for the adaptive yield strategy.
    let mut idle_spins: u32 = 0;
    // Periodic serve-thread diagnostics (always visible; bench sets log=ERROR).
    let mut last_diag = Instant::now();

    while !shutdown.load(Ordering::Acquire) {
        // ── Phase 1: drain completed results (non-blocking, bounded) ─────
        drain_ready_results(
            &mut pending,
            pipeline.results(),
            flags_ref,
            &pub_,
            MAX_DRAIN_PER_CALL,
            &offer_failures_metric,
        );
        pending_len_metric.store(pending.len(), Ordering::Relaxed);

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
            // Use MAX_DRAIN_PER_CALL per iteration to keep the spin tight;
            // remaining results are picked up in the next outer-loop Phase 1.
            while !pipeline.ring_buffer().has_available_capacity() {
                let n = drain_ready_results(
                    &mut pending,
                    pipeline.results(),
                    flags_ref,
                    &pub_,
                    MAX_DRAIN_PER_CALL,
                    &offer_failures_metric,
                );
                if n == 0 {
                    // Nothing drained — TB batch not yet complete. Spin lightly.
                    std::hint::spin_loop();
                }
            }

            // Single producer guarantee: capacity verified above — cannot
            // return RingBufferFull here.
            match pipeline.publish_event(event) {
                Ok(seq) => pending.push_back((seq, req_id_u64)),
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

        if count == 0 {
            // Adaptive idle: spin_loop for SPIN_BEFORE_YIELD iters, then yield.
            idle_spins = idle_spins.wrapping_add(1);
            if idle_spins & (SPIN_BEFORE_YIELD - 1) == 0 {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        } else {
            idle_spins = 0;
        }

        // Serve-thread diagnostics every 5 s — always visible regardless of
        // tracing log level (bench binary sets ERROR, suppressing warn!/info!).
        if last_diag.elapsed().as_secs() >= 5 {
            let pending_n = pending.len();
            let offer_fail = offer_failures_metric.load(Ordering::Relaxed);
            println!("[serve-diag] pending={pending_n} offer_fail={offer_fail}");
            last_diag = Instant::now();
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

/// Maximum results drained per `drain_ready_results` call.
/// Prevents the serve thread stalling > 512 * reply_overhead per iteration.
const MAX_DRAIN_PER_CALL: usize = 512;

/// Drains completed results from `pending` and sends Aeron reply messages.
///
/// # Two modes
///
/// ## Reorder-buffer mode (`ready_flags` = Some)
///
/// Uses a VecDeque (insertion-ordered) + AtomicBool flag array.
///
/// LedgerHandler stores each result in the DashMap then does:
///   `flags[seq % cap].store(true, Release)`
///
/// Here we check:
///   `flags[front_seq % cap].load(Acquire)` — O(1), cache-line aligned.
///
/// The Acquire/Release pair guarantees the DashMap insert is visible.
/// If the front is not flagged, we break immediately (head-of-line wait).
/// When the flag IS set, Result is guaranteed present — no DashMap miss.
///
/// ## Fallback mode (`ready_flags` = None)
///
/// Plain DashMap front-check on VecDeque front. Correct for real TB
/// (in-order single-TCP delivery). May stall on out-of-order completions.
#[inline(always)]
fn drain_ready_results(
    pending: &mut VecDeque<(i64, u64)>,
    results: &Arc<DashMap<i64, TransactionResult>>,
    ready_flags: Option<&[AtomicBool]>,
    pub_: &AeronPublication,
    max_drain: usize,
    offer_failures: &AtomicU64,
) -> usize {
    let mut drained = 0;

    if let Some(flags) = ready_flags {
        // Reorder-buffer path: O(1) per entry via AtomicBool Acquire check.
        let mask = flags.len() - 1; // flags.len() is power of 2
        while drained < max_drain {
            let &(seq, req_id_u64) = match pending.front() {
                Some(v) => v,
                None => break,
            };
            let idx = (seq as usize) & mask;
            // Acquire: all writes done before the Release store are visible.
            if !flags[idx].load(Ordering::Acquire) {
                break; // front result not yet written -- head-of-line wait
            }
            pending.pop_front();
            // Clear flag for slot reuse. Relaxed: no subsequent dependent read.
            flags[idx].store(false, Ordering::Relaxed);
            if let Some((_, result)) = results.remove(&seq) {
                let req_id_str = TransactionId::from_u64(req_id_u64).to_string();
                let resp = build_response(&req_id_str, result);
                if let Ok(bytes) = serialize_response(&resp) {
                    if pub_.offer(&bytes).is_err() {
                        offer_failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            drained += 1;
        }
    } else {
        // Fallback: direct DashMap check on front entry.
        while drained < max_drain {
            let &(seq, req_id_u64) = match pending.front() {
                Some(v) => v,
                None => break,
            };
            if let Some((_, result)) = results.remove(&seq) {
                pending.pop_front();
                let req_id_str = TransactionId::from_u64(req_id_u64).to_string();
                let resp = build_response(&req_id_str, result);
                if let Ok(bytes) = serialize_response(&resp) {
                    if pub_.offer(&bytes).is_err() {
                        offer_failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
                drained += 1;
            } else {
                break;
            }
        }
    }
    drained
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
