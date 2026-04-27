//! AF_XDP socket bound to one NIC RX/TX queue.
//!
//! Each `XSocket` represents a single `AF_XDP` file descriptor bound to one
//! queue of the NIC.  The typical setup for Blazil is:
//!
//! - NIC queues 0-7 for the 8 bench shards (one queue per shard)
//! - All sockets share one `OwnedUmem`
//!
//! # Queue isolation
//!
//! AF_XDP sockets only receive packets that the XDP program redirects to
//! their specific queue index.  Packets on other queues are invisible.
//! The eBPF `xsks_map` (populated by `ebpf/mod.rs`) maps queue index → fd.
//!
//! # Zero-copy vs copy mode
//!
//! `XSocketConfig::zero_copy = true` attempts `XDP_ZEROCOPY`.  If the NIC
//! driver does not support it (e.g. `virtio_net` < 5.10 or `tap`), the kernel
//! silently falls back to `XDP_COPY`.  On AWS ENA (i4i.metal) with kernel ≥
//! 5.10, zero-copy works natively.

use std::num::NonZeroU32;
use std::os::unix::io::RawFd;

use xsk_rs::{
    config::{BindFlags, SocketConfig},
    socket::{InterfaceName, Socket},
    CompletionQueue, FillQueue, RxQueue, TxQueue,
};

use blazil_common::error::{BlazerError, BlazerResult};

use super::umem::OwnedUmem;

// ── Ring sizes ────────────────────────────────────────────────────────────────

/// RX ring depth per socket.  Must be power of 2.  8 K slots at 2048 B/frame
/// = 16 MiB max on-the-wire data simultaneously queued per shard.
pub const RX_RING_SIZE: u32 = 8192;

/// TX ring depth per socket (needed even if we use std::net for responses).
pub const TX_RING_SIZE: u32 = 2048;

// ── XSocketConfig ─────────────────────────────────────────────────────────────

/// Configuration for one AF_XDP socket.
pub struct XSocketConfig {
    /// NIC interface name, e.g. `"eth0"` or `"enX0"` on AWS.
    pub if_name: String,
    /// NIC RX/TX queue index.  Must match the XDP `ctx->rx_queue_index` in
    /// the eBPF program and the Aya `xsks_map` entry.
    pub queue_id: u32,
    /// Request zero-copy DMA mode.  Falls back to copy-mode if unsupported.
    pub zero_copy: bool,
}

// ── XSocket ───────────────────────────────────────────────────────────────────

/// One AF_XDP socket bound to a single NIC queue.
///
/// Owns the four rings associated with its queue:
/// - `rx`: descriptors of frames received from the wire (kernel → userspace)
/// - `tx`: descriptors of frames to send (userspace → kernel)
/// - `fill`: free frame descriptors returned to the kernel for reuse
/// - `comp`: descriptors of TX-completed frames (kernel → userspace)
pub struct XSocket {
    /// Underlying socket (kept alive to maintain the kernel resource).
    _socket: Socket,
    /// RX ring — poll here to receive packets.
    pub rx: RxQueue,
    /// TX ring — write here to send packets (responses use std::net::UdpSocket
    /// for simplicity; TX ring reserved for future zero-copy TX).
    pub tx: TxQueue,
    /// Fill ring — place free frame descs here so the kernel can fill them.
    pub fill: FillQueue,
    /// Completion ring — kernel returns TX-completed descs here.
    pub comp: CompletionQueue,
    /// Raw socket fd, passed to `xsks_map` via Aya.
    pub raw_fd: RawFd,
}

impl XSocket {
    /// Create an AF_XDP socket bound to `cfg.queue_id` on `cfg.if_name`,
    /// sharing the provided `OwnedUmem`.
    ///
    /// Must be called **after** the Aya eBPF program is loaded and attached
    /// to the interface (so the XDP hook is active when the socket binds).
    ///
    /// # Safety
    ///
    /// `Socket::new` performs kernel syscalls that:
    /// - Create an `AF_XDP` socket fd
    /// - Bind it to the specified interface/queue
    /// - Map UMEM ring memory into the process address space
    ///
    /// The caller must ensure `umem` outlives this socket (enforced by the
    /// `Arc<OwnedUmem>` ownership pattern in `AfXdpTransportServer`).
    pub fn new(cfg: &XSocketConfig, umem: &OwnedUmem) -> BlazerResult<Self> {
        let if_name = InterfaceName::try_from(cfg.if_name.as_str())
            .map_err(|e| BlazerError::Transport(format!("invalid if_name '{}': {e}", cfg.if_name)))?;

        let rx_size = NonZeroU32::new(RX_RING_SIZE).unwrap();
        let tx_size = NonZeroU32::new(TX_RING_SIZE).unwrap();

        let mut bind_flags = BindFlags::empty();
        if cfg.zero_copy {
            bind_flags |= BindFlags::XDP_ZEROCOPY;
        }

        let socket_config = SocketConfig::builder()
            .rx_queue_size(rx_size)
            .tx_queue_size(tx_size)
            .bind_flags(bind_flags)
            .build()
            .map_err(|e| BlazerError::Transport(format!("socket config: {e}")))?;

        // SAFETY: Socket::new creates AF_XDP fd, binds to interface/queue, and
        // maps ring memory.  We hold `umem` alive via Arc in the caller.
        let (socket, tx, rx, fq_cq) = unsafe {
            Socket::new(socket_config, umem.inner(), &if_name, cfg.queue_id)
                .map_err(|e| BlazerError::Transport(format!(
                    "AF_XDP socket bind to {}@queue{}: {e}",
                    cfg.if_name, cfg.queue_id
                )))?
        };

        // fq_cq is Some only for the *first* socket created on a UMEM.
        // If this is a subsequent socket sharing the UMEM, fill/comp queues are
        // managed by the first socket.  In our setup (one UMEM, one socket per
        // queue owned by AfXdpTransportServer), there is always exactly one
        // socket per UMEM so this is always Some.
        let (fill, comp) = fq_cq.ok_or_else(|| {
            BlazerError::Transport(
                "fill/comp queues unavailable — sharing UMEM across sockets \
                 requires calling Socket::new with fq_cq=None handling"
                    .into(),
            )
        })?;

        let raw_fd = socket.fd_raw();

        Ok(Self {
            _socket: socket,
            rx,
            tx,
            fill,
            comp,
            raw_fd,
        })
    }
}
