//! RDMA (InfiniBand / RoCEv2) zero-copy transport server.
//!
//! Uses ibverbs RC (Reliable Connected) Queue Pairs for kernel-bypass data
//! transfer. A TCP side-channel handles QP parameter exchange at connection
//! setup; all subsequent I/O bypasses the kernel entirely.
//!
//! # Architecture
//!
//! ```text
//! Enterprise client
//!    │  [TCP side-channel: QP parameter exchange only]
//!    ▼
//! RdmaTransportServer (rdma_transport.rs)
//!    │  tokio TcpListener::accept → into_std → std::thread::spawn
//!    ▼
//! rdma_connection (OS thread, one per client)
//!    │  create CQ + RC QP → PreparedQueuePair::handshake → ConnectedQueuePair
//!    │  alloc DMA-registered MemoryRegion (recv + send)
//!    │  CQ busy-poll loop:
//!    │    IBV_WC_RECV → deserialize → pipeline.publish_event
//!    │               → spin-wait result → post_send response
//!    ▼
//! blazil-engine pipeline (unchanged hot path)
//! ```
//!
//! # Performance
//!
//! | Metric      | TCP (tokio) | RDMA RC (ibverbs)         |
//! |-------------|-------------|---------------------------|
//! | P50 latency | ~10 µs      | ~0.5 µs                   |
//! | P99 latency | ~50 µs      | ~2 µs                     |
//! | CPU / TX    | kernel path | MMIO only — zero syscall  |
//! | Copies      | 2 (k→u)     | 0 (DMA direct to MR)      |
//!
//! # Hardware requirements
//!
//! - Linux kernel with `libibverbs-dev` + `librdmacm-dev` installed
//! - InfiniBand HCA or RoCEv2 NIC (Mellanox mlx5 recommended)
//! - softRoCE for dev/test:
//!   `modprobe rdma_rxe && rdma link add rxe0 type rxe seldev eth0`
//! - Cargo feature: `--features rdma`
//! - Runtime env: `BLAZIL_TRANSPORT=rdma`
//! - Optional: `BLAZIL_RDMA_DEVICE=mlx5_0` (defaults to first enumerated device)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use ibverbs::ibv_qp_type;
use tracing::{error, info, warn};

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::TransactionResult;
use blazil_engine::pipeline::Pipeline;

use crate::connection::{build_event, build_response};
use crate::protocol::{deserialize_request, serialize_response, TransactionResponse};
use crate::server::TransportServer;

// ── Constants ─────────────────────────────────────────────────────────────────

/// DMA-registered buffer size per connection (recv and send each).
///
/// Matches the existing Blazil frame ceiling — a single RDMA send/receive
/// carries at most this many bytes. Clients must not exceed this.
const RDMA_BUFFER_SIZE: usize = 65_536;

/// Completion queue depth per connection.
///
/// 64 entries is generous for the single-outstanding model; raise if
/// batched pipelining is added in a future iteration.
const CQ_DEPTH: i32 = 64;

/// Maximum outstanding send / receive work requests per QP.
const MAX_WR: i32 = 32;

/// How long the blocking result-wait spin loop waits before giving up.
const RESULT_TIMEOUT: Duration = Duration::from_millis(100);

// ── RdmaTransportServer ───────────────────────────────────────────────────────

/// An RDMA (InfiniBand / RoCEv2) Blazil transport server.
///
/// Listens on `bind_addr` for inbound TCP connections (QP side-channel).
/// Each accepted connection is handed to a dedicated OS thread that creates
/// an RC Queue Pair, performs the ibverbs handshake, and enters a busy-poll
/// CQ loop for sub-microsecond request processing.
///
/// Activate with `BLAZIL_TRANSPORT=rdma`.
/// Optionally select the RDMA device with `BLAZIL_RDMA_DEVICE=<name>`.
pub struct RdmaTransportServer {
    /// TCP bind address for the QP side-channel.
    bind_addr: String,
    pipeline: Arc<Pipeline>,
    results: Arc<DashMap<i64, TransactionResult>>,
    shutdown: Arc<AtomicBool>,
    active_connections: Arc<AtomicU64>,
    /// RDMA device name override. `None` = first enumerated device.
    device_name: Option<String>,
}

impl RdmaTransportServer {
    /// Creates a new `RdmaTransportServer`.
    ///
    /// No hardware resources are allocated until [`serve`] is called.
    ///
    /// # Arguments
    ///
    /// - `addr`        — TCP bind address for the QP side-channel.
    /// - `pipeline`    — shared engine pipeline.
    /// - `results`     — shared results map.
    /// - `device_name` — optional InfiniBand device name (`BLAZIL_RDMA_DEVICE`).
    pub fn new(
        addr: &str,
        pipeline: Arc<Pipeline>,
        results: Arc<DashMap<i64, TransactionResult>>,
        device_name: Option<String>,
    ) -> Self {
        Self {
            bind_addr: addr.to_owned(),
            pipeline,
            results,
            shutdown: Arc::new(AtomicBool::new(false)),
            active_connections: Arc::new(AtomicU64::new(0)),
            device_name,
        }
    }
}

#[async_trait]
impl TransportServer for RdmaTransportServer {
    /// Enumerate RDMA devices, open the selected one, allocate a shared
    /// Protection Domain, and accept inbound connections.
    ///
    /// Each accepted connection is moved into a dedicated OS thread.
    /// The method returns only after [`shutdown`] is called.
    async fn serve(&self) -> BlazerResult<()> {
        // ── Open RDMA device + shared Protection Domain ───────────────────
        let device_list = ibverbs::devices().map_err(|e| {
            BlazerError::Transport(format!("ibverbs: cannot enumerate devices: {e}"))
        })?;

        let ctx = {
            let want = self.device_name.as_deref();
            let device = device_list
                .iter()
                .find(|d| match want {
                    Some(name) => d.name().map_or(false, |n| n == name),
                    None => true,
                })
                .ok_or_else(|| {
                    BlazerError::Transport(format!(
                        "no RDMA device found (BLAZIL_RDMA_DEVICE={:?})",
                        self.device_name
                    ))
                })?;

            info!(device = ?device.name(), "opening RDMA device");
            device
                .open()
                .map_err(|e| BlazerError::Transport(format!("ibverbs open device: {e}")))?
        };
        // device_list is dropped here; ctx is Arc so it owns the reference.

        let pd = ctx
            .alloc_pd()
            .map_err(|e| BlazerError::Transport(format!("ibverbs alloc_pd: {e}")))?;

        info!(
            addr = %self.bind_addr,
            "RDMA transport ready — RC QPs, poll-mode CQ, DMA MRs ({} KiB each)",
            RDMA_BUFFER_SIZE / 1024,
        );

        // ── Async TCP listener (QP parameter exchange side-channel) ───────
        let listener = tokio::net::TcpListener::bind(&self.bind_addr)
            .await
            .map_err(|e| {
                BlazerError::Transport(format!("RDMA TCP bind {}: {e}", self.bind_addr))
            })?;

        loop {
            let accept_result = tokio::select! {
                res = listener.accept() => res,
                _ = rdma_shutdown_signal(Arc::clone(&self.shutdown)) => break,
            };

            let (tokio_stream, peer_addr) = match accept_result {
                Ok(pair) => pair,
                Err(e) => {
                    error!(error = %e, "RDMA side-channel accept() failed");
                    continue;
                }
            };

            // Convert to std TcpStream for the synchronous ibverbs handshake.
            let tcp_stream = match tokio_stream.into_std() {
                Ok(s) => s,
                Err(e) => {
                    warn!(peer = %peer_addr, error = %e, "into_std() failed — dropping connection");
                    continue;
                }
            };
            // ibverbs handshake is blocking — set to blocking mode explicitly.
            if let Err(e) = tcp_stream.set_nonblocking(false) {
                warn!(peer = %peer_addr, error = %e, "set_nonblocking(false) failed");
            }

            info!(peer = %peer_addr, "RDMA connection incoming");

            let pipeline = Arc::clone(&self.pipeline);
            let results = Arc::clone(&self.results);
            let shutdown = Arc::clone(&self.shutdown);
            let active = Arc::clone(&self.active_connections);
            let ctx_clone = Arc::clone(&ctx);
            let pd_clone = Arc::clone(&pd);

            active.fetch_add(1, Ordering::Release);
            std::thread::spawn(move || {
                if let Err(e) =
                    rdma_connection(tcp_stream, ctx_clone, pd_clone, pipeline, results, shutdown)
                {
                    warn!(peer = %peer_addr, error = %e, "RDMA connection error");
                }
                active.fetch_sub(1, Ordering::Release);
            });
        }

        info!("RDMA transport accept loop exited");
        Ok(())
    }

    async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);

        let deadline = Instant::now() + Duration::from_secs(5);
        while self.active_connections.load(Ordering::Acquire) > 0 {
            if Instant::now() >= deadline {
                warn!(
                    remaining = self.active_connections.load(Ordering::Acquire),
                    "RDMA shutdown timeout — some connections still active"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        info!("RDMA transport shut down");
    }

    fn local_addr(&self) -> &str {
        &self.bind_addr
    }
}

// ── Per-connection RDMA handler ───────────────────────────────────────────────

/// Runs on a dedicated OS thread: creates a per-connection CQ and RC QP,
/// exchanges parameters via the TCP side-channel, and enters a poll-mode
/// CQ loop until the peer disconnects or shutdown is signalled.
///
/// # Protocol
///
/// Frames use the same layout as the TCP transport:
///
/// ```text
/// ┌──────────────────┬─────────────────────────┐
/// │ 4 bytes (u32 BE) │  N bytes (MessagePack)  │
/// │  payload length  │  TransactionRequest     │
/// └──────────────────┴─────────────────────────┘
/// ```
///
/// This means any client supporting the Blazil TCP protocol can connect via
/// RDMA with only a transport-layer change — the application protocol is
/// identical.
fn rdma_connection(
    tcp_stream: std::net::TcpStream,
    ctx: Arc<ibverbs::Context>,
    pd: Arc<ibverbs::ProtectionDomain>,
    pipeline: Arc<Pipeline>,
    results: Arc<DashMap<i64, TransactionResult>>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ── Per-connection Completion Queue ───────────────────────────────────
    let cq = ctx
        .create_cq(CQ_DEPTH, ())
        .map_err(|e| format!("ibverbs create_cq: {e}"))?;

    // ── Build RC Queue Pair ───────────────────────────────────────────────
    //
    // RC = Reliable Connected: guaranteed in-order delivery with hardware
    // retransmission. Required for financial transactions.
    let prepared_qp = pd
        .create_qp(&cq, &cq, ibv_qp_type::IBV_QPT_RC)
        .set_max_send_wr(MAX_WR)
        .set_max_recv_wr(MAX_WR)
        .build()
        .map_err(|e| format!("ibverbs create_qp: {e}"))?;

    // ── Exchange QP parameters via TCP side-channel ───────────────────────
    //
    // Both peers call handshake(stream) simultaneously. The method
    // serialises local QP attributes (QP num, LID/GID), deserialises the
    // peer's, and transitions the QP to RTS (Ready To Send) state.
    let mut qp = prepared_qp
        .handshake(tcp_stream)
        .map_err(|e| format!("ibverbs QP handshake: {e}"))?;

    // ── DMA-registered memory regions ────────────────────────────────────
    //
    // Memory is registered with the HCA so the NIC can DMA directly into/
    // out of these buffers without OS involvement on the hot path.
    let mut recv_mr: ibverbs::MemoryRegion<u8> = pd
        .allocate(RDMA_BUFFER_SIZE)
        .map_err(|e| format!("ibverbs alloc recv MR: {e}"))?;
    let mut send_mr: ibverbs::MemoryRegion<u8> = pd
        .allocate(RDMA_BUFFER_SIZE)
        .map_err(|e| format!("ibverbs alloc send MR: {e}"))?;

    // Pre-arm the receive WR so the QP is ready for the client's first send.
    qp.post_receive(&mut recv_mr, 0..RDMA_BUFFER_SIZE as u64, 0)
        .map_err(|e| format!("ibverbs initial post_receive: {e}"))?;

    // Completion buffer: stack-allocated slice of ibv_wc structs.
    // SAFETY: ibv_wc is a C POD struct; zeroed initialisation is valid.
    let mut completions = vec![unsafe { std::mem::zeroed::<ibverbs::ibv_wc>() }; CQ_DEPTH as usize];

    // ── CQ busy-poll loop ─────────────────────────────────────────────────
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        let wcs = cq
            .poll(&mut completions)
            .map_err(|e| format!("ibverbs cq.poll: {e}"))?;

        if wcs.is_empty() {
            // No completions ready — spin with a CPU pause hint.
            // On the RDMA hot path this is the correct strategy: sleeping
            // would add hundreds of microseconds of wake-up latency.
            std::hint::spin_loop();
            continue;
        }

        for wc in wcs {
            // Any non-SUCCESS status indicates a fatal QP error.
            if wc.status != ibverbs::ibv_wc_status::IBV_WC_SUCCESS {
                warn!(
                    status = wc.status as u32,
                    wr_id = wc.wr_id,
                    "RDMA WC error — closing connection"
                );
                return Ok(());
            }

            match wc.opcode {
                // ── Receive completion: a full request frame arrived ──────
                ibverbs::ibv_wc_opcode::IBV_WC_RECV => {
                    let recv_len = wc.byte_len as usize;

                    // Frame must be at least 5 bytes: 4-byte header + 1 byte payload.
                    if recv_len <= 4 {
                        warn!(recv_len, "RDMA frame too short — discarding");
                        let _ = qp.post_receive(&mut recv_mr, 0..RDMA_BUFFER_SIZE as u64, 0);
                        continue;
                    }

                    // The first 4 bytes are the big-endian payload length.
                    // With RDMA, the receive completion delivers exactly the
                    // bytes the sender posted — no partial reads.
                    let payload = &recv_mr[4..recv_len];

                    let request = match deserialize_request(payload) {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(error = %e, "RDMA: malformed request — sending rejection");
                            rdma_send_error(&mut qp, &mut send_mr, "", &e);
                            let _ = qp.post_receive(&mut recv_mr, 0..RDMA_BUFFER_SIZE as u64, 0);
                            continue;
                        }
                    };

                    let request_id = request.request_id.clone();

                    let event = match build_event(request) {
                        Ok(e) => e,
                        Err(e) => {
                            warn!(%request_id, error = %e, "RDMA: build_event failed");
                            rdma_send_error(&mut qp, &mut send_mr, &request_id, &e);
                            let _ = qp.post_receive(&mut recv_mr, 0..RDMA_BUFFER_SIZE as u64, 0);
                            continue;
                        }
                    };

                    let seq = match pipeline.publish_event(event) {
                        Ok(s) => s,
                        Err(e) => {
                            error!(%request_id, error = %e, "RDMA: publish_event failed");
                            rdma_send_error(&mut qp, &mut send_mr, &request_id, &e);
                            let _ = qp.post_receive(&mut recv_mr, 0..RDMA_BUFFER_SIZE as u64, 0);
                            continue;
                        }
                    };

                    // ── Spin-wait for pipeline result ─────────────────────
                    //
                    // Busy-poll is intentional on the RDMA path — the pipeline
                    // typically completes within a few hundred nanoseconds for
                    // in-memory ledger. Sleeping here would destroy the latency
                    // advantage RDMA provides.
                    let deadline = Instant::now() + RESULT_TIMEOUT;
                    let result = loop {
                        if let Some(entry) = results.get(&seq) {
                            break Some(entry.value().clone());
                        }
                        if Instant::now() >= deadline {
                            break None;
                        }
                        std::hint::spin_loop();
                    };

                    let response = match result {
                        Some(r) => build_response(&request_id, r),
                        None => {
                            warn!(%request_id, "RDMA: pipeline timeout");
                            TransactionResponse {
                                request_id: request_id.clone(),
                                committed: false,
                                transfer_id: None,
                                error: Some("processing timeout".into()),
                                timestamp_ns: Timestamp::now().as_nanos(),
                            }
                        }
                    };

                    rdma_send_response(&mut qp, &mut send_mr, &response);

                    // Re-arm the receive WR for the next request.
                    if let Err(e) = qp.post_receive(&mut recv_mr, 0..RDMA_BUFFER_SIZE as u64, 0) {
                        warn!(error = %e, "RDMA: post_receive re-arm failed — closing");
                        return Ok(());
                    }
                }

                // ── Send completion: response was delivered ───────────────
                //
                // Single-outstanding model: one request/response at a time.
                // The send CQ completion is informational only.
                ibverbs::ibv_wc_opcode::IBV_WC_SEND => {}

                other => {
                    warn!(opcode = other as u32, "RDMA: unexpected WC opcode");
                }
            }
        }
    }

    Ok(())
}

// ── Wire-format helpers ───────────────────────────────────────────────────────

/// Serialises `response` into the DMA-registered send MR and posts a
/// RDMA send work request.
///
/// Frame layout:
/// ```text
/// [u32 BE payload_len][MessagePack bytes...]
/// ```
/// This is identical to the TCP transport wire format so clients need only
/// change the transport layer.
fn rdma_send_response(
    qp: &mut ibverbs::ConnectedQueuePair,
    send_mr: &mut ibverbs::MemoryRegion<u8>,
    response: &TransactionResponse,
) {
    let payload = match serialize_response(response) {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "RDMA: serialize_response failed");
            return;
        }
    };

    let frame_len = 4 + payload.len();
    if frame_len > RDMA_BUFFER_SIZE {
        error!(
            frame_len,
            max = RDMA_BUFFER_SIZE,
            "RDMA: response frame exceeds MR size — dropping"
        );
        return;
    }

    // Write [4-byte big-endian length][payload] into the DMA-registered MR.
    let len_bytes = (payload.len() as u32).to_be_bytes();
    send_mr[..4].copy_from_slice(&len_bytes);
    send_mr[4..frame_len].copy_from_slice(&payload);

    if let Err(e) = qp.post_send(send_mr, 0..frame_len as u64, 1) {
        warn!(error = %e, "RDMA: post_send failed");
    }
}

/// Constructs an error [`TransactionResponse`] and sends it via RDMA.
fn rdma_send_error(
    qp: &mut ibverbs::ConnectedQueuePair,
    send_mr: &mut ibverbs::MemoryRegion<u8>,
    request_id: &str,
    err: &blazil_common::error::BlazerError,
) {
    let message = match err {
        blazil_common::error::BlazerError::RingBufferFull { retry_after_ms } => {
            format!("server busy, retry after {retry_after_ms}ms")
        }
        _ => err.to_string(),
    };
    let resp = TransactionResponse {
        request_id: request_id.to_owned(),
        committed: false,
        transfer_id: None,
        error: Some(message),
        timestamp_ns: Timestamp::now().as_nanos(),
    };
    rdma_send_response(qp, send_mr, &resp);
}

// ── Shutdown signal helper ────────────────────────────────────────────────────

/// Resolves when `shutdown` is set to `true`.
///
/// Polled via `tokio::select!` in the accept loop so the server can
/// react to a shutdown signal without blocking on `accept()`.
async fn rdma_shutdown_signal(shutdown: Arc<AtomicBool>) {
    loop {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
