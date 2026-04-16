//! eBPF program loader and lifecycle manager (Aya).
//!
//! This module loads, attaches, and cleans up the XDP BPF program
//! (`blazil_xdp.bpf.o`) that acts as the packet gatekeeper at the NIC.
//!
//! # What the loader does
//!
//! 1. Load the pre-compiled BPF object (embedded via `include_bytes!` at
//!    compile time — no file I/O at runtime).
//! 2. Attach the `blazil_xdp_filter` program to the target interface at
//!    `XDP_DRV` (native) mode — runs directly in the NIC driver's poll loop,
//!    before any kernel skb allocation.
//! 3. Populate the `xsks_map` XSKMAP with AF_XDP socket file descriptors so
//!    the BPF program can redirect matching packets directly into userspace
//!    ring buffers (zero-copy).
//!
//! # XDP mode fallback
//!
//! `XdpFlags::DRV_MODE` (native driver-level) is requested first.
//! If the driver doesn't support it, Aya raises an error.  Callers can retry
//! with `XdpFlags::SKB_MODE` (generic mode — works everywhere, slightly
//! slower, still filters at XDP hook).  See `XdpGatekeeper::attach_with_fallback`.
//!
//! # Cleanup (Drop)
//!
//! `XdpGatekeeper` implements `Drop`.  When it is dropped (server shutdown),
//! Aya automatically detaches the XDP program from the NIC, restoring normal
//! kernel packet processing.

use std::os::unix::io::RawFd;

use aya::{
    maps::XskMap,
    programs::{Xdp, XdpFlags},
    Ebpf,
};
use tracing::{info, warn};

use blazil_common::error::{BlazerError, BlazerResult};

// ── Embedded BPF object ───────────────────────────────────────────────────────

/// The compiled BPF ELF object, embedded at compile time.
/// `OUT_DIR/blazil_xdp.bpf.o` is produced by `build.rs` via clang.
static BPF_OBJECT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/blazil_xdp.bpf.o"));

/// Name of the XDP function inside the BPF object (must match `SEC("xdp")` label
/// in `ebpf/blazil_xdp.bpf.c`).
const XDP_PROG_NAME: &str = "blazil_xdp_filter";

/// Name of the XSKMAP inside the BPF object (must match `xsks_map SEC(".maps")`
/// in `ebpf/blazil_xdp.bpf.c`).
const XSKS_MAP_NAME: &str = "xsks_map";

// ── XdpGatekeeper ─────────────────────────────────────────────────────────────

/// Owns the loaded BPF program.  Detaches from the NIC on `Drop`.
pub struct XdpGatekeeper {
    /// The Aya eBPF handle — keeps the BPF object alive until we drop it.
    _bpf: Ebpf,
    /// Interface name the program is attached to.
    if_name: String,
}

impl XdpGatekeeper {
    /// Load and attach the XDP gatekeeper to `if_name`.
    ///
    /// Attempts `XDP_DRV` (native NIC driver mode) first for lowest latency.
    /// Falls back to `XDP_SKB` (generic mode via generic XDP path) if the
    /// driver doesn't support native mode.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The BPF object fails to load (verifier rejection)
    /// - The program fails to attach in both DRV and SKB modes
    /// - The `xsks_map` is not found in the object
    pub fn attach(if_name: &str) -> BlazerResult<Self> {
        let mut bpf = Ebpf::load(BPF_OBJECT)
            .map_err(|e| BlazerError::Transport(format!("eBPF load: {e}")))?;

        let program: &mut Xdp = bpf
            .program_mut(XDP_PROG_NAME)
            .ok_or_else(|| {
                BlazerError::Transport(format!(
                    "XDP program '{XDP_PROG_NAME}' not found in BPF object"
                ))
            })?
            .try_into()
            .map_err(|e| BlazerError::Transport(format!("cast XDP program: {e}")))?;

        program
            .load()
            .map_err(|e| BlazerError::Transport(format!("eBPF verify/load: {e}")))?;

        // Attempt native DRV mode (runs in the NIC poll loop, before skb alloc).
        let attach_result = program.attach(if_name, XdpFlags::DRV_MODE);
        match attach_result {
            Ok(_) => {
                info!(if_name, "XDP gatekeeper attached (DRV mode)");
            }
            Err(e) => {
                warn!(
                    if_name,
                    "XDP DRV mode unavailable ({e}); falling back to SKB mode"
                );
                program.attach(if_name, XdpFlags::SKB_MODE).map_err(|e2| {
                    BlazerError::Transport(format!("XDP SKB mode fallback failed: {e2}"))
                })?;
                info!(if_name, "XDP gatekeeper attached (SKB mode — generic)");
            }
        }

        Ok(Self {
            _bpf: bpf,
            if_name: if_name.to_string(),
        })
    }

    /// Register an AF_XDP socket in the `xsks_map` so the BPF program knows
    /// which socket to redirect packets arriving on `queue_id` to.
    ///
    /// Must be called **after** `attach()` and **after** the `XSocket` is created.
    ///
    /// # Arguments
    ///
    /// - `queue_id` — NIC queue index (key in xsks_map).
    /// - `socket_fd` — raw file descriptor of the `XSocket`.
    pub fn register_socket(&mut self, queue_id: u32, socket_fd: RawFd) -> BlazerResult<()> {
        let mut xsks_map: XskMap<_> = self
            ._bpf
            .map_mut(XSKS_MAP_NAME)
            .ok_or_else(|| BlazerError::Transport(format!("'{XSKS_MAP_NAME}' map not found")))?
            .try_into()
            .map_err(|e| BlazerError::Transport(format!("xsks_map cast: {e}")))?;

        xsks_map.set(queue_id, socket_fd, 0).map_err(|e| {
            BlazerError::Transport(format!(
                "xsks_map.set(queue={queue_id}, fd={socket_fd}): {e}"
            ))
        })?;

        info!(queue_id, socket_fd, "AF_XDP socket registered in xsks_map");
        Ok(())
    }

    /// Interface this gatekeeper is attached to.
    pub fn if_name(&self) -> &str {
        &self.if_name
    }
}

impl Drop for XdpGatekeeper {
    fn drop(&mut self) {
        // Aya's own Drop impl on `Ebpf` / the attached `Xdp` program detaches
        // the XDP hook from the interface automatically.  Normal kernel packet
        // processing is restored without any syscall from our side.
        info!(if_name = %self.if_name, "XDP gatekeeper detached");
    }
}
