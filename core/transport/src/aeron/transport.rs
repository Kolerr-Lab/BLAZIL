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

/// How long to spin-wait for a pipeline result before returning a timeout response.
const RESULT_TIMEOUT: Duration = Duration::from_millis(100);

/// Timeout waiting for publication / subscription async registration.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of fragments processed per `poll_fragments` call.
const FRAGMENT_LIMIT: usize = 10;

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

    // ── 5. Poll loop ──────────────────────────────────────────────────────────
    let mut frags: Vec<Vec<u8>> = Vec::with_capacity(FRAGMENT_LIMIT);

    while !shutdown.load(Ordering::Acquire) {
        frags.clear();
        let count = sub.poll_fragments(&mut frags, FRAGMENT_LIMIT);

        for payload in &frags {
            let response = handle_fragment(payload, &pipeline);

            match serialize_response(&response) {
                Ok(bytes) => {
                    // Ignore back-pressure errors in the poll loop — the client
                    // will eventually retry the request.
                    if let Err(e) = pub_.offer(&bytes) {
                        warn!(error = %e, "Aeron offer failed");
                    }
                }
                Err(e) => error!(error = %e, "response serialisation failed"),
            }
        }

        if count == 0 {
            // Nothing received this iteration — yield the CPU briefly.
            std::hint::spin_loop();
        }
    }

    info!("Aeron poll loop exited cleanly");

    // ── 6. Ordered teardown ───────────────────────────────────────────────────
    // DROP ORDER IS CRITICAL — streams before client, client before driver.
    drop(pub_);    // aeron_publication_close
    drop(sub);     // aeron_subscription_close
    drop(ctx);     // aeron_close  → aeron_context_close
    drop(driver);  // driver thread exits → aeron_driver_close → context_close

    Ok(())
}

// ── Per-fragment request processing ──────────────────────────────────────────

/// Deserializes one Aeron fragment, drives it through the engine pipeline,
/// and returns a [`TransactionResponse`] ready to publish.
fn handle_fragment(payload: &[u8], pipeline: &Arc<Pipeline>) -> TransactionResponse {
    // ── Deserialize ───────────────────────────────────────────────────────────
    let request = match deserialize_request(payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Aeron: malformed fragment — rejected");
            return error_response("", &BlazerError::Transport(e.to_string()));
        }
    };

    let request_id = request.request_id.clone();

    // ── Build event ───────────────────────────────────────────────────────────
    let event = match build_event(request) {
        Ok(e) => e,
        Err(e) => {
            warn!(%request_id, error = %e, "Aeron: event build failed");
            return error_response(&request_id, &e);
        }
    };

    // ── Publish to pipeline ───────────────────────────────────────────────────
    let seq = match pipeline.publish_event(event) {
        Ok(s) => s,
        Err(e) => {
            error!(%request_id, error = %e, "Aeron: publish_event failed");
            return error_response(&request_id, &e);
        }
    };

    // ── Wait for result ───────────────────────────────────────────────────────
    let results = pipeline.results();
    match wait_for_result(results, seq) {
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

/// Synchronous spin-wait for a pipeline result — safe from the blocking thread.
///
/// Spins with a 100 µs sleep between polls until either a result is written by
/// the engine or [`RESULT_TIMEOUT`] elapses.
fn wait_for_result(
    results: &Arc<DashMap<i64, TransactionResult>>,
    seq: i64,
) -> Option<TransactionResult> {
    let deadline = std::time::Instant::now() + RESULT_TIMEOUT;
    loop {
        if let Some(r) = results.get(&seq) {
            return Some(r.value().clone());
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
