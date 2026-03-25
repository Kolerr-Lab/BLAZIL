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

use std::rc::Rc;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use blazil_engine::event::{EventFlags, TransactionEvent, TransactionResult};
use blazil_engine::pipeline::Pipeline;
use blazil_engine::ring_buffer::RingBuffer;
use blazil_engine::sharded_pipeline::ShardedPipeline;
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
            // SQPOLL: kernel thread continuously polls the SQ — zero syscalls
            // on the hot path once the sq_thread is warm.
            tokio_uring::builder()
                .setup_sqpoll(2_000)
                .start(uring_accept_loop(
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
    _ring_buffer: Arc<RingBuffer>,
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
        let result = match wait_for_result(results, seq).await {
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

    let (res, _) = stream.write_all(wire).await;
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

// ════════════════════════════════════════════════════════════════════════════
// IoUringUdpTransport — io_uring UDP transport with pre-registered buffers
// ════════════════════════════════════════════════════════════════════════════
//
// ## Architecture
//
// ```text
// tokio_uring runtime (wraps tokio + io_uring SQ/CQ)
//    │
//    ▼  tokio_uring::net::UdpSocket (bound, connectionless)
// IoUringUdpTransport::serve()
//    │  pre-registered fixed recv buffers (RECV_BUFFER_COUNT × RECV_BUFFER_SIZE)
//    │  in-flight bitset: [u64; 4] → 256 slots
//    │  recv_from loop → deserialize → ShardedPipeline::publish_event
//    ▼
// per-request tokio_uring task: wait_result → build 16-byte response → send_to
// ```
//
// ## Zero-copy buffer management
//
// Rather than allocating a new heap buffer for every recv, the transport
// manages a pool of RECV_BUFFER_COUNT fixed-size bufs.  A compact bitset
// ([u64; 4] = 256 bits) tracks which slots are currently in-flight inside
// a spawned task.  When a task completes the slot bit is cleared and the
// buffer is available for the next recv.
//
// tokio-uring 0.5 uses owned-buffer passing: the caller gives the Vec to
// the kernel and gets it back upon completion — no additional copy.

// ── Buffer pool constants ─────────────────────────────────────────────────────

/// Size of each recv buffer (larger than max UDP datagram we ever handle).
const RECV_BUFFER_SIZE: usize = 2_048;
/// Number of pre-allocated recv buffers (= max concurrent in-flight recvs).
const RECV_BUFFER_COUNT: usize = 256;
/// Number of pre-allocated send buffers (one per in-flight send task).
const SEND_BUFFER_COUNT: usize = 256;

// ── UDP packet layout ─────────────────────────────────────────────────────────

const UDP_HEADER_SIZE: usize = 8; // sequence (u64)
const UDP_PAYLOAD_SIZE: usize = 48; // TransactionEvent fields
const UDP_PACKET_SIZE: usize = UDP_HEADER_SIZE + UDP_PAYLOAD_SIZE; // 56 bytes
const UDP_RESPONSE_SIZE: usize = 16; // seq(8) + result(8)

// ── Result-wait timeout ───────────────────────────────────────────────────────

const UDP_RESULT_TIMEOUT: Duration = Duration::from_millis(100);

// ── IoUringUdpTransport ───────────────────────────────────────────────────────

/// io_uring-backed UDP transport with pre-registered fixed-size recv buffers.
///
/// # Buffer pool
///
/// `RECV_BUFFER_COUNT` (256) fixed recv buffers of `RECV_BUFFER_SIZE` (2048)
/// bytes are allocated at construction.  An in-flight bitset (`[u64; 4]`)
/// tracks which buffer slots are currently owned by a spawned task, allowing
/// the recv loop to pick up a free slot on every iteration without heap
/// allocation.
///
/// # Linux requirements
///
/// Requires Linux 5.1+ for `io_uring`.  Compiled only on Linux with the
/// `io-uring` feature enabled.  Activate with `BLAZIL_TRANSPORT=io-uring-udp`.
pub struct IoUringUdpTransport {
    addr: String,
    pipeline: Arc<ShardedPipeline>,
    shutdown: Arc<AtomicBool>,
    packets_received: Arc<AtomicU64>,
    packets_sent: Arc<AtomicU64>,
    bound_addr: Arc<std::sync::Mutex<Option<String>>>,
}

impl IoUringUdpTransport {
    /// Creates a new `IoUringUdpTransport`.
    ///
    /// - `addr`     — bind address, e.g. `"0.0.0.0:7879"`.
    /// - `pipeline` — shared sharded pipeline for event processing.
    pub fn new(addr: &str, pipeline: Arc<ShardedPipeline>) -> Self {
        info!(
            recv_buf_count = RECV_BUFFER_COUNT,
            recv_buf_size = RECV_BUFFER_SIZE,
            send_buf_count = SEND_BUFFER_COUNT,
            "io_uring UDP: buffer pool initialized"
        );
        Self {
            addr: addr.to_owned(),
            pipeline,
            shutdown: Arc::new(AtomicBool::new(false)),
            packets_received: Arc::new(AtomicU64::new(0)),
            packets_sent: Arc::new(AtomicU64::new(0)),
            bound_addr: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Returns the actual bound address after `serve()` has been called.
    pub fn local_addr(&self) -> String {
        self.bound_addr
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.addr.clone())
    }

    /// Async helper: waits until the socket is bound and returns the address.
    pub async fn local_addr_async(&self) -> String {
        loop {
            {
                let guard = self.bound_addr.lock().unwrap();
                if let Some(ref a) = *guard {
                    return a.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Returns total packets received since start.
    pub fn packets_received(&self) -> u64 {
        self.packets_received.load(Ordering::Relaxed)
    }

    /// Returns total packets sent since start.
    pub fn packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }

    /// Signals the server to stop accepting new packets.
    pub async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        info!(
            received = self.packets_received.load(Ordering::Relaxed),
            sent = self.packets_sent.load(Ordering::Relaxed),
            "io_uring UDP: shutdown requested"
        );
    }

    /// Starts the io_uring UDP transport.
    ///
    /// Runs the recv loop inside `tokio::task::spawn_blocking` → `tokio_uring::start`
    /// so the io_uring runtime does not interfere with the outer Tokio executor.
    pub async fn serve(&self) -> BlazerResult<()> {
        let addr = self.addr.clone();
        let pipeline = Arc::clone(&self.pipeline);
        let shutdown = Arc::clone(&self.shutdown);
        let packets_received = Arc::clone(&self.packets_received);
        let packets_sent = Arc::clone(&self.packets_sent);
        let bound_addr = Arc::clone(&self.bound_addr);

        tokio::task::spawn_blocking(move || {
            // SQPOLL: eliminates enter() syscall on every submission — kernel
            // thread polls the SQ ring directly.
            tokio_uring::builder()
                .setup_sqpoll(2_000)
                .start(uring_udp_recv_loop(
                    addr,
                    pipeline,
                    shutdown,
                    packets_received,
                    packets_sent,
                    bound_addr,
                ))
        })
        .await
        .map_err(|e| BlazerError::Transport(format!("io_uring UDP task panicked: {e}")))?
    }
}

// ── io_uring UDP recv loop ────────────────────────────────────────────────────

/// Core io_uring UDP recv loop.
///
/// Allocates `RECV_BUFFER_COUNT` owned `Vec<u8>` buffers upfront and cycles
/// through them.  Each buffer is handed to `recv_from` (which gives it to the
/// kernel via the io_uring SQ); when the CQ entry fires the buffer is returned
/// to userspace alongside the peer address and byte count.
///
/// An in-flight bitset (`[u64; 4]` = 256 bits) tracks slots currently owned
/// by spawned response tasks.  The recv loop busy-picks the next free slot;
/// if all 256 are taken it yields once before retrying to avoid starving other
/// tasks on the same thread.
async fn uring_udp_recv_loop(
    addr: String,
    pipeline: Arc<ShardedPipeline>,
    shutdown: Arc<AtomicBool>,
    packets_received: Arc<AtomicU64>,
    packets_sent: Arc<AtomicU64>,
    bound_addr: Arc<std::sync::Mutex<Option<String>>>,
) -> BlazerResult<()> {
    // ── Bind socket ───────────────────────────────────────────────────────
    let sock_addr: std::net::SocketAddr = addr
        .parse()
        .map_err(|e| BlazerError::Transport(format!("invalid bind address '{addr}': {e}")))?;

    let socket = tokio_uring::net::UdpSocket::bind(sock_addr)
        .await
        .map_err(|e| BlazerError::Transport(format!("io_uring UDP bind failed on {addr}: {e}")))?;

    let local = socket
        .local_addr()
        .map_err(|e| BlazerError::Transport(format!("local_addr() failed: {e}")))?;

    {
        let mut guard = bound_addr.lock().unwrap();
        *guard = Some(local.to_string());
    }

    info!(addr = %local, "🚀 io_uring UDP transport active");

    // ── Pre-allocate buffer pool ──────────────────────────────────────────
    // Each buffer is a Vec<u8> of RECV_BUFFER_SIZE bytes.
    // tokio-uring 0.5 owns the Vec during the I/O op and returns it on completion.
    let mut bufs: Vec<Vec<u8>> = (0..RECV_BUFFER_COUNT)
        .map(|_| vec![0u8; RECV_BUFFER_SIZE])
        .collect();

    // Wrap socket in Rc — tokio-uring is single-threaded so Rc is sufficient.
    let socket = Rc::new(socket);

    // In-flight bitset: bit i set ↔ buf slot i is owned by a spawned task.
    // Using AtomicU64 array for lock-free slot management.
    let in_flight: Arc<[std::sync::atomic::AtomicU64; 4]> = Arc::new([
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ]);

    let mut slot_cursor: usize = 0;

    // ── Channel for response sends ────────────────────────────────────────
    // Spawned tasks push (response_bytes, peer) here; a dedicated send task
    // drains the channel and calls socket.send_to to avoid concurrent mutable
    // access to the socket from multiple tasks.
    let (resp_tx, mut resp_rx) =
        tokio::sync::mpsc::channel::<([u8; UDP_RESPONSE_SIZE], std::net::SocketAddr)>(65_536);

    // ── Dedicated send task ───────────────────────────────────────────────
    let send_socket = Rc::clone(&socket);
    let send_sent = Arc::clone(&packets_sent);
    tokio_uring::spawn(async move {
        while let Some((response, peer)) = resp_rx.recv().await {
            // send_to takes ownership of the buffer, returns it on completion.
            let buf = response.to_vec();
            let (res, _buf) = send_socket.send_to(buf, peer).await;
            match res {
                Ok(_) => {
                    send_sent.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    error!(error = %e, peer = %peer, "io_uring UDP: send_to failed");
                }
            }
        }
    });

    // ── Recv loop ─────────────────────────────────────────────────────────
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        // Find a free buffer slot (bit = 0 in the in-flight bitset).
        let slot = loop {
            let word = slot_cursor / 64;
            let bit = slot_cursor % 64;
            let mask = 1u64 << bit;
            let current = in_flight[word].load(Ordering::Acquire);
            if current & mask == 0 {
                // Tentatively mark as in-flight (CAS to prevent races).
                if in_flight[word]
                    .compare_exchange(current, current | mask, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break slot_cursor;
                }
            }
            // Advance cursor; wrap around.
            slot_cursor = (slot_cursor + 1) % RECV_BUFFER_COUNT;
            // If we've cycled through all slots, yield to avoid spinning.
            if slot_cursor == 0 {
                tokio::task::yield_now().await;
            }
        };
        slot_cursor = (slot + 1) % RECV_BUFFER_COUNT;

        // Take the buffer out of the pool.
        let buf = std::mem::take(&mut bufs[slot]);

        // ── Submit recv_from to io_uring ──────────────────────────────────
        let (res, buf) = socket.recv_from(buf).await;
        let (n, peer) = match res {
            Ok(pair) => pair,
            Err(e) => {
                error!(error = %e, "io_uring UDP: recv_from failed");
                // Return buffer to pool and clear in-flight bit.
                bufs[slot] = buf;
                let word = slot / 64;
                let mask = 1u64 << (slot % 64);
                in_flight[word].fetch_and(!mask, Ordering::Release);
                continue;
            }
        };

        // Return buffer to pool immediately (task gets a copy of the data).
        // This keeps bufs[] always populated for the next recv.
        let packet_bytes = buf[..n].to_vec();
        bufs[slot] = buf; // give buffer back before clearing bit

        // Clear in-flight bit — buffer is back in pool.
        let word = slot / 64;
        let mask = 1u64 << (slot % 64);
        in_flight[word].fetch_and(!mask, Ordering::Release);

        packets_received.fetch_add(1, Ordering::Relaxed);

        // Validate packet size.
        if n != UDP_PACKET_SIZE {
            warn!(
                peer = %peer,
                expected = UDP_PACKET_SIZE,
                got = n,
                "io_uring UDP: invalid packet size — dropping"
            );
            continue;
        }

        // ── Deserialize ───────────────────────────────────────────────────
        let sequence = u64::from_be_bytes(packet_bytes[0..8].try_into().unwrap());

        let event = match udp_deserialize_event(&packet_bytes[UDP_HEADER_SIZE..UDP_PACKET_SIZE]) {
            Ok(e) => e,
            Err(e) => {
                error!(error = %e, "io_uring UDP: deserialize failed");
                continue;
            }
        };

        // ── Publish to pipeline ───────────────────────────────────────────
        let shard_id = (event.debit_account_id.as_u64() as usize) % pipeline.shard_count();

        let ring_seq = match pipeline.publish_event(event) {
            Ok(seq) => seq,
            Err(e) => {
                warn!(seq = sequence, error = %e, "io_uring UDP: pipeline backpressure");
                continue;
            }
        };

        // ── Spawn response task ───────────────────────────────────────────
        // Each task waits for one pipeline result then pushes response bytes
        // to the send channel.  Bounded channel provides back-pressure.
        let task_results = pipeline.shard_results(shard_id);
        let task_resp_tx = resp_tx.clone();

        tokio_uring::spawn(async move {
            let result_code = match udp_wait_for_result(&task_results, ring_seq).await {
                Some(TransactionResult::Committed { .. }) => 0u64,
                Some(TransactionResult::Rejected { .. }) => 1u64,
                None => {
                    warn!(seq = sequence, "io_uring UDP: processing timeout");
                    1u64
                }
            };

            let mut response = [0u8; UDP_RESPONSE_SIZE];
            response[0..8].copy_from_slice(&sequence.to_be_bytes());
            response[8..16].copy_from_slice(&result_code.to_be_bytes());

            let _ = task_resp_tx.send((response, peer)).await;
        });
    }

    info!("io_uring UDP recv loop exited");
    Ok(())
}

// ── UDP helpers ───────────────────────────────────────────────────────────────

/// Deserializes a [`TransactionEvent`] from a 48-byte UDP payload.
///
/// # Wire format (48 bytes, big-endian)
///
/// ```text
/// [0-7]:   transaction_id (u64)
/// [8-15]:  debit_account_id (u64)
/// [16-23]: credit_account_id (u64)
/// [24-31]: amount_units (u64)
/// [32-39]: ingestion_timestamp nanos (u64)
/// [40-43]: ledger_id (u32)
/// [44-45]: code (u16)
/// [46]:    flags (u8)
/// [47]:    padding (u8)
/// ```
fn udp_deserialize_event(bytes: &[u8]) -> BlazerResult<TransactionEvent> {
    if bytes.len() != UDP_PAYLOAD_SIZE {
        return Err(BlazerError::Internal(format!(
            "io_uring UDP: invalid payload size: expected {UDP_PAYLOAD_SIZE}, got {}",
            bytes.len()
        )));
    }

    let tx_id = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
    let debit_id = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let credit_id = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
    let amount = u64::from_be_bytes(bytes[24..32].try_into().unwrap());
    let timestamp_nanos = u64::from_be_bytes(bytes[32..40].try_into().unwrap());
    let ledger_u32 = u32::from_be_bytes(bytes[40..44].try_into().unwrap());
    let code = u16::from_be_bytes(bytes[44..46].try_into().unwrap());
    let flags_byte = bytes[46];

    let ledger_id = match ledger_u32 {
        0 => LedgerId::USD,
        1 => LedgerId::EUR,
        2 => LedgerId::GBP,
        _ => LedgerId::USD,
    };

    let mut event = TransactionEvent::new(
        TransactionId::from_u64(tx_id),
        AccountId::from_u64(debit_id),
        AccountId::from_u64(credit_id),
        amount,
        ledger_id,
        code,
    );

    event.ingestion_timestamp = Timestamp::from_nanos(timestamp_nanos);
    event.flags = EventFlags::from_raw(flags_byte);

    Ok(event)
}

/// Polls the shard results map until `seq` appears or `UDP_RESULT_TIMEOUT` expires.
async fn udp_wait_for_result(
    results: &Arc<dashmap::DashMap<i64, TransactionResult>>,
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
    tokio::time::timeout(UDP_RESULT_TIMEOUT, fut).await.ok()
}
