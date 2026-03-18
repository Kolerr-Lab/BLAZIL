//! io_uring TCP transport server (Linux 5.1+ only).
//!
//! Compiled and used only when:
//!   - `target_os = "linux"`
//!   - feature `io-uring` is enabled
//!
//! Selected at runtime via `BLAZIL_TRANSPORT=io-uring` or
//! `BLAZIL_TRANSPORT=aeron+io-uring`.
//!
//! ## Why io_uring?
//!
//! Standard tokio TCP: each `read()`/`write()` triggers a syscall,
//! causing a context switch between userspace and kernel.
//!
//! io_uring: I/O operations are submitted as entries to a shared-memory
//! submission queue.  The kernel drains the queue asynchronously and posts
//! completions to a completion queue — **zero extra context switches** on
//! the hot path.  A single `io_uring_enter` syscall can submit and/or
//! reap many operations at once, amortising the syscall cost across many
//! in-flight requests.
//!
//! ## Architecture
//!
//! ```text
//! tokio_uring runtime (wraps tokio + io_uring SQ/CQ)
//!    │
//!    ▼  tokio_uring::net::TcpListener
//! IoUringTransportServer::serve()
//!    │  accept loop — spawn task per connection
//!    ▼
//! handle_uring_connection()
//!    │  read_frame_uring → deserialize → build TransactionEvent
//!    ▼
//! Pipeline::publish_event → wait for TransactionResult
//!    ▼
//! TransactionResponse → write_frame_uring → client
//! ```

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use rust_decimal::Decimal;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{TransactionEvent, TransactionResult};
use blazil_engine::pipeline::Pipeline;
use blazil_engine::ring_buffer::RingBuffer;
use blazil_ledger::convert::amount_to_minor_units;

use crate::protocol::{
    deserialize_request, serialize_response, TransactionRequest, TransactionResponse,
    MAX_FRAME_SIZE,
};
use crate::rate_limit::TokenBucket;
use crate::server::TransportServer;

// ── Result-wait timeout ───────────────────────────────────────────────────────

const RESULT_TIMEOUT: Duration = Duration::from_millis(100);

// ── Rate limiting ─────────────────────────────────────────────────────────────

const RATE_LIMIT_TPS: u64 = 55_000; // Max 55K TPS to prevent OOM
const RATE_LIMIT_BURST: u64 = 1_000; // Allow 1-second burst headroom

// ── IoUringTransportServer ────────────────────────────────────────────────────

/// io_uring-backed TCP transport server with lock-free rate limiting.
///
/// Uses `tokio-uring` to drive all accept/read/write operations through
/// Linux io_uring submission and completion queues, eliminating per-I/O
/// syscall overhead compared to standard tokio TCP.
///
/// Rate limiting: Token bucket (55K TPS) prevents OOM under extreme load.
/// Requests exceeding the limit receive gRPC ResourceExhausted error.
pub struct IoUringTransportServer {
    addr: String,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    rate_limiter: Arc<TokenBucket>,
    shutdown: Arc<AtomicBool>,
}

impl IoUringTransportServer {
    /// Creates a new `IoUringTransportServer`.
    ///
    /// - `addr`        — bind address (e.g. `"0.0.0.0:7878"`).
    /// - `pipeline`    — shared engine pipeline.
    /// - `ring_buffer` — shared ring buffer for result polling.
    pub fn new(addr: &str, pipeline: Arc<Pipeline>, ring_buffer: Arc<RingBuffer>) -> Self {
        let rate_limiter = Arc::new(TokenBucket::new(RATE_LIMIT_TPS, RATE_LIMIT_BURST));
        info!(
            rate_limit_tps = RATE_LIMIT_TPS,
            burst = RATE_LIMIT_BURST,
            "io_uring: rate limiter enabled (lock-free token bucket)"
        );

        Self {
            addr: addr.to_owned(),
            pipeline,
            ring_buffer,
            rate_limiter,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl TransportServer for IoUringTransportServer {
    /// Start the io_uring TCP transport.
    ///
    /// Runs the accept loop inside `tokio::task::spawn_blocking` → `tokio_uring::start`
    /// so the io_uring runtime does not interfere with the outer tokio runtime.
    async fn serve(&self) -> BlazerResult<()> {
        let addr = self.addr.clone();
        let pipeline = Arc::clone(&self.pipeline);
        let ring_buffer = Arc::clone(&self.ring_buffer);
        let rate_limiter = Arc::clone(&self.rate_limiter);
        let shutdown = Arc::clone(&self.shutdown);

        tokio::task::spawn_blocking(move || {
            tokio_uring::start(uring_accept_loop(
                addr,
                pipeline,
                ring_buffer,
                rate_limiter,
                shutdown,
            ))
        })
        .await
        .map_err(|e| BlazerError::Transport(format!("io_uring task panicked: {e}")))?
    }

    async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        info!("io_uring transport: shutdown requested");
    }

    fn local_addr(&self) -> &str {
        &self.addr
    }
}

// ── io_uring accept loop ──────────────────────────────────────────────────────

/// Runs the io_uring accept loop.
///
/// Binds a `tokio_uring::net::TcpListener`, accepts connections in a loop,
/// and spawns one `tokio_uring` task per connection.  The loop exits when
/// the shutdown flag is set or an unrecoverable listener error occurs.
async fn uring_accept_loop(
    addr: String,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    rate_limiter: Arc<TokenBucket>,
    shutdown: Arc<AtomicBool>,
) -> BlazerResult<()> {
    let listener = tokio_uring::net::TcpListener::bind(
        addr.parse::<std::net::SocketAddr>()
            .map_err(|e| BlazerError::Transport(format!("invalid bind address '{addr}': {e}")))?,
    )
    .map_err(|e| BlazerError::Transport(format!("io_uring bind failed on {addr}: {e}")))?;

    info!(addr = %addr, "🚀 io_uring TCP transport active");

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        let (stream, peer_addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                error!(error = %e, "io_uring accept() failed");
                continue;
            }
        };

        let pipeline = Arc::clone(&pipeline);
        let ring_buffer = Arc::clone(&ring_buffer);
        let rate_limiter = Arc::clone(&rate_limiter);

        tokio_uring::spawn(async move {
            if let Err(e) =
                handle_uring_connection(stream, pipeline, ring_buffer, rate_limiter).await
            {
                warn!(peer = %peer_addr, error = %e, "io_uring connection handler error");
            }
        });
    }

    info!("io_uring accept loop exited");
    Ok(())
}

// ── Per-connection handler ────────────────────────────────────────────────────

/// Handles one client connection using io_uring async I/O for its full lifetime.
///
/// Loops reading one framed request per iteration and writing one framed
/// response.  Exits cleanly on EOF or unrecoverable I/O error.
async fn handle_uring_connection(
    stream: tokio_uring::net::TcpStream,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    rate_limiter: Arc<TokenBucket>,
) -> BlazerResult<()> {
    loop {
        // ── Step 1: Read 4-byte length header ─────────────────────────────
        let len_buf = vec![0u8; 4];
        let (res, len_buf) = stream.read(len_buf).await;
        let n = match res {
            Ok(0) => {
                debug!("io_uring connection: EOF");
                break;
            }
            Ok(n) => n,
            Err(e) => {
                debug!(error = %e, "io_uring read header failed");
                break;
            }
        };
        if n < 4 {
            warn!(read = n, "io_uring: short header read — closing connection");
            break;
        }
        let payload_len =
            u32::from_be_bytes([len_buf[0], len_buf[1], len_buf[2], len_buf[3]]) as usize;

        if payload_len > MAX_FRAME_SIZE {
            warn!(
                len = payload_len,
                max = MAX_FRAME_SIZE,
                "io_uring: frame too large — closing connection"
            );
            break;
        }

        // ── Step 2: Read payload ───────────────────────────────────────────
        let payload_buf = vec![0u8; payload_len];
        let (res, payload_buf) = stream.read(payload_buf).await;
        match res {
            Ok(0) => {
                debug!("io_uring connection: EOF on payload");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                debug!(error = %e, "io_uring read payload failed");
                break;
            }
        }

        // ── Step 3: Deserialize request ────────────────────────────────────
        let request = match deserialize_request(&payload_buf) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "io_uring: malformed request — sending rejection");
                let resp = uring_error_response("", &e);
                let _ = write_frame_uring(&stream, &resp).await;
                continue;
            }
        };

        let request_id = request.request_id.clone();

        // ── Step 4: Rate Limiting (Token Bucket Check) ────────────────────
        // Lock-free check: if bucket empty, reject immediately.
        // Prevents OOM under extreme load (>55K TPS).
        if !rate_limiter.try_consume() {
            warn!(%request_id, "io_uring: rate limit exceeded (55K TPS) — rejecting");
            let resp = TransactionResponse {
                request_id: request_id.clone(),
                committed: false,
                transfer_id: None,
                error: Some("rate limit exceeded (55K TPS max)".into()),
                timestamp_ns: Timestamp::now().as_nanos(),
            };
            let _ = write_frame_uring(&stream, &resp).await;
            continue;
        }

        // ── Step 5: Build TransactionEvent ────────────────────────────────
        let event = match build_event(request) {
            Ok(e) => e,
            Err(e) => {
                warn!(%request_id, error = %e, "io_uring: event build failed");
                let resp = uring_error_response(&request_id, &e);
                let _ = write_frame_uring(&stream, &resp).await;
                continue;
            }
        };

        // ── Step 6: Publish to pipeline ────────────────────────────────────
        let seq = match pipeline.publish_event(event) {
            Ok(s) => s,
            Err(e) => {
                error!(%request_id, error = %e, "io_uring: publish_event failed");
                let resp = uring_error_response(&request_id, &e);
                let _ = write_frame_uring(&stream, &resp).await;
                continue;
            }
        };

        // ── Step 7: Wait for result (up to 100 ms) ────────────────────────
        let results = pipeline.results();
        let result = match wait_for_result(&results, seq).await {
            Some(r) => r,
            None => {
                warn!(%request_id, "io_uring: processing timeout");
                let resp = TransactionResponse {
                    request_id: request_id.clone(),
                    committed: false,
                    transfer_id: None,
                    error: Some("processing timeout".into()),
                    timestamp_ns: Timestamp::now().as_nanos(),
                };
                let _ = write_frame_uring(&stream, &resp).await;
                continue;
            }
        };

        // ── Step 8: Send response ──────────────────────────────────────────
        let response = build_response(&request_id, result);
        let _ = write_frame_uring(&stream, &response).await;
    }

    Ok(())
}

// ── Frame I/O helpers ─────────────────────────────────────────────────────────

/// Writes a length-prefixed MessagePack frame using io_uring async write.
async fn write_frame_uring(
    stream: &tokio_uring::net::TcpStream,
    response: &TransactionResponse,
) -> BlazerResult<()> {
    let payload = serialize_response(response)?;
    let len = payload.len() as u32;
    let mut wire = Vec::with_capacity(4 + payload.len());
    wire.extend_from_slice(&len.to_be_bytes());
    wire.extend_from_slice(&payload);

    let (res, _) = stream.write(wire).await;
    res.map_err(|e| BlazerError::Transport(format!("io_uring write failed: {e}")))?;
    Ok(())
}

// ── Pipeline helpers ──────────────────────────────────────────────────────────

/// Polls the results map at `seq` until a result appears or 100 ms elapses.
async fn wait_for_result(
    results: &Arc<DashMap<i64, TransactionResult>>,
    seq: i64,
) -> Option<TransactionResult> {
    let results = Arc::clone(results);
    let fut = async move {
        loop {
            if let Some(r) = results.get(&seq) {
                return r.value().clone();
            }
            tokio::task::yield_now().await;
        }
    };
    timeout(RESULT_TIMEOUT, fut).await.ok()
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

/// Constructs an error [`TransactionResponse`].
fn uring_error_response(request_id: &str, err: &BlazerError) -> TransactionResponse {
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
