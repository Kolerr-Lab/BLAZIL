//! AF_XDP RX cursor — the zero-copy wire → pipeline ingestion hot loop.
//!
//! # Zero-copy path
//!
//! ```text
//! NIC DMA ──▶ UMEM frame  (no copy — DMA writes directly into mmap region)
//!                │
//!          RX ring descriptor {addr, len}
//!                │
//!          frame_data(desc)  ──▶ &[u8] into UMEM  (no copy — raw pointer)
//!                │
//!          parse_blazil_frame()  ──▶ &[u8] payload slice (no copy — sub-slice)
//!                │
//!          deserialize_request()  ──▶ TransactionRequest  (msgpack parse; reads
//!                │                    UMEM bytes once, output is stack/heap)
//!                │
//!          desc returned to fill queue  (UMEM frame is free again)
//!                │
//!          build_transaction_event()  ──▶  TransactionEvent
//!                │
//!          pipeline.publish_event()  ──▶  RingBuffer slot
//! ```
//!
//! The only memory traffic on the hot path is:
//! 1. DMA write by NIC (no CPU involvement)
//! 2. Single pass of `rmp_serde::from_slice` reading UMEM bytes
//! 3. `TransactionEvent` construction (one `memcpy`-worth into ring slot)
//!
//! # Response path
//!
//! After the pipeline commits or rejects the event, the response is sent
//! via a plain `std::net::UdpSocket` (CPU TX).  This is intentional:
//! responses are tiny (~50 bytes) and far less frequent than requests.
//! Zero-copy TX can be added in a future iteration if profiling demands it.
//!
//! # Thread model
//!
//! One `RxCursor` per NIC queue, pinned to one OS thread.  The thread runs a
//! busy-poll loop (no epoll sleep) for minimum latency.  CPU pinning is
//! configured via `v0.4_aws_setup.sh` (cores 4+ reserved for bench shards;
//! cores 0-3 for networking).

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, error, warn};

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::BlazerResult;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{TransactionEvent, TransactionResult};
use blazil_engine::pipeline::Pipeline;

use crate::connection::build_response;
use crate::protocol::{deserialize_request, serialize_response, TransactionResponse};

use super::socket::XSocket;
use super::umem::OwnedUmem;

// ── Wire frame constants ──────────────────────────────────────────────────────

/// Ethernet header size (bytes).
const ETH_HDR:   usize = 14;
/// Minimum IPv4 header size (bytes). Variable if IHL > 5.
const IPV4_HDR:  usize = 20;
/// UDP header size (bytes).
const UDP_HDR:   usize = 8;
/// Blazil magic word size (bytes).  Must be "BLZL" = 0x424C5A4C.
const MAGIC_LEN: usize = 4;
/// Minimum payload offset in a Blazil UDP frame.
const PAYLOAD_OFFSET: usize = ETH_HDR + IPV4_HDR + UDP_HDR + MAGIC_LEN;

/// Blazil magic in network byte order (big-endian).
const BLAZIL_MAGIC: [u8; 4] = [0x42, 0x4C, 0x5A, 0x4C]; // "BLZL"

/// How many RX descriptors to consume per poll iteration.  Matches the
/// typical TB batch size so the pipeline sees full batches.
const CONSUME_BATCH: usize = 256;

/// Maximum time to wait for a pipeline result before giving up and sending
/// an error response.  Keeps the fill queue from stalling if TB is slow.
const RESULT_TIMEOUT: Duration = Duration::from_millis(200);

/// Spin iterations before yield in the result-wait loop.
const SPIN_BEFORE_YIELD: u32 = 512;

// ── RxCursor ──────────────────────────────────────────────────────────────────

/// Drives the AF_XDP RX ring for one NIC queue, zero-copying frames into the
/// Blazil engine pipeline.
pub struct RxCursor {
    /// The AF_XDP socket for this queue.
    socket: XSocket,
    /// Shared UMEM — owned via Arc so this cursor can borrow frame memory.
    umem: Arc<OwnedUmem>,
    /// Target pipeline shard.  One RxCursor : one shard : one queue.
    pipeline: Arc<Pipeline>,
    /// Results map for polling committed/rejected outcomes.
    results: Arc<DashMap<i64, TransactionResult>>,
    /// Plain UDP socket for sending responses back to clients.
    /// Source port = BLAZIL_UDP_PORT, dest = sender's (ip, port) from the frame.
    response_sock: UdpSocket,
    /// Stop flag.  Set by `AfXdpTransportServer::shutdown()`.
    stop: Arc<AtomicBool>,
    /// Shard index (for diagnostics).
    shard_id: usize,
}

impl RxCursor {
    pub fn new(
        shard_id: usize,
        socket: XSocket,
        umem: Arc<OwnedUmem>,
        pipeline: Arc<Pipeline>,
        results: Arc<DashMap<i64, TransactionResult>>,
        bind_addr: &str,
        stop: Arc<AtomicBool>,
    ) -> BlazerResult<Self> {
        let response_sock = UdpSocket::bind(bind_addr)
            .map_err(|e| blazil_common::error::BlazerError::Transport(
                format!("RxCursor{shard_id} response socket bind: {e}")
            ))?;
        response_sock.set_nonblocking(true)
            .map_err(|e| blazil_common::error::BlazerError::Transport(
                format!("RxCursor{shard_id} set_nonblocking: {e}")
            ))?;

        Ok(Self {
            socket,
            umem,
            pipeline,
            results,
            response_sock,
            stop,
            shard_id,
        })
    }

    /// Seed the fill queue with all available UMEM frames so the kernel can
    /// begin DMA-filling them.  Must be called once before entering `run()`.
    pub fn seed_fill_queue(&mut self) {
        let descs: Vec<_> = self.umem.free_descs.drain(..).collect();
        let n = descs.len();
        if n == 0 {
            return;
        }
        // SAFETY: descriptors are valid UMEM offsets from OwnedUmem::new().
        let produced = unsafe {
            // wakeup=true: NAPI poll trigger on kernel side.
            self.socket.fill.produce_and_wakeup(&descs)
                .unwrap_or(0)
        };
        debug!(
            shard_id = self.shard_id,
            frames = n,
            produced,
            "AF_XDP fill queue seeded"
        );
    }

    /// The hot loop.  Runs until `stop` is set.
    ///
    /// Busy-polls the RX ring; each batch of received descriptors is processed
    /// inline (zero-copy read → deserialize → pipeline publish → await result
    /// → send response), then the descriptors are returned to the fill queue.
    pub fn run(mut self) {
        self.seed_fill_queue();

        let mut descs = vec![xsk_rs::FrameDesc::default(); CONSUME_BATCH];
        let shard_id  = self.shard_id;

        debug!(shard_id, "AF_XDP RxCursor started");

        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }

            // ── Consume RX ring ───────────────────────────────────────────────
            // SAFETY: `consume` writes valid frame descriptors into `descs`.
            // Each descriptor's addr is a valid offset into OwnedUmem.inner.
            let n_recv = unsafe {
                self.socket.rx.consume(&mut descs)
            };

            if n_recv == 0 {
                // Nothing in the ring — hint CPU and loop (busy-poll).
                std::hint::spin_loop();
                continue;
            }

            // ── Process each received frame ───────────────────────────────────
            let mut filled_back: Vec<xsk_rs::FrameDesc> = Vec::with_capacity(n_recv);

            for desc in descs[..n_recv].iter() {
                // SAFETY:
                // - `desc` was received from the RX ring (kernel gave us ownership).
                // - No other code accesses this frame between now and when we
                //   push desc back to fill (after the block below).
                // - `umem` (and its mmap region) is alive for the duration via Arc.
                let frame = unsafe { self.umem.frame_data(desc) };

                match self.process_frame(frame) {
                    Ok(()) => {}
                    Err(e) => {
                        debug!(shard_id, "AF_XDP frame error: {e}");
                    }
                }

                // Frame bytes no longer needed — return descriptor to fill queue.
                filled_back.push(*desc);
            }

            // ── Return descriptors to fill queue ──────────────────────────────
            // SAFETY: these descriptors were received from RX ring; returning
            // them to fill is the correct protocol.
            let _ = unsafe {
                self.socket.fill.produce_and_wakeup(&filled_back)
            };

            // ── Drain TX completion queue ─────────────────────────────────────
            // (If zero-copy TX is used, comp queue releases TX'd frames.)
            let mut comp_descs = [xsk_rs::FrameDesc::default(); 256];
            let _ = unsafe { self.socket.comp.consume(&mut comp_descs) };
        }

        debug!(shard_id, "AF_XDP RxCursor stopped");
    }

    // ── Frame processing ──────────────────────────────────────────────────────

    /// Parse one Blazil wire frame and inject it into the pipeline.
    ///
    /// Returns `Ok(())` on success (committed or rejected) and
    /// `Err` for malformed frames (which are silently dropped).
    fn process_frame(&self, frame: &[u8]) -> BlazerResult<()> {
        // ── Parse wire headers ────────────────────────────────────────────────
        if frame.len() < PAYLOAD_OFFSET + 1 {
            return Err(blazil_common::error::BlazerError::Transport(
                format!("frame too short: {} bytes", frame.len()),
            ));
        }

        // Verify magic (the XDP program already checked this, but defence-in-depth).
        let magic = &frame[ETH_HDR + IPV4_HDR + UDP_HDR..ETH_HDR + IPV4_HDR + UDP_HDR + MAGIC_LEN];
        if magic != BLAZIL_MAGIC {
            return Err(blazil_common::error::BlazerError::Transport(
                "bad Blazil magic".into(),
            ));
        }

        // Extract source IPv4 address and UDP port for the response.
        // IPv4 src addr: bytes 12-15 of IPv4 header (after ETH 14B).
        let ip_start  = ETH_HDR;
        let src_ip    = &frame[ip_start + 12..ip_start + 16];
        let ip_hlen   = ((frame[ip_start] & 0x0F) as usize) * 4;
        let udp_start = ip_start + ip_hlen;
        // UDP source port: bytes 0-1 of UDP header (network byte order).
        let src_port  = u16::from_be_bytes([frame[udp_start], frame[udp_start + 1]]);
        let src_addr  = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(src_ip[0], src_ip[1], src_ip[2], src_ip[3])),
            src_port,
        );

        // MessagePack payload starts after ETH + IP (variable) + UDP + MAGIC.
        let payload = &frame[ETH_HDR + ip_hlen + UDP_HDR + MAGIC_LEN..];

        // ── Deserialize request ───────────────────────────────────────────────
        // Reads UMEM bytes once, no copy of wire data.
        let req = deserialize_request(payload)?;

        // ── Build TransactionEvent ────────────────────────────────────────────
        let event = super::build_event_from_request(&req)?;

        // ── Publish into pipeline ─────────────────────────────────────────────
        let seq = self.pipeline.try_publish_event(event)
            .map_err(|_| blazil_common::error::BlazerError::Transport("pipeline full".into()))?;

        // ── Await result ──────────────────────────────────────────────────────
        let result = self.wait_for_result(seq);

        // ── Send response (standard UDP TX; not zero-copy) ────────────────────
        let resp = build_response(&req.request_id, &result);
        if let Ok(bytes) = serialize_response(&resp) {
            let _ = self.response_sock.send_to(&bytes, src_addr);
        }

        Ok(())
    }

    /// Spin-wait for a result in the DashMap (committed via ring or rejected).
    fn wait_for_result(&self, seq: i64) -> TransactionResult {
        let deadline = Instant::now() + RESULT_TIMEOUT;
        let mut spins: u32 = 0;
        loop {
            // Check result ring first (fast path — committed transfers).
            if let Some(result) = self.results.remove(&seq).map(|(_, v)| v) {
                return result;
            }
            if Instant::now() >= deadline {
                return TransactionResult::Rejected {
                    reason: blazil_common::error::BlazerError::Transport(
                        "result timeout".into(),
                    ),
                };
            }
            spins = spins.wrapping_add(1);
            if spins & (SPIN_BEFORE_YIELD - 1) == 0 {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }
}
