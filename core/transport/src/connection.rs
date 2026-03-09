//! Per-connection request handler.
//!
//! [`handle_connection`] drives the full lifecycle of a single TCP
//! connection:
//!
//! ```text
//! read_frame → deserialize → validate → publish_event
//!   → wait for result → build response → write_frame
//! ```
//!
//! The loop continues until the client disconnects (EOF) or an
//! unrecoverable I/O error occurs.
//!
//! # Result waiting strategy
//!
//! After calling [`Pipeline::publish_event`], the handler polls the ring
//! buffer slot for a [`TransactionResult`] using
//! [`tokio::task::yield_now`] in a tight async loop with a 100 ms
//! timeout. This keeps the connection task responsive without burning a
//! dedicated OS thread and without sleeping on the hot path.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, error, instrument, warn};

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{TransactionEvent, TransactionResult};
use blazil_engine::pipeline::Pipeline;
use blazil_engine::ring_buffer::RingBuffer;

use crate::protocol::{
    deserialize_request, serialize_response, Frame, TransactionRequest, TransactionResponse,
};

/// Result-wait timeout: how long to wait for the pipeline to process an event.
const RESULT_TIMEOUT: Duration = Duration::from_millis(100);

// ── handle_connection ─────────────────────────────────────────────────────────

/// Handles one client connection for its full lifetime.
///
/// Loops, reading one framed request per iteration and writing one framed
/// response. Exits cleanly on EOF or unrecoverable I/O error.
///
/// `active_connections` is decremented unconditionally before returning.
///
/// # Arguments
///
/// - `stream` — the accepted [`TcpStream`].
/// - `pipeline` — shared engine pipeline for publishing events.
/// - `ring_buffer` — shared ring buffer for reading results.
/// - `active_connections` — shared counter; decremented on exit.
///
/// # Errors
///
/// Returns [`BlazerError::Transport`] if the initial handshake fails. Most
/// per-request errors are handled internally (rejection response sent,
/// loop continues).
#[instrument(skip(stream, pipeline, ring_buffer, active_connections),
             fields(remote_addr))]
pub async fn handle_connection(
    mut stream: TcpStream,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    active_connections: Arc<std::sync::atomic::AtomicU64>,
) -> BlazerResult<()> {
    // Record remote address in tracing span (best-effort).
    if let Ok(addr) = stream.peer_addr() {
        tracing::Span::current().record("remote_addr", addr.to_string());
    }

    loop {
        // ── Step 1: Read frame ─────────────────────────────────────────────
        let frame = match Frame::read_frame(&mut stream).await {
            Ok(f) => f,
            Err(e) => {
                // Transport errors include EOF — treat as clean disconnect.
                debug!(error = %e, "connection closed (read_frame)");
                break;
            }
        };

        // ── Step 2: Deserialize request ────────────────────────────────────
        let request = match deserialize_request(&frame.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "malformed request — sending rejection");
                let resp = error_response("", &e);
                send_response(&mut stream, &resp).await;
                continue; // don't close — client may send a valid next request
            }
        };

        let request_id = request.request_id.clone();

        // ── Step 3: Convert request → TransactionEvent ────────────────────
        let event = match build_event(request) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    %request_id,
                    error = %e,
                    "request parse error — sending rejection"
                );
                let resp = error_response(&request_id, &e);
                send_response(&mut stream, &resp).await;
                continue;
            }
        };

        // ── Step 4: Publish to pipeline ────────────────────────────────────
        let seq = match pipeline.publish_event(event) {
            Ok(s) => s,
            Err(e) => {
                error!(%request_id, error = %e, "publish_event failed");
                let resp = error_response(&request_id, &e);
                send_response(&mut stream, &resp).await;
                continue;
            }
        };

        // ── Step 5: Wait for result (up to 100 ms) ────────────────────────
        let result = match wait_for_result(&ring_buffer, seq).await {
            Some(r) => r,
            None => {
                warn!(%request_id, "processing timeout — sending timeout response");
                let resp = TransactionResponse {
                    request_id: request_id.clone(),
                    committed: false,
                    transfer_id: None,
                    error: Some("processing timeout".into()),
                    timestamp_ns: Timestamp::now().as_nanos(),
                };
                send_response(&mut stream, &resp).await;
                continue;
            }
        };

        // ── Step 6: Build and send response ───────────────────────────────
        let response = build_response(&request_id, result);
        send_response(&mut stream, &response).await;
    }

    // Decrement connection counter unconditionally.
    active_connections.fetch_sub(1, std::sync::atomic::Ordering::Release);
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parses a [`TransactionRequest`] into a [`TransactionEvent`].
fn build_event(req: TransactionRequest) -> BlazerResult<TransactionEvent> {
    // Parse debit account ID.
    let debit_account_id = AccountId::from_str(&req.debit_account_id)
        .map_err(|_| BlazerError::ValidationError(
            format!("invalid debit_account_id: {}", req.debit_account_id)
        ))?;

    // Parse credit account ID.
    let credit_account_id = AccountId::from_str(&req.credit_account_id)
        .map_err(|_| BlazerError::ValidationError(
            format!("invalid credit_account_id: {}", req.credit_account_id)
        ))?;

    // Parse amount decimal.
    let decimal = Decimal::from_str(&req.amount)
        .map_err(|_| BlazerError::ValidationError(
            format!("invalid amount: {}", req.amount)
        ))?;

    // Parse currency.
    let currency = parse_currency(&req.currency)?;

    // Build Amount.
    let amount = Amount::new(decimal, currency)?;

    // Build LedgerId.
    let ledger_id = LedgerId::new(req.ledger_id)?;

    // Parse request_id as TransactionId; fall back to a new random ID.
    let transaction_id = TransactionId::from_str(&req.request_id)
        .unwrap_or_else(|_| {
            warn!(request_id = %req.request_id, "client sent non-UUID request_id — generating new TransactionId");
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

/// Polls the ring buffer slot at `seq` until a result appears or the
/// 100 ms deadline expires.
///
/// Uses [`tokio::task::yield_now`] to cooperatively yield between polls,
/// keeping the async executor responsive.
async fn wait_for_result(ring_buffer: &Arc<RingBuffer>, seq: i64) -> Option<TransactionResult> {
    let rb = Arc::clone(ring_buffer);
    let fut = async move {
        loop {
            // SAFETY: The producer wrote this slot before advancing the cursor
            // (Release store in `publish`). We only read after the cursor has
            // passed `seq` (via the `publish_event` return). The pipeline
            // runner writes `result` once and never mutates it again. Reading
            // here is safe because:
            //   1. We observe the slot's address via a shared Arc.
            //   2. The runner's write to `result` happens-before our read due
            //      to tokio's cooperative scheduling and the atomic cursor.
            let result = unsafe { &*rb.get(seq) }.result.clone();
            if let Some(r) = result {
                return r;
            }
            // Yield back to the tokio runtime so other tasks can progress.
            tokio::task::yield_now().await;
        }
    };

    timeout(RESULT_TIMEOUT, fut).await.ok()
}

/// Builds a [`TransactionResponse`] from a [`TransactionResult`].
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

/// Constructs an error rejection response.
fn error_response(request_id: &str, err: &BlazerError) -> TransactionResponse {
    TransactionResponse {
        request_id: request_id.to_owned(),
        committed: false,
        transfer_id: None,
        error: Some(err.to_string()),
        timestamp_ns: Timestamp::now().as_nanos(),
    }
}

/// Serializes `response` and writes it as a framed message.
///
/// Errors are logged and swallowed — if we can't write a response, the
/// connection will be closed naturally on the next read attempt.
async fn send_response(stream: &mut TcpStream, response: &TransactionResponse) {
    match serialize_response(response) {
        Ok(bytes) => {
            if let Err(e) = Frame::write_frame(stream, &bytes).await {
                warn!(error = %e, "failed to write response frame");
            }
        }
        Err(e) => {
            error!(error = %e, "failed to serialize response");
        }
    }
}
