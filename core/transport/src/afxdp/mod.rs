//! AF_XDP zero-copy transport server.
//!
//! # Architecture overview
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────┐
//! │                    AfXdpTransportServer                            │
//! │                                                                    │
//! │  1. Boot: load & attach XDP eBPF gatekeeper to NIC                │
//! │  2. Allocate shared UMEM (128 MiB, mmap + mlock)                  │
//! │  3. Per queue:                                                     │
//! │       a. Create XSocket (AF_XDP fd bound to queue N)               │
//! │       b. Register socket fd in xsks_map (BPF → AF_XDP redirect)   │
//! │       c. Spawn OS thread: RxCursor::run() (busy-poll hot loop)     │
//! │                                                                    │
//! │  Hot path per packet:                                              │
//! │    NIC DMA → UMEM frame → RX ring descriptor                      │
//! │    → frame_data() [zero-copy slice into UMEM]                     │
//! │    → parse headers + deserialize request [reads UMEM once]        │
//! │    → return desc to fill queue [UMEM frame reused]                │
//! │    → push TransactionEvent into Pipeline ring buffer shard         │
//! │    → poll result → send UDP response                               │
//! └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Activation
//!
//! Set `BLAZIL_TRANSPORT=af-xdp` (runtime) and configure:
//!
//! ```bash
//! BLAZIL_TRANSPORT=af-xdp \
//! BLAZIL_XDP_IF=eth1 \
//! BLAZIL_XDP_QUEUES=0,1,2,3,4,5,6,7 \
//!   ./blazil-server
//! ```
//!
//! # Prerequisites
//!
//! - Linux kernel 4.18+ with `CONFIG_XDP_SOCKETS=y`
//! - NIC driver with XDP\_DRV support (ENA ≥ kernel 5.10 on AWS)
//! - `ulimit -l unlimited` (memlock for UMEM)
//! - `clang` + `libbpf-dev` for BPF compilation at build time
//! - Feature flag: `cargo build --features af-xdp -p blazil-transport`

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::TransactionResult;
use blazil_engine::event::{TransactionEvent, TransactionFlags};
use blazil_engine::pipeline::Pipeline;

use crate::protocol::TransactionRequest;
use crate::server::TransportServer;

use super::afxdp::{
    rx::RxCursor,
    socket::{XSocket, XSocketConfig},
    umem::OwnedUmem,
};
use super::ebpf::XdpGatekeeper;

// ── AfXdpConfig ───────────────────────────────────────────────────────────────

/// Runtime configuration for the AF_XDP transport server.
#[derive(Debug, Clone)]
pub struct AfXdpConfig {
    /// NIC interface name to attach the XDP program to.
    /// e.g. `"eth1"` on Linux, `"enX0"` on AWS.  Avoid eth0 if it carries
    /// management (SSH) traffic and the node has only one interface.
    pub if_name: String,

    /// NIC RX queue IDs to bind AF_XDP sockets to.
    /// Length must equal `pipeline` shard count — one socket per shard.
    /// Typical: `vec![0, 1, 2, 3, 4, 5, 6, 7]` for 8-shard bench.
    pub queue_ids: Vec<u32>,

    /// UDP port used for Blazil AF_XDP traffic.  Must match the
    /// `BLAZIL_UDP_PORT` constant in `ebpf/blazil_xdp.bpf.c`.
    pub port: u16,

    /// Request XDP\_ZEROCOPY bind flag.  Falls back to XDP\_COPY if the
    /// driver doesn't support it.
    pub zero_copy: bool,
}

impl AfXdpConfig {
    /// Build config from environment variables.
    ///
    /// | Variable               | Default          |
    /// |------------------------|------------------|
    /// | `BLAZIL_XDP_IF`        | `"eth1"`         |
    /// | `BLAZIL_XDP_QUEUES`    | `"0,1,2,3,4,5,6,7"` |
    /// | `BLAZIL_XDP_PORT`      | `"7878"`         |
    /// | `BLAZIL_XDP_ZEROCOPY`  | `"1"`            |
    pub fn from_env() -> Self {
        let if_name = std::env::var("BLAZIL_XDP_IF").unwrap_or_else(|_| "eth1".into());
        let queues_str =
            std::env::var("BLAZIL_XDP_QUEUES").unwrap_or_else(|_| "0,1,2,3,4,5,6,7".into());
        let queue_ids = queues_str
            .split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .collect();
        let port = std::env::var("BLAZIL_XDP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7878);
        let zero_copy = std::env::var("BLAZIL_XDP_ZEROCOPY")
            .map(|v| v != "0")
            .unwrap_or(true);
        Self {
            if_name,
            queue_ids,
            port,
            zero_copy,
        }
    }
}

// ── AfXdpTransportServer ──────────────────────────────────────────────────────

/// AF_XDP zero-copy transport server implementing [`TransportServer`].
pub struct AfXdpTransportServer {
    cfg: AfXdpConfig,
    pipelines: Vec<Arc<Pipeline>>,
    results: Arc<dashmap::DashMap<i64, TransactionResult>>,
    stop: Arc<AtomicBool>,
}

impl AfXdpTransportServer {
    /// Create a new server.  Does not allocate UMEM or bind sockets yet
    /// (that happens in `serve()`).
    ///
    /// `pipelines` must have one entry per queue in `cfg.queue_ids`.
    pub fn new(
        cfg: AfXdpConfig,
        pipelines: Vec<Arc<Pipeline>>,
        results: Arc<dashmap::DashMap<i64, TransactionResult>>,
    ) -> Self {
        assert_eq!(
            cfg.queue_ids.len(),
            pipelines.len(),
            "queue_ids and pipelines must have the same length"
        );
        Self {
            cfg,
            pipelines,
            results,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl TransportServer for AfXdpTransportServer {
    async fn serve(&self) -> BlazerResult<()> {
        // ── Step 1: Load and attach XDP eBPF gatekeeper ───────────────────────
        info!(if_name = %self.cfg.if_name, "Loading XDP gatekeeper");
        let mut gatekeeper = XdpGatekeeper::attach(&self.cfg.if_name)?;

        // ── Step 2: Allocate shared UMEM ──────────────────────────────────────
        info!("Allocating AF_XDP UMEM (128 MiB, mlock pinned)");
        let umem = Arc::new(OwnedUmem::new()?);
        info!(
            frames = super::afxdp::umem::FRAME_COUNT,
            frame_size = super::afxdp::umem::FRAME_SIZE,
            "UMEM allocated"
        );

        // ── Step 3: Create sockets + spawn RX cursor threads ─────────────────
        let mut rx_threads = Vec::with_capacity(self.cfg.queue_ids.len());

        for (idx, &queue_id) in self.cfg.queue_ids.iter().enumerate() {
            let pipeline = Arc::clone(&self.pipelines[idx]);
            let results = Arc::clone(&self.results);
            let stop = Arc::clone(&self.stop);
            let umem_arc = Arc::clone(&umem);
            let if_name = self.cfg.if_name.clone();
            let port = self.cfg.port;
            let zero_cp = self.cfg.zero_copy;

            // Bind socket to queue.
            let sock_cfg = XSocketConfig {
                if_name: if_name.clone(),
                queue_id,
                zero_copy: zero_cp,
            };
            // SAFETY: OwnedUmem outlives XSocket via Arc.
            let socket = unsafe_new_socket(&sock_cfg, &umem)?;

            let socket_fd = socket.raw_fd;

            // Register in xsks_map so BPF redirects this queue's packets.
            gatekeeper.register_socket(queue_id, socket_fd)?;

            info!(queue_id, socket_fd, shard = idx, "AF_XDP socket ready");

            // Bind address for UDP response socket.
            let bind_addr = format!("0.0.0.0:{}", port + idx as u16 + 1);

            let cursor = RxCursor::new(idx, socket, umem_arc, pipeline, results, &bind_addr, stop)?;

            let handle = std::thread::Builder::new()
                .name(format!("afxdp-rx-{queue_id}"))
                .spawn(move || cursor.run())
                .map_err(|e| BlazerError::Transport(format!("spawn RxCursor thread: {e}")))?;

            rx_threads.push(handle);
        }

        info!(
            queues = self.cfg.queue_ids.len(),
            if_name = %self.cfg.if_name,
            "AF_XDP transport server running — waiting for stop signal"
        );

        // ── Step 4: Block until stop() is called ──────────────────────────────
        while !self.stop.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // ── Step 5: Join RX threads ───────────────────────────────────────────
        for h in rx_threads {
            let _ = h.join();
        }

        // gatekeeper drops here → XDP program detached from NIC automatically.
        drop(gatekeeper);
        info!("AF_XDP transport server shut down");

        Ok(())
    }

    async fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn local_addr(&self) -> String {
        format!("af-xdp://{}:{}", self.cfg.if_name, self.cfg.port)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Thin wrapper to isolate the `unsafe` call to `XSocket::new`.
/// All safety preconditions are documented in `socket.rs`.
fn unsafe_new_socket(cfg: &XSocketConfig, umem: &OwnedUmem) -> BlazerResult<XSocket> {
    XSocket::new(cfg, umem)
}

/// Build a [`TransactionEvent`] from a wire [`TransactionRequest`].
///
/// Centralised here so both the TCP transport and the AF_XDP RX cursor
/// use identical parsing logic.
pub(super) fn build_event_from_request(req: &TransactionRequest) -> BlazerResult<TransactionEvent> {
    let transaction_id =
        TransactionId::from_str(&req.request_id).unwrap_or_else(|_| TransactionId::new());

    let debit_account_id = AccountId::from_str(&req.debit_account_id)
        .map_err(|_| BlazerError::ValidationError("bad debit_account_id".into()))?;

    let credit_account_id = AccountId::from_str(&req.credit_account_id)
        .map_err(|_| BlazerError::ValidationError("bad credit_account_id".into()))?;

    let currency = parse_currency(&req.currency).map_err(|_| {
        BlazerError::ValidationError(format!("unknown currency '{}'", req.currency))
    })?;

    let decimal = req
        .amount
        .parse::<rust_decimal::Decimal>()
        .map_err(|_| BlazerError::ValidationError(format!("bad amount '{}'", req.amount)))?;

    let amount_units = Amount::from_decimal(decimal, currency)
        .map_err(|e| BlazerError::ValidationError(e.to_string()))?
        .minor_units();

    Ok(TransactionEvent {
        transaction_id,
        debit_account_id,
        credit_account_id,
        amount_units,
        currency,
        ledger_id: LedgerId::from(req.ledger_id),
        code: req.code,
        flags: TransactionFlags::default(),
        timestamp: Timestamp::now(),
        result: None,
    })
}
